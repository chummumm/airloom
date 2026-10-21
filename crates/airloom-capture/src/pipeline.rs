//! Native sample conversion and a stateful, anti-aliased stereo resampler.

use std::collections::VecDeque;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::{CaptureError, Result};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Encoding {
    Unsigned8,
    Signed16,
    Signed24,
    Signed32,
    Float32,
    Float64,
}

impl Encoding {
    fn bytes(self) -> usize {
        match self {
            Self::Unsigned8 => 1,
            Self::Signed16 => 2,
            Self::Signed24 => 3,
            Self::Signed32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    fn decode(self, bytes: &[u8]) -> f32 {
        let value = match self {
            Self::Unsigned8 => (f32::from(bytes[0]) - 128.0) / 128.0,
            Self::Signed16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0,
            Self::Signed24 => {
                let signed = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                signed as f32 / 8_388_608.0
            }
            Self::Signed32 => {
                i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f32 / 2_147_483_648.0
            }
            Self::Float32 => f32::from_le_bytes(bytes[..4].try_into().unwrap()),
            Self::Float64 => {
                let sample = f64::from_le_bytes(bytes[..8].try_into().unwrap());
                // Clamp before narrowing so a finite, oversized f64 does not
                // turn into infinity and get mistaken for an invalid sample.
                if sample.is_finite() {
                    sample.clamp(-1.0, 1.0) as f32
                } else {
                    0.0
                }
            }
        };
        finite_sample(value)
    }
}

fn finite_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

pub(crate) struct NativeFormat {
    pub(crate) sample_rate: u32,
    pub(crate) block_align: usize,
    encoding: Encoding,
    weights: Vec<[f32; 2]>,
}

impl NativeFormat {
    pub(crate) fn new(
        sample_rate: u32,
        channels: usize,
        block_align: usize,
        channel_mask: u32,
        encoding: Encoding,
    ) -> Result<Self> {
        if !(8_000..=384_000).contains(&sample_rate) || !(1..=8).contains(&channels) {
            return Err(CaptureError::UnsupportedFormat(
                "expected 1–8 channels at 8000–384000 Hz".into(),
            ));
        }
        if block_align < channels * encoding.bytes() || block_align > 256 {
            return Err(CaptureError::UnsupportedFormat(
                "invalid PCM frame alignment".into(),
            ));
        }
        let weights = stereo_weights(channels, channel_mask)?;
        Ok(Self {
            sample_rate,
            block_align,
            encoding,
            weights,
        })
    }

    /// Does not read `bytes` for a WASAPI silent packet, whose data pointer may
    /// legitimately be NULL. Extensible integer PCM is left aligned in its
    /// container, so normalization by container width also handles valid bits.
    pub(crate) fn decode(&self, bytes: &[u8], frames: usize, silent: bool) -> Result<Vec<f32>> {
        let byte_count = frames
            .checked_mul(self.block_align)
            .ok_or_else(|| CaptureError::UnsupportedFormat("PCM packet length overflow".into()))?;
        if !silent && bytes.len() < byte_count {
            return Err(CaptureError::UnsupportedFormat(
                "truncated PCM packet".into(),
            ));
        }
        let mut stereo = vec![
            0.0;
            frames.checked_mul(2).ok_or_else(|| {
                CaptureError::UnsupportedFormat("PCM frame count overflow".into())
            })?
        ];
        if silent {
            return Ok(stereo);
        }

        for (frame, output) in bytes[..byte_count]
            .chunks_exact(self.block_align)
            .zip(stereo.chunks_exact_mut(2))
        {
            for (channel, weight) in self.weights.iter().enumerate() {
                let offset = channel * self.encoding.bytes();
                let sample = self
                    .encoding
                    .decode(&frame[offset..offset + self.encoding.bytes()]);
                output[0] += sample * weight[0];
                output[1] += sample * weight[1];
            }
            output[0] = finite_sample(output[0]);
            output[1] = finite_sample(output[1]);
        }
        Ok(stereo)
    }
}

fn stereo_weights(channels: usize, channel_mask: u32) -> Result<Vec<[f32; 2]>> {
    if channels == 1 {
        return Ok(vec![[1.0, 1.0]]);
    }
    if channels == 2 {
        return Ok(vec![[1.0, 0.0], [0.0, 1.0]]);
    }
    if channel_mask.count_ones() as usize != channels || channel_mask & !0x3ffff != 0 {
        return Err(CaptureError::UnsupportedFormat(
            "multichannel audio requires a valid speaker mask".into(),
        ));
    }
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let mut weights = Vec::with_capacity(channels);
    for speaker in 0..18 {
        if channel_mask & (1 << speaker) == 0 {
            continue;
        }
        weights.push(match speaker {
            0 => [1.0, 0.0], // front left
            1 => [0.0, 1.0], // front right
            2 => [diagonal, diagonal],
            3 => [0.0, 0.0], // LFE omitted from a stereo downmix
            4 | 6 | 9 | 12 | 15 => [diagonal, 0.0],
            5 | 7 | 10 | 14 | 17 => [0.0, diagonal],
            _ => [0.5, 0.5], // back/top centre
        });
    }
    // A bounded, predictable headroom policy for correlated surround channels.
    // Mono and stereo bypass this normalization to preserve their unity gain.
    let gains = weights.iter().fold([0.0_f32; 2], |sum, gain| {
        [sum[0] + gain[0], sum[1] + gain[1]]
    });
    for weight in &mut weights {
        weight[0] /= gains[0].max(1.0);
        weight[1] /= gains[1].max(1.0);
    }
    Ok(weights)
}

const RESAMPLE_FRAMES: usize = 256;

pub(crate) struct StereoResampler {
    resampler: Option<SincFixedIn<f32>>,
    pending: VecDeque<[f32; 2]>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
}

impl StereoResampler {
    pub(crate) fn new(input_rate: u32, output_rate: u32) -> Result<Self> {
        let resampler = if input_rate == output_rate {
            None
        } else {
            let parameters = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            Some(
                SincFixedIn::new(
                    f64::from(output_rate) / f64::from(input_rate),
                    1.0,
                    parameters,
                    RESAMPLE_FRAMES,
                    2,
                )
                .map_err(|error| CaptureError::Conversion(error.to_string()))?,
            )
        };
        let output = resampler
            .as_ref()
            .map(|r| r.output_buffer_allocate(true))
            .unwrap_or_default();
        Ok(Self {
            resampler,
            pending: VecDeque::with_capacity(RESAMPLE_FRAMES * 4),
            input: vec![vec![0.0; RESAMPLE_FRAMES]; 2],
            output,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }

    pub(crate) fn push(&mut self, stereo: Vec<f32>) -> Result<Vec<Vec<f32>>> {
        if stereo.len() % 2 != 0 {
            return Err(CaptureError::Conversion("incomplete stereo frame".into()));
        }
        if stereo.is_empty() {
            return Ok(Vec::new());
        }
        let Some(resampler) = &mut self.resampler else {
            return Ok(vec![stereo]);
        };
        self.pending
            .extend(stereo.chunks_exact(2).map(|frame| [frame[0], frame[1]]));
        let mut chunks = Vec::new();
        while self.pending.len() >= RESAMPLE_FRAMES {
            for frame in 0..RESAMPLE_FRAMES {
                let pair = self
                    .pending
                    .pop_front()
                    .expect("pending length was checked");
                self.input[0][frame] = pair[0];
                self.input[1][frame] = pair[1];
            }
            let (_, written) = resampler
                .process_into_buffer(&self.input, &mut self.output, None)
                .map_err(|error| CaptureError::Conversion(error.to_string()))?;
            let mut output = Vec::with_capacity(written * 2);
            for frame in 0..written {
                output.push(finite_sample(self.output[0][frame]));
                output.push(finite_sample(self.output[1][frame]));
            }
            if !output.is_empty() {
                chunks.push(output);
            }
        }
        Ok(chunks)
    }
}

pub(crate) fn levels(samples: &[f32]) -> ([f32; 2], [f32; 2]) {
    let mut squares = [0.0_f64; 2];
    let mut peaks = [0.0_f32; 2];
    let frames = samples.len() / 2;
    if frames == 0 {
        return ([0.0; 2], [0.0; 2]);
    }
    for pair in samples.chunks_exact(2) {
        for channel in 0..2 {
            let sample = finite_sample(pair[channel]);
            squares[channel] += f64::from(sample).powi(2);
            peaks[channel] = peaks[channel].max(sample.abs());
        }
    }
    (
        [
            (squares[0] / frames as f64).sqrt() as f32,
            (squares[1] / frames as f64).sqrt() as f32,
        ],
        peaks,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_24_bit_extremes_and_mono_duplication() {
        let format = NativeFormat::new(48_000, 1, 3, 0, Encoding::Signed24).unwrap();
        assert_eq!(format.sample_rate, 48_000);
        let pcm = format
            .decode(&[0, 0, 0x80, 0xff, 0xff, 0x7f], 2, false)
            .unwrap();
        assert_eq!(&pcm[..2], &[-1.0, -1.0]);
        assert!((pcm[2] - (1.0 - 1.0 / 8_388_608.0)).abs() < 0.000001);
        assert_eq!(pcm[2], pcm[3]);
    }

    #[test]
    fn integer_extremes_and_wide_floats_keep_their_sign_and_scale() {
        assert_eq!(Encoding::Unsigned8.decode(&[0]), -1.0);
        assert_eq!(Encoding::Unsigned8.decode(&[128]), 0.0);
        assert_eq!(Encoding::Signed32.decode(&i32::MIN.to_le_bytes()), -1.0);
        assert_eq!(
            Encoding::Signed32.decode(&1_073_741_824_i32.to_le_bytes()),
            0.5
        );
        assert_eq!(Encoding::Float64.decode(&f64::MAX.to_le_bytes()), 1.0);
        assert_eq!(Encoding::Float64.decode(&(-f64::MAX).to_le_bytes()), -1.0);
        assert_eq!(Encoding::Float64.decode(&f64::NAN.to_le_bytes()), 0.0);
    }

    #[test]
    fn float_nan_is_silent_and_truncated_packet_is_rejected() {
        let format = NativeFormat::new(48_000, 2, 8, 3, Encoding::Float32).unwrap();
        let mut bytes = f32::NAN.to_le_bytes().to_vec();
        bytes.extend(0.5_f32.to_le_bytes());
        assert_eq!(format.decode(&bytes, 1, false).unwrap(), vec![0.0, 0.5]);
        assert!(format.decode(&bytes[..4], 1, false).is_err());
        assert_eq!(format.decode(&[], 1, true).unwrap(), vec![0.0, 0.0]);
    }

    #[test]
    fn surround_lfe_is_not_repeated_in_both_channels() {
        let format = NativeFormat::new(48_000, 6, 12, 0x3f, Encoding::Signed16).unwrap();
        let mut bytes = vec![0; 12];
        bytes[6..8].copy_from_slice(&i16::MAX.to_le_bytes());
        assert_eq!(format.decode(&bytes, 1, false).unwrap(), vec![0.0, 0.0]);
        assert!(NativeFormat::new(48_000, 6, 12, 0, Encoding::Signed16).is_err());
    }

    #[test]
    fn resampler_preserves_channel_separation_and_rate_over_many_packets() {
        let mut resampler = StereoResampler::new(48_000, 44_100).unwrap();
        let mut output = Vec::new();
        for _ in 0..100 {
            let input = (0..480).flat_map(|_| [0.25, -0.25]).collect();
            for chunk in resampler.push(input).unwrap() {
                output.extend(chunk);
            }
        }
        // Some input waits for a complete 256-frame block; sinc also has startup delay.
        let frames = output.len() / 2;
        assert!((43_500..=44_100).contains(&frames), "{frames}");
        for pair in output[2_000..].chunks_exact(2) {
            assert!((pair[0] - 0.25).abs() < 0.005);
            assert!((pair[1] + 0.25).abs() < 0.005);
        }
    }

    #[test]
    fn telemetry_reports_each_channel_separately() {
        let (rms, peak) = levels(&[0.5, -1.0, -0.5, 0.0]);
        assert_eq!(rms[0], 0.5);
        assert!((rms[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.000001);
        assert_eq!(peak, [0.5, 1.0]);
    }

    #[test]
    fn discontinuity_reset_discards_pending_audio_and_filter_history() {
        let mut resampler = StereoResampler::new(48_000, 44_100).unwrap();
        resampler.push(vec![0.75; 256 * 2 * 4 + 128 * 2]).unwrap();
        resampler.reset();
        let chunks = resampler.push(vec![0.0; 256 * 2 * 5]).unwrap();
        assert!(!chunks.is_empty());
        assert!(chunks
            .iter()
            .flatten()
            .all(|sample| sample.abs() < 0.000001));
    }
}
