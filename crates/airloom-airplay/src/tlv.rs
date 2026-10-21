use crate::{protocol, Result};
use std::collections::BTreeMap;

pub const METHOD: u8 = 0;
pub const IDENTIFIER: u8 = 1;
pub const SALT: u8 = 2;
pub const PUBLIC_KEY: u8 = 3;
pub const PROOF: u8 = 4;
pub const ENCRYPTED: u8 = 5;
pub const STATE: u8 = 6;
pub const ERROR: u8 = 7;
pub const SIGNATURE: u8 = 10;
pub const FLAGS: u8 = 19;

pub fn encode(fields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(tag, bytes) in fields {
        if bytes.is_empty() {
            out.extend_from_slice(&[tag, 0]);
        }
        for chunk in bytes.chunks(255) {
            out.extend_from_slice(&[tag, chunk.len() as u8]);
            out.extend_from_slice(chunk);
        }
    }
    out
}

pub fn decode(mut input: &[u8]) -> Result<BTreeMap<u8, Vec<u8>>> {
    if input.len() > 16 * 1024 {
        return Err(protocol("pairing TLV exceeds size limit"));
    }
    let mut fields = BTreeMap::<u8, Vec<u8>>::new();
    let mut previous = None;
    while !input.is_empty() {
        if input.len() < 2 {
            return Err(protocol("truncated TLV header"));
        }
        let (tag, len) = (input[0], input[1] as usize);
        input = &input[2..];
        if input.len() < len {
            return Err(protocol("truncated TLV value"));
        }
        if fields.contains_key(&tag) && previous != Some((tag, 255)) {
            return Err(protocol("duplicate pairing TLV field"));
        }
        fields
            .entry(tag)
            .or_default()
            .extend_from_slice(&input[..len]);
        previous = Some((tag, len));
        input = &input[len..];
    }
    Ok(fields)
}

pub fn required(fields: &BTreeMap<u8, Vec<u8>>, tag: u8) -> Result<&[u8]> {
    fields
        .get(&tag)
        .map(Vec::as_slice)
        .ok_or_else(|| protocol(format!("missing pairing field {tag}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_large_public_key_and_rejects_malformed_data() {
        let public = vec![42; 384];
        let encoded = encode(&[(STATE, &[3]), (PUBLIC_KEY, &public)]);
        assert_eq!(decode(&encoded).unwrap()[&PUBLIC_KEY], public);
        assert!(decode(&[1, 3, 1]).is_err());
        assert!(decode(&[6, 1, 2, 6, 1, 4]).is_err());
        assert!(decode(&[6]).is_err());
    }
}
