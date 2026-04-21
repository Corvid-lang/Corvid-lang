use crate::{render_descriptor_json, CorvidAbi};
use sha2::{Digest, Sha256};

pub fn hash_json_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

pub fn hash_json_str(json: &str) -> [u8; 32] {
    hash_json_bytes(json.as_bytes())
}

pub fn hash_abi(abi: &CorvidAbi) -> Result<[u8; 32], serde_json::Error> {
    Ok(hash_json_str(&render_descriptor_json(abi)?))
}
