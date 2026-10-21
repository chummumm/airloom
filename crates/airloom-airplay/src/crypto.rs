use crate::{protocol, Error, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use hkdf::Hkdf;
use num_bigint::BigUint;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha512};
use srp::{client::SrpClient, groups::G_3072};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

pub fn derive(secret: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut key = [0; 32];
    // A fixed 32-byte output is always within HKDF-SHA512's size limit.
    Hkdf::<Sha512>::new(Some(salt), secret)
        .expand(info, &mut key)
        .expect("HKDF output length");
    key
}

pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new(key.into())
        .encrypt(Nonce::from_slice(nonce), Payload { msg: data, aad })
        .map_err(|_| protocol("encryption failed"))
}

pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    ChaCha20Poly1305::new(key.into())
        .decrypt(Nonce::from_slice(nonce), Payload { msg: data, aad })
        .map_err(|_| protocol("encrypted message authentication failed"))
}

pub fn named_nonce(name: &[u8; 8]) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(name);
    nonce
}

pub struct ChannelCipher {
    key: [u8; 32],
    counter: u64,
}

impl Drop for ChannelCipher {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl ChannelCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, counter: 0 }
    }
    fn nonce(&self) -> [u8; 12] {
        named_nonce(&self.counter.to_le_bytes())
    }
    fn advance(&mut self) -> Result<()> {
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| protocol("encrypted channel counter exhausted"))?;
        Ok(())
    }
    pub fn encode(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut encoded = Vec::with_capacity(plaintext.len() + plaintext.len().div_ceil(1024) * 18);
        for chunk in plaintext.chunks(1024) {
            let size = (chunk.len() as u16).to_le_bytes();
            let ciphertext = seal(&self.key, &self.nonce(), &size, chunk)?;
            self.advance()?;
            encoded.extend_from_slice(&size);
            encoded.extend_from_slice(&ciphertext);
        }
        Ok(encoded)
    }
    pub fn decode_frame(&mut self, size: [u8; 2], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let expected = u16::from_le_bytes(size) as usize;
        if expected > 1024 || ciphertext.len() != expected + 16 {
            return Err(protocol("invalid HAP frame size"));
        }
        let plain = open(&self.key, &self.nonce(), &size, ciphertext)?;
        self.advance()?;
        Ok(plain)
    }
}

/// HomeKit's SHA512/3072 SRP profile. Modular SRP operations come from
/// RustCrypto; HAP's proof transcript differs from srp 0.6's legacy API.
pub struct SrpExchange {
    pub public: Vec<u8>,
    pub proof: [u8; 64],
    expected_server_proof: [u8; 64],
    session_key: Zeroizing<Vec<u8>>,
}

fn hash(parts: &[&[u8]]) -> [u8; 64] {
    let mut h = Sha512::new();
    for part in parts {
        h.update(part);
    }
    h.finalize().into()
}

fn pad(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() > 384 {
        return Err(protocol("oversized SRP value"));
    }
    let mut padded = vec![0; 384];
    padded[384 - bytes.len()..].copy_from_slice(bytes);
    Ok(padded)
}

