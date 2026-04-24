//! Invoke the system C toolchain to link the emitted object file into
//! a native binary.
//!
//! Uses the `cc` crate's compiler discovery (`cc::Build::new().get_compiler()`)
//! so we pick up `cl.exe` on Windows/MSVC, `cc`/`clang` on macOS, and
//! `cc` on Linux. We drive it directly via `std::process::Command`
//! because `cc::Build` is optimised for build-script use and does not
//! expose a "link these objects into this binary" entry point on all
//! platforms uniformly.
//!
//! The C runtime files (alloc.c, strings.c, lists.c, entry.c, shim.c)
//! live in `corvid-runtime/runtime/`. `corvid-runtime`'s build.rs
//! compiles them into the runtime static libraries, so this
//! linker invocation just needs to combine the Cranelift-emitted .obj
//! with whichever runtime-bearing staticlib the caller picked.

use crate::errors::CodegenError;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Link `object_path` together with the runtime staticlib(s) into an
/// executable at `output_path`. Creates parent directories as needed.
pub fn link_binary(
    object_path: &Path,
    _entry_agent_symbol: &str,
    output_path: &Path,
    // Tool-implementation staticlibs to link in. The Cranelift
    // codegen's `IrCallKind::Tool` lowering emits calls to
    // `__corvid_tool_<name>` symbols which must be provided by these
    // libs; if an expected symbol is missing, the linker fails with a
    // clear "unresolved external" error at build time rather than a
    // runtime "tool not found" at execution time.
    extra_tool_libs: &[&Path],
) -> Result<(), CodegenError> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CodegenError::io(format!("create {}: {e}", parent.display())))?;
    }

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

    // Locate the corvid-runtime staticlib. `CORVID_STATICLIB_DIR` is set
    // at build-script time to `<target>/<profile>/` — the directory
    // where Cargo writes artifact files. The staticlib filename follows
    // platform convention (`corvid_runtime.lib` on MSVC, `libcorvid_runtime.a`
    // on Unix). Resolved here, not in the build script, so the exact
    // filename matches the host we're linking on right now.
    //
    // When the caller supplies a tools staticlib via `extra_tool_libs`,
    // that lib transitively includes corvid-runtime (via rlib dep) and
    // linking BOTH would produce LNK2005 duplicate-symbol errors —
    // each staticlib bundles its own copy of Rust's std. Resolution:
    // link exactly one "runtime-bearing" staticlib, either
    // corvid-runtime standalone (tool-free programs) or the user's
    // tools crate (tool-using programs; their staticlib brings the
    // runtime along for the ride).
    let staticlib_dir = std::path::Path::new(env!("CORVID_STATICLIB_DIR"));
    let runtime_staticlib_path = if compiler.is_like_msvc() {
        staticlib_dir.join("corvid_runtime.lib")
    } else {
        staticlib_dir.join("libcorvid_runtime.a")
    };
    let link_standalone_runtime = extra_tool_libs.is_empty();
    if link_standalone_runtime && !runtime_staticlib_path.exists() {
        return Err(CodegenError::link(format!(
            "corvid-runtime staticlib missing at `{}`. Run `cargo build -p corvid-runtime --release` — this is a build-setup issue, not a codegen bug.",
            runtime_staticlib_path.display()
        )));
    }

    if compiler.is_like_msvc() {
        // MSVC: cl.exe acts as the link driver. The C runtime is
        // already bundled into the runtime-bearing staticlib we link
        // below, so we hand cl.exe only the Cranelift object plus
        // that one runtime/tools library.
        cmd.arg(object_path)
            .arg(format!("/Fe:{}", output_path.display()));
        // Exactly ONE runtime-bearing staticlib: either the standalone
        // corvid-runtime (tool-free programs) or the user's tools
        // staticlib (which transitively includes corvid-runtime via
        // its rlib dep). Linking both triggers LNK2005 on every Rust
        // std symbol.
        if link_standalone_runtime {
            cmd.arg(&runtime_staticlib_path);
        } else {
            for lib in extra_tool_libs {
                cmd.arg(lib);
            }
        }
        cmd
            // `/link` separates cl.exe driver args from linker args.
            // Everything after this goes straight to link.exe.
            .arg("/link")
            // Make the PE deterministic so rebuild verification can
            // compare committed and rebuilt binaries byte-for-byte.
            .arg("/BREPRO")
            // Native system libs tokio + reqwest + rustls + Rust's
            // std need on MSVC. Discovered via
            //   `rustc --print native-static-libs --crate-type staticlib`
            // on the corvid-runtime build. Update this list if the
            // corvid-runtime dep graph changes in a way that adds
            // new system-lib requirements.
            .arg("bcrypt.lib")
            .arg("advapi32.lib")
            .arg("kernel32.lib")
            .arg("ntdll.lib")
            .arg("userenv.lib")
            .arg("ws2_32.lib")
            .arg("dbghelp.lib")
            // Rust's std expects legacy_stdio_definitions on MSVC
            // (printf family implementations); msvcrt is pulled via
            // /defaultlib by cl.exe already, so we don't add it
            // explicitly.
            .arg("legacy_stdio_definitions.lib");
    } else {
        // GCC/Clang: cc object.o libcorvid_runtime.a <native libs> -o output
        // The runtime-bearing staticlib already bundles the C runtime,
        // so just hand the linker the object + that one library.
        cmd.arg(object_path);
        // Exactly ONE runtime-bearing staticlib (see MSVC branch above
        // for the LNK2005 explanation — same constraint applies on
        // Unix, just with a different linker phrasing).
        if link_standalone_runtime {
            cmd.arg(&runtime_staticlib_path);
        } else {
            for lib in extra_tool_libs {
                cmd.arg(lib);
            }
        }
        cmd
            // System libs tokio + reqwest + rustls + Rust std need
            // on Linux / macOS. The set is near-identical; macOS
            // additions are frameworks (`-framework Security` etc.).
            // Conservative minimal set below works on both; add
            // platform-specific frameworks as a cfg! chain if a
            // future rustls or tokio bump demands it.
            .arg("-lpthread")
            .arg("-ldl")
            .arg("-lm")
            .arg("-o")
            .arg(output_path);

        if cfg!(target_os = "macos") {
            cmd.arg("-framework").arg("Security");
            cmd.arg("-framework").arg("CoreFoundation");
            cmd.arg("-framework").arg("SystemConfiguration");
        } else if cfg!(target_os = "linux") {
            // Linux-specific libs rustls / reqwest pull in when the
            // platform crypto provider is active.
            cmd.arg("-lutil");
        }
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
