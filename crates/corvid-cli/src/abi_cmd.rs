use anyhow::{anyhow, Context, Result};
use corvid_abi::read_embedded_section_from_library;
use corvid_driver::{build_catalog_descriptor_for_source, render_all_pretty};
use std::path::Path;

pub fn run_dump(library: &Path) -> Result<u8> {
    let section = read_embedded_section_from_library(library)
        .with_context(|| format!("read embedded descriptor from `{}`", library.display()))?;
    println!("{}", section.json);
    Ok(0)
}

pub fn run_hash(source: &Path) -> Result<u8> {
    let out = build_catalog_descriptor_for_source(source)?;
    if let Some(json) = out.descriptor_json {
        let hash = corvid_abi::hash_json_str(&json);
        println!("{}", encode_hex(&hash));
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&out.diagnostics, source, &out.source));
        Ok(1)
    }
}

pub fn run_verify(library: &Path, expected_hash: &str) -> Result<u8> {
    let section = read_embedded_section_from_library(library)
        .with_context(|| format!("read embedded descriptor from `{}`", library.display()))?;
    let expected = decode_hex(expected_hash)?;
    Ok(if section.sha256 == expected { 0 } else { 2 })
}

fn decode_hex(hex: &str) -> Result<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(anyhow!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut out = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (decode_nibble(chunk[0])? << 4) | decode_nibble(chunk[1])?;
    }
    Ok(out)
}

fn decode_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(anyhow!("invalid hex character `{}`", byte as char)),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
