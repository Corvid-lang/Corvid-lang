//! Compile cache for the native-tier `corvid run` path.
//!
//! Without a cache, `corvid run foo.cor` re-compiles + re-links on every
//! invocation, which destroys the "native is faster" promise of the auto
//! dispatch — linking through `cl.exe` or `cc` costs seconds even for
//! trivial programs. This cache lets the second run of an unchanged
//! file skip codegen and linking entirely.
//!
//! ## Cache key
//!
//! FNV-1a-64 over every input that can change the emitted binary's
//! behaviour:
//!
//!   - source bytes (the `.cor` file text)
//!   - the `corvid-codegen-cl` package version (changes on codegen edits)
//!   - every C runtime shim shipped with the codegen crate
//!     (`shim.c` / `entry.c` / `alloc.c` / `strings.c` / `lists.c`)
//!
//! Not covered: host toolchain upgrades (`cl.exe` / `cc` version), libc
//! changes. The manual escape hatch is `cargo clean` or `rm -rf
//! target/cache/native/` — same as any other build cache.
//!
//! FNV-1a is not cryptographic. For a build cache it only needs to be
//! deterministic and collision-resistant-enough that two distinct input
//! sets don't hash to the same 64-bit key in practice. That bar is met.

use corvid_codegen_cl::link;
use std::path::{Path, PathBuf};

/// Where compiled native binaries live. Rooted under the same `target/`
/// the rest of the driver uses, so `cargo clean` sweeps it.
pub fn cache_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(root) = dir.parent() {
                return root.join("target").join("cache").join("native");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("cache").join("native")
}

/// Compute the cache key for a given source. Deterministic — same input
/// bytes always produce the same 16-char hex string.
pub fn cache_key(source: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let feed = |bytes: &[u8], h: &mut u64| {
        for b in bytes {
            *h ^= *b as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    };
    feed(source.as_bytes(), &mut h);
    feed(b"|cl=", &mut h);
    feed(env!("CARGO_PKG_VERSION").as_bytes(), &mut h);
    feed(b"|shim=", &mut h);
    feed(link::ENTRY_SHIM_SOURCE.as_bytes(), &mut h);
    feed(b"|entry=", &mut h);
    feed(link::ENTRY_SOURCE.as_bytes(), &mut h);
    feed(b"|alloc=", &mut h);
    feed(link::ALLOC_SOURCE.as_bytes(), &mut h);
    feed(b"|strings=", &mut h);
    feed(link::STRINGS_SOURCE.as_bytes(), &mut h);
    feed(b"|lists=", &mut h);
    feed(link::LISTS_SOURCE.as_bytes(), &mut h);
    format!("{h:016x}")
}

/// Build the final binary path for `(cache_dir, key)`. Adds `.exe` on
/// Windows per the codegen's own `binary_extension` — keeps the cache
/// format in sync with the live build.
pub fn cached_binary_path(cache_dir: &Path, key: &str) -> PathBuf {
    let ext = link::binary_extension();
    if ext.is_empty() {
        cache_dir.join(key)
    } else {
        cache_dir.join(format!("{key}.{ext}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_source_same_key() {
        let a = cache_key("agent f() -> Int:\n    return 1\n");
        let b = cache_key("agent f() -> Int:\n    return 1\n");
        assert_eq!(a, b);
    }

    #[test]
    fn different_source_different_key() {
        let a = cache_key("agent f() -> Int:\n    return 1\n");
        let b = cache_key("agent f() -> Int:\n    return 2\n");
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_is_hex_16() {
        let k = cache_key("agent f() -> Int:\n    return 0\n");
        assert_eq!(k.len(), 16);
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
