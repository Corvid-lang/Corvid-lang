use crate::canonical_hash::{hash_abi, hash_json_str};
use crate::{descriptor_from_json, render_descriptor_json, CorvidAbi, CORVID_ABI_VERSION};
use libloading::Library;
use std::fmt;
use std::path::Path;

pub const CORVID_ABI_SECTION_MAGIC: u32 = 0x434F5256;
pub const CORVID_ABI_DESCRIPTOR_SYMBOL: &str = "CORVID_ABI_DESCRIPTOR";

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedDescriptorSection {
    pub magic: u32,
    pub abi_version: u32,
    pub json_len: u64,
    pub json: String,
    pub sha256: [u8; 32],
}

#[derive(Debug)]
pub enum EmbeddedDescriptorError {
    TooShort { len: usize },
    BadMagic { found: u32 },
    Utf8(std::string::FromUtf8Error),
    Json(serde_json::Error),
    HashMismatch,
    LengthOverflow(u64),
    VersionMismatch { found: u32, expected: u32 },
    SymbolLoad(String),
}

impl fmt::Display for EmbeddedDescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => write!(f, "descriptor section too short: {len} bytes"),
            Self::BadMagic { found } => write!(f, "bad descriptor magic: 0x{found:08X}"),
            Self::Utf8(err) => write!(f, "descriptor JSON was not valid UTF-8: {err}"),
            Self::Json(err) => write!(f, "descriptor JSON parse failed: {err}"),
            Self::HashMismatch => write!(f, "descriptor embedded hash did not match JSON payload"),
            Self::LengthOverflow(len) => write!(f, "descriptor length {len} does not fit in memory"),
            Self::VersionMismatch { found, expected } => write!(
                f,
                "descriptor abi_version {found} did not match current CORVID_ABI_VERSION {expected}"
            ),
            Self::SymbolLoad(err) => write!(f, "failed to load embedded descriptor symbol: {err}"),
        }
    }
}

impl std::error::Error for EmbeddedDescriptorError {}

pub fn descriptor_to_embedded_bytes(abi: &CorvidAbi) -> Result<Vec<u8>, serde_json::Error> {
    let json = render_descriptor_json(abi)?;
    let hash = hash_abi(abi)?;
    let mut bytes = Vec::with_capacity(16 + json.len() + 32);
    bytes.extend_from_slice(&CORVID_ABI_SECTION_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&CORVID_ABI_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u64).to_le_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes.extend_from_slice(&hash);
    Ok(bytes)
}

pub fn parse_embedded_section_bytes(
    bytes: &[u8],
) -> Result<EmbeddedDescriptorSection, EmbeddedDescriptorError> {
    if bytes.len() < 16 + 32 {
        return Err(EmbeddedDescriptorError::TooShort { len: bytes.len() });
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("magic width"));
    if magic != CORVID_ABI_SECTION_MAGIC {
        return Err(EmbeddedDescriptorError::BadMagic { found: magic });
    }
    let abi_version = u32::from_le_bytes(bytes[4..8].try_into().expect("version width"));
    if abi_version != CORVID_ABI_VERSION {
        return Err(EmbeddedDescriptorError::VersionMismatch {
            found: abi_version,
            expected: CORVID_ABI_VERSION,
        });
    }
    let json_len = u64::from_le_bytes(bytes[8..16].try_into().expect("len width"));
    let json_len_usize = usize::try_from(json_len)
        .map_err(|_| EmbeddedDescriptorError::LengthOverflow(json_len))?;
    let expected_len = 16usize
        .checked_add(json_len_usize)
        .and_then(|len| len.checked_add(32))
        .ok_or(EmbeddedDescriptorError::LengthOverflow(json_len))?;
    if bytes.len() < expected_len {
        return Err(EmbeddedDescriptorError::TooShort { len: bytes.len() });
    }
    let json_bytes = &bytes[16..16 + json_len_usize];
    let json = String::from_utf8(json_bytes.to_vec()).map_err(EmbeddedDescriptorError::Utf8)?;
    let sha_offset = 16 + json_len_usize;
    let sha256: [u8; 32] = bytes[sha_offset..sha_offset + 32]
        .try_into()
        .expect("sha width");
    if hash_json_str(&json) != sha256 {
        return Err(EmbeddedDescriptorError::HashMismatch);
    }
    let abi = descriptor_from_json(&json).map_err(EmbeddedDescriptorError::Json)?;
    abi.validate_supported_version()
        .map_err(|_| EmbeddedDescriptorError::VersionMismatch {
            found: abi.corvid_abi_version,
            expected: CORVID_ABI_VERSION,
        })?;
    Ok(EmbeddedDescriptorSection {
        magic,
        abi_version,
        json_len,
        json,
        sha256,
    })
}

pub fn read_embedded_section_from_library(
    path: &Path,
) -> Result<EmbeddedDescriptorSection, EmbeddedDescriptorError> {
    unsafe {
        let lib = Library::new(path).map_err(|err| {
            EmbeddedDescriptorError::SymbolLoad(format!("open `{}`: {err}", path.display()))
        })?;
        let symbol: libloading::Symbol<*const u8> = lib
            .get(format!("{CORVID_ABI_DESCRIPTOR_SYMBOL}\0").as_bytes())
            .map_err(|err| EmbeddedDescriptorError::SymbolLoad(err.to_string()))?;
        let ptr = *symbol;
        if ptr.is_null() {
            return Err(EmbeddedDescriptorError::SymbolLoad(
                "symbol resolved to null".to_string(),
            ));
        }
        let header = std::slice::from_raw_parts(ptr, 16);
        let json_len = u64::from_le_bytes(header[8..16].try_into().expect("len width"));
        let total_len = usize::try_from(json_len)
            .ok()
            .and_then(|len| len.checked_add(16 + 32))
            .ok_or(EmbeddedDescriptorError::LengthOverflow(json_len))?;
        let section_bytes = std::slice::from_raw_parts(ptr, total_len);
        parse_embedded_section_bytes(section_bytes)
    }
}

pub fn descriptor_from_embedded_section(
    section: &EmbeddedDescriptorSection,
) -> Result<CorvidAbi, serde_json::Error> {
    descriptor_from_json(&section.json)
}
