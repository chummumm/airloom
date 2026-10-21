use crate::{crypto, protocol, Error, Result, CHANNELS, FRAMES_PER_PACKET};
use zeroize::Zeroize;

/// ALAC uncompressed-frame encoder, expressed directly from the published
/// ALAC bitstream. This is lossless; it deliberately performs no compression.
pub fn alac_packet(samples: &[i16]) -> Result<Vec<u8>> {
    if samples.len() != FRAMES_PER_PACKET * CHANNELS {
        return Err(Error::InvalidArgument(
            "ALAC packet requires 352 stereo frames".into(),
        ));
    }
    let mut bits = BitWriter::default();
    bits.write(1, 3); // channel-pair element
    bits.write(0, 4); // element instance tag
    bits.write(0, 12); // reserved
    bits.write(0, 1); // frame count comes from negotiated format
    bits.write(0, 2); // no low-order bytes separately stored
    bits.write(1, 1); // uncompressed samples
    for sample in samples {
        bits.write(*sample as u16 as u32, 16);
    }
    bits.write(7, 3); // end element
    Ok(bits.bytes)
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u8,
}
impl BitWriter {
    fn write(&mut self, value: u32, count: u8) {
        for index in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> index) & 1) as u8) << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }
}

pub struct PacketEncoder {
    key: [u8; 32],
    nonce: u64,
    pub sequence: u16,
    pub timestamp: u32,
    ssrc: u32,
    first: bool,
}
impl Drop for PacketEncoder {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
impl PacketEncoder {
    pub fn new(key: [u8; 32], timestamp: u32, ssrc: u32) -> Self {
        let sequence = u16::from_le_bytes(crypto::random());
        Self {
            key,
            nonce: sequence as u64,
            sequence,
            timestamp,
            ssrc,
            first: true,
        }
    }
    pub fn packet(&mut self, samples: &[i16]) -> Result<(u16, Vec<u8>)> {
        let payload = alac_packet(samples)?;
        let mut header = [0u8; 12];
        header[0] = 0x80;
        header[1] = 0x60 | if self.first { 0x80 } else { 0 };
        header[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        header[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        header[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        let nonce = crypto::named_nonce(&self.nonce.to_le_bytes());
        let cipher = crypto::seal(&self.key, &nonce, &header[4..12], &payload)?;
        let mut output = header.to_vec();
        output.extend(cipher);
        output.extend_from_slice(&self.nonce.to_le_bytes());
        self.nonce = self
            .nonce
            .checked_add(1)
            .ok_or_else(|| protocol("audio nonce exhausted"))?;
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(FRAMES_PER_PACKET as u32);
        self.first = false;
        Ok((sequence, output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// FFmpeg is an independent decoder used only for this optional developer
    /// test. It is not shipped, linked, or invoked by the application.
    #[test]
    #[ignore = "requires FFmpeg on PATH"]
    fn independent_ffmpeg_decodes_alac_to_bit_exact_pcm() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let samples: Vec<i16> = (0..FRAMES_PER_PACKET * CHANNELS)
            .map(|i| (i as i16).wrapping_mul(97).wrapping_add(i16::MIN))
            .collect();
        // Exercise the actual encrypted RTP payload, not only the standalone
        // encoder: a receiver reconstructs the nonce from the wire suffix,
        // authenticates the RTP timestamp/SSRC, then decodes the ALAC frame.
        let key = [0x5a; 32];
        let (_, wire) = PacketEncoder::new(key, 0xffff_ff00, 0)
            .packet(&samples)
            .unwrap();
        let suffix: [u8; 8] = wire[wire.len() - 8..].try_into().unwrap();
        let packet = crypto::open(
            &key,
            &crypto::named_nonce(&suffix),
            &wire[4..12],
            &wire[12..wire.len() - 8],
        )
        .unwrap();
        // Minimal CAF container, fixed-size packets, ALACSpecificConfig cookie.
        let mut caf = b"caff\x00\x01\x00\x00".to_vec();
        fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(tag);
            out.extend_from_slice(&(data.len() as u64).to_be_bytes());
            out.extend_from_slice(data);
        }
        let mut description = 44100f64.to_be_bytes().to_vec();
        description.extend_from_slice(b"alac");
        for field in [1, packet.len() as u32, FRAMES_PER_PACKET as u32, 2, 0] {
            description.extend_from_slice(&field.to_be_bytes());
        }
        chunk(&mut caf, b"desc", &description);
        let mut cookie = (FRAMES_PER_PACKET as u32).to_be_bytes().to_vec();
        cookie.extend_from_slice(&[0, 16, 40, 10, 14, 2]);
        cookie.extend_from_slice(&255u16.to_be_bytes());
        for field in [0u32, 0, 44100] {
            cookie.extend_from_slice(&field.to_be_bytes());
        }
        chunk(&mut caf, b"kuki", &cookie);
        let mut data = vec![0; 4];
        data.extend(packet);
        chunk(&mut caf, b"data", &data);
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "caf",
                "-i",
                "pipe:0",
                "-f",
                "s16le",
                "-acodec",
                "pcm_s16le",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("FFmpeg installed");
        child.stdin.take().unwrap().write_all(&caf).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            output.stdout, expected,
            "independent decoder must recover every sample"
        );
    }
    #[test]
    fn alac_bitstream_contains_exact_signed_stereo_samples() {
        let samples: Vec<i16> = (0..704)
            .map(|i| (i as i16).wrapping_mul(97).wrapping_add(i16::MIN))
            .collect();
        let encoded = alac_packet(&samples).unwrap();
        let bit = |position: usize| (encoded[position / 8] >> (7 - position % 8)) & 1;
        assert_eq!((0..3).fold(0u32, |v, i| (v << 1) | bit(i) as u32), 1);
        assert_eq!(bit(22), 1);
        for (index, expected) in samples.iter().enumerate() {
            let value = (0..16).fold(0u16, |value, b| {
                (value << 1) | bit(23 + index * 16 + b) as u16
            });
            assert_eq!(value as i16, *expected);
        }
        assert_eq!(encoded.len(), 1412);
    }
    #[test]
    fn rtp_encryption_uses_unique_full_nonce_across_sequence_wrap() {
        let key = [5; 32];
        let mut encoder = PacketEncoder::new(key, 10, 20);
        encoder.sequence = u16::MAX;
        encoder.nonce = u16::MAX as u64;
        let (_, a) = encoder.packet(&[0; 704]).unwrap();
        let (_, b) = encoder.packet(&[0; 704]).unwrap();
        assert_ne!(&a[a.len() - 8..], &b[b.len() - 8..]);
        let n: [u8; 8] = b[b.len() - 8..].try_into().unwrap();
        let clear = crypto::open(
            &key,
            &crypto::named_nonce(&n),
            &b[4..12],
            &b[12..b.len() - 8],
        )
        .unwrap();
        assert_eq!(clear, alac_packet(&[0; 704]).unwrap());
        assert_eq!(&b[2..4], &0u16.to_be_bytes());
    }
}
