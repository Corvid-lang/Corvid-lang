//! Build script for `corvid-codegen-cl`.
//!
//! Computes the path where Cargo places the `corvid-runtime` staticlib
//! artifact at the current profile, and emits it as a compile-time env
//! var (`CORVID_STATICLIB_DIR`). `link.rs` reads that via `env!()` at
//! compile time and passes the staticlib to the native linker when
//! producing compiled Corvid binaries.
//!
//! Why compute this at build-script time (not runtime): the path
//! depends on `CARGO_TARGET_DIR` / workspace layout / profile, all of
//! which are known at build time and stable for the lifetime of the
//! `corvid-codegen-cl` binary we're compiling. Runtime discovery would
//! need to walk file system for the staticlib and re-parse profile
//! state — lazy semantics we're avoiding on purpose. The linker setup
//! stays eager and explicit for anything load-bearing.

use std::path::PathBuf;

fn main() {
    // `OUT_DIR` is `<target-dir>/<profile>/build/<crate>-<hash>/out`.
    // Walk up to the `<target-dir>/<profile>/` level — that's where
    // Cargo writes per-crate artifact files including `corvid_runtime.lib`
    // (Windows) / `libcorvid_runtime.a` (Unix).
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("walk up from OUT_DIR to profile dir");

    // Emit as a compile-time env var. `env!("CORVID_STATICLIB_DIR")` in
    // lib.rs / link.rs resolves to this absolute path at compilation.
    println!(
        "cargo:rustc-env=CORVID_STATICLIB_DIR={}",
        profile_dir.display()
    );

    // Rebuild the dependent code if corvid-runtime's sources change
    // so the staticlib path check stays current.
    println!("cargo:rerun-if-changed=build.rs");
}