impl SrpExchange {
    pub fn begin(password: &[u8], salt: &[u8], server_public: &[u8]) -> Result<Self> {
        let private = Zeroizing::new(random::<32>());
        Self::with_secret(password, salt, server_public, private.as_ref())
    }
    fn with_secret(
        password: &[u8],
        salt: &[u8],
        server_public: &[u8],
        secret: &[u8],
    ) -> Result<Self> {
        if !(8..=64).contains(&salt.len()) || server_public.is_empty() || server_public.len() > 384
        {
            return Err(protocol("invalid SRP challenge"));
        }
        let group = &*G_3072;
        let client = SrpClient::<Sha512>::new(group);
        let b = BigUint::from_bytes_be(server_public);
        if &b % &group.n == BigUint::default() || b >= group.n {
            return Err(protocol("invalid SRP server public key"));
        }
        let public = client.compute_public_ephemeral(secret);
        let a_padded = pad(&public)?;
        let b_padded = pad(server_public)?;
        let k = BigUint::from_bytes_be(&hash(&[
            &group.n.to_bytes_be(),
            &pad(&group.g.to_bytes_be())?,
        ]));
        let u = BigUint::from_bytes_be(&hash(&[&a_padded, &b_padded]));
        if u == BigUint::default() {
            return Err(protocol("invalid SRP scrambling parameter"));
        }
        let identity = SrpClient::<Sha512>::compute_identity_hash(b"Pair-Setup", password);
        let x = SrpClient::<Sha512>::compute_x(identity.as_slice(), salt);
        let premaster =
            client.compute_premaster_secret(&b, &k, &x, &BigUint::from_bytes_be(secret), &u);
        let secret_bytes = Zeroizing::new(premaster.to_bytes_be());
        let key = hash(&[&secret_bytes]);
        let mut ng = hash(&[&group.n.to_bytes_be()]);
        for (a, b) in ng.iter_mut().zip(hash(&[&group.g.to_bytes_be()])) {
            *a ^= b;
        }
        let proof = hash(&[
            &ng,
            &hash(&[b"Pair-Setup"]),
            salt,
            &public,
            server_public,
            &key,
        ]);
        let expected_server_proof = hash(&[&public, &proof, &key]);
        Ok(Self {
            public,
            proof,
            expected_server_proof,
            session_key: Zeroizing::new(key.to_vec()),
        })
    }
    pub fn verify(self, proof: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if proof.ct_eq(&self.expected_server_proof).unwrap_u8() != 1 {
            return Err(Error::AuthenticationRejected {
                kind: crate::AuthenticationKind::Password,
            });
        }
        Ok(self.session_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn srp_matches_independently_generated_srptools_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/srp_sha512_3072.json")).unwrap();
        let bytes = |key: &str| hex::decode(fixture[key].as_str().unwrap()).unwrap();
        let exchange = SrpExchange::with_secret(
            fixture["password"].as_str().unwrap().as_bytes(),
            &bytes("salt"),
            &bytes("server_public"),
            &bytes("client_private"),
        )
        .unwrap();
        assert_eq!(exchange.public, bytes("client_public"));
        assert_eq!(exchange.proof.as_slice(), bytes("client_proof"));
        assert_eq!(
            exchange.verify(&bytes("server_proof")).unwrap().as_slice(),
            bytes("session_key")
        );
        let invalid = SrpExchange::with_secret(
            b"wrong password",
            &bytes("salt"),
            &bytes("server_public"),
            &bytes("client_private"),
        )
        .unwrap();
        assert!(matches!(
            invalid.verify(&bytes("server_proof")),
            Err(Error::AuthenticationRejected { .. })
        ));
    }
    #[test]
    fn hap_frames_are_authenticated_and_counters_are_directional() {
        let key = [9; 32];
        let data = vec![7; 2300];
        let encoded = ChannelCipher::new(key).encode(&data).unwrap();
        let mut decoder = ChannelCipher::new(key);
        let mut out = Vec::new();
        let mut position = 0;
        while position < encoded.len() {
            let size = [encoded[position], encoded[position + 1]];
            let end = position + 2 + u16::from_le_bytes(size) as usize + 16;
            out.extend(
                decoder
                    .decode_frame(size, &encoded[position + 2..end])
                    .unwrap(),
            );
            position = end;
        }
        assert_eq!(out, data);
        let mut corrupt = encoded[2..1042].to_vec();
        corrupt[19] ^= 1;
        assert!(ChannelCipher::new(key)
            .decode_frame([0, 4], &corrupt)
            .is_err());
    }
    #[test]
    fn rejects_invalid_srp_server_values_before_modular_exponentiation() {
        assert!(SrpExchange::begin(b"secret", &[1; 16], &[0]).is_err());
        assert!(SrpExchange::begin(b"secret", &[1; 16], &G_3072.n.to_bytes_be()).is_err());
    }
}
