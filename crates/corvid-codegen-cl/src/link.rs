//! Invoke the system C toolchain to link the emitted object file into
//! a native binary.
//!
//! Uses the `cc` crate's compiler discovery (`cc::Build::new().get_compiler()`)
//! so we pick up `cl.exe` on Windows/MSVC, `cc`/`clang` on macOS, and
//! `cc` on Linux. We then drive it directly via `std::process::Command`
//! because `cc::Build` is optimised for build-script use and does not
//! expose a "link these objects into this binary" entry point on all
//! platforms uniformly.

use crate::errors::CodegenError;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Source of the C entry shim. Compiled and linked with every binary.
pub const ENTRY_SHIM_SOURCE: &str = include_str!("../runtime/shim.c");

/// Link `object_path` together with the built-in entry shim into an
/// executable at `output_path`. Creates parent directories as needed.
/// The object file must export a symbol named `corvid_entry` — the
/// codegen emits a trampoline with that name that calls the chosen
/// entry agent, which keeps the shim free of per-user patching.
pub fn link_binary(
    object_path: &Path,
    _entry_agent_symbol: &str,
    output_path: &Path,
) -> Result<(), CodegenError> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CodegenError::io(format!("create {}: {e}", parent.display())))?;
    }

    // Write the (unmodified) shim to a temp file the compiler can read.
    let shim_dir = tempfile::Builder::new()
        .prefix("corvid-link-")
        .tempdir()
        .map_err(|e| CodegenError::io(format!("tempdir: {e}")))?;
    let shim_path = shim_dir.path().join("corvid_shim.c");
    std::fs::write(&shim_path, ENTRY_SHIM_SOURCE)
        .map_err(|e| CodegenError::io(format!("write shim: {e}")))?;

    let compiler = cc::Build::new()
        .opt_level(2)
        .cargo_metadata(false)
        .cargo_warnings(false)
        .host(&target_lexicon::HOST.to_string())
        .target(&target_lexicon::HOST.to_string())
        .try_get_compiler()
        .map_err(|e| CodegenError::link(format!("compiler discovery: {e}")))?;

    let path_to_cc = compiler.path();
    let mut cmd = Command::new(path_to_cc);
    // Start from the compiler's detected args (include paths, MSVC env
    // vars, cross-compile flags, etc.) so we inherit whatever the host
    // toolchain needs.
    for (k, v) in compiler.env() {
        cmd.env(k, v);
    }

    if compiler.is_like_msvc() {
        // MSVC: cl.exe writes the intermediate shim .obj into the cwd
        // unless /Fo redirects it. Point at the per-invocation tempdir
        // so parallel compiles don't collide on `corvid_shim.obj`.
        // `/Fo<path>` without a colon is the canonical form; trailing
        // backslash makes cl treat it as a directory.
        let obj_out_dir = shim_dir.path();
        cmd.arg(format!(
            "/Fo{}{}",
            obj_out_dir.display(),
            std::path::MAIN_SEPARATOR
        ))
        .arg(&shim_path)
        .arg(object_path)
        .arg(format!("/Fe:{}", output_path.display()));
    } else {
        // GCC/Clang: cc shim.c object.o -o output
        cmd.arg(&shim_path)
            .arg(object_path)
            .arg("-o")
            .arg(output_path);
    }

    let output = cmd
        .output()
        .map_err(|e| CodegenError::link(format!("spawn linker `{}`: {e}", path_to_cc.display())))?;
    if !output.status.success() {
        return Err(CodegenError::link(format!(
            "linker exited {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        )));
    }
    // Keep `shim_dir` alive until link completes.
    drop(shim_dir);
    Ok(())
}

/// The host output-file suffix. `.exe` on Windows, nothing elsewhere.
pub fn binary_extension() -> &'static str {
    if cfg!(windows) {
        "exe"
    } else {
        ""
    }
}

/// Produce an appropriate output path for `stem` under `out_dir`.
pub fn binary_path_for(out_dir: &Path, stem: &str) -> PathBuf {
    let ext = binary_extension();
    if ext.is_empty() {
        out_dir.join(stem)
    } else {
        out_dir.join(format!("{stem}.{ext}"))
    }
}
