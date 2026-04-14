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

/// Refcounted heap allocator. Linked with every binary so the runtime
/// helpers (`corvid_alloc` / `corvid_retain` / `corvid_release`) are
/// available even when the program doesn't allocate (the symbols cost
/// nothing if unreferenced).
pub const ALLOC_SOURCE: &str = include_str!("../runtime/alloc.c");

/// String runtime helpers (`corvid_string_concat` / `_eq` / `_cmp`).
/// Linked alongside the allocator.
pub const STRINGS_SOURCE: &str = include_str!("../runtime/strings.c");

/// List runtime helpers (`corvid_destroy_list_refcounted`). One shared
/// destructor for every refcounted-element list type — operating on
/// raw pointers, it doesn't care whether T is String, Struct, or
/// nested List.
pub const LISTS_SOURCE: &str = include_str!("../runtime/lists.c");

/// Entry-agent helpers: argv decoding (parse_i64/_f64/_bool),
/// result printing (print_i64/_bool/_f64/_string), arity-mismatch
/// reporting, and the atexit registration for leak-counter printing.
/// Linked alongside the rest of the runtime; the codegen-emitted
/// `main` calls into these helpers per the entry agent's signature.
pub const ENTRY_SOURCE: &str = include_str!("../runtime/entry.c");

/// Link `object_path` together with the built-in entry shim into an
/// executable at `output_path`. Creates parent directories as needed.
/// The object file must export a symbol named `corvid_entry` — the
/// codegen emits a trampoline with that name that calls the chosen
/// entry agent, which keeps the shim free of per-user patching.
pub fn link_binary(
    object_path: &Path,
    _entry_agent_symbol: &str,
    output_path: &Path,
    // Phase 14: tool-implementation staticlibs to link in. The Cranelift
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

    // Write the (unmodified) shim, allocator, and string runtime to a
    // temp dir the compiler can read.
    let shim_dir = tempfile::Builder::new()
        .prefix("corvid-link-")
        .tempdir()
        .map_err(|e| CodegenError::io(format!("tempdir: {e}")))?;
    let shim_path = shim_dir.path().join("corvid_shim.c");
    let alloc_path = shim_dir.path().join("corvid_alloc.c");
    let strings_path = shim_dir.path().join("corvid_strings.c");
    let lists_path = shim_dir.path().join("corvid_lists.c");
    let entry_path = shim_dir.path().join("corvid_entry.c");
    std::fs::write(&shim_path, ENTRY_SHIM_SOURCE)
        .map_err(|e| CodegenError::io(format!("write shim: {e}")))?;
    std::fs::write(&alloc_path, ALLOC_SOURCE)
        .map_err(|e| CodegenError::io(format!("write alloc: {e}")))?;
    std::fs::write(&strings_path, STRINGS_SOURCE)
        .map_err(|e| CodegenError::io(format!("write strings: {e}")))?;
    std::fs::write(&lists_path, LISTS_SOURCE)
        .map_err(|e| CodegenError::io(format!("write lists: {e}")))?;
    std::fs::write(&entry_path, ENTRY_SOURCE)
        .map_err(|e| CodegenError::io(format!("write entry: {e}")))?;

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
        // MSVC: cl.exe writes intermediate .obj files into the cwd
        // unless /Fo redirects them. Per-invocation tempdir so parallel
        // tests don't collide. `/std:c11` enables `<stdatomic.h>`,
        // which the alloc.c runtime depends on.
        let obj_out_dir = shim_dir.path();
        cmd.arg("/std:c11")
            .arg("/experimental:c11atomics")
            .arg(format!(
                "/Fo{}{}",
                obj_out_dir.display(),
                std::path::MAIN_SEPARATOR
            ))
            .arg(&shim_path)
            .arg(&alloc_path)
            .arg(&strings_path)
            .arg(&lists_path)
            .arg(&entry_path)
            .arg(object_path)
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
        // GCC/Clang: cc shim.c alloc.c strings.c lists.c entry.c object.o
        //   libcorvid_runtime.a <native system libs> -o output
        // `-std=c11` enables `<stdatomic.h>` portably. The staticlib
        // goes after the .o (left-to-right symbol resolution on Unix
        // linkers; user code references staticlib symbols, not the
        // other way around).
        cmd.arg("-std=c11")
            .arg(&shim_path)
            .arg(&alloc_path)
            .arg(&strings_path)
            .arg(&lists_path)
            .arg(&entry_path)
            .arg(object_path);
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
