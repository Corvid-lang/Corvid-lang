//! Compile the C runtime files (`runtime/*.c`) into corvid-runtime's
//! library artifact. This makes corvid-runtime self-contained — every
//! `extern "C"` reference its abi + ffi_bridge modules make to the C
//! helpers (`corvid_alloc`, `corvid_release`, `corvid_string_from_bytes`,
//! `corvid_runtime_overflow`, etc.) resolves at link time of any binary
//! that depends on corvid-runtime, including Rust test binaries that
//! never touch the native-codegen pipeline.
//!
//! Without this, link errors like
//!   `unresolved external symbol corvid_string_from_bytes`
//! surface in cargo-test for any crate that depends on corvid-runtime,
//! because Rust extern "C" declarations don't synthesize implementations.
//!
//! The C files were previously compiled by `corvid-codegen-cl`'s
//! `link.rs` at user-binary link time. Phase 15 moves the compilation
//! here so corvid-codegen-cl just links against corvid-runtime's
//! staticlib (which already contains the C objects).

fn main() {
    let runtime_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");

    let mut build = cc::Build::new();
    build
        .file(runtime_dir.join("alloc.c"))
        .file(runtime_dir.join("strings.c"))
        .file(runtime_dir.join("lists.c"))
        .file(runtime_dir.join("entry.c"))
        .file(runtime_dir.join("shim.c"))
        .file(runtime_dir.join("stack_maps.c"))
        .file(runtime_dir.join("stack_maps_fallback.c"))
        .file(runtime_dir.join("collector.c"))
        .file(runtime_dir.join("verify.c"))
        .opt_level(2);

    // C standard: C11 kept for designated initializers in static
    // typeinfo blocks (corvid_typeinfo_String in alloc.c).
    if cc::Build::new().get_compiler().is_like_msvc() {
        build.flag("/std:c11");
    } else {
        build.flag("-std=c11");
    }

    build.compile("corvid_c_runtime");

    // Expose the path to the C-runtime staticlib so downstream
    // crates (corvid-codegen-cl's link.rs, corvid-codegen-cl's
    // ffi_bridge_smoke test) can pass it to the linker when
    // assembling a binary that uses corvid-runtime via static
    // linking. Rust's normal `cargo:rustc-link-lib=static=...`
    // directive only auto-links into Rust executable / dylib
    // targets — staticlib consumers (compiled Corvid binaries
    // linked outside cargo) need to know the path explicitly.
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let lib_name = if cfg!(target_env = "msvc") {
        "corvid_c_runtime.lib"
    } else {
        "libcorvid_c_runtime.a"
    };
    let lib_path = std::path::Path::new(&out_dir).join(lib_name);
    let path_rs = std::path::Path::new(&out_dir).join("c_runtime_path.rs");
    std::fs::write(
        &path_rs,
        format!(
            "/// Absolute path to the `corvid_c_runtime` staticlib produced by\n\
             /// `corvid-runtime`'s build.rs. Downstream crates that build\n\
             /// non-cargo binaries linking against `corvid-runtime` must add\n\
             /// this path to their linker invocation; `cargo:rustc-link-lib`\n\
             /// only flows through cargo-managed link steps.\n\
             pub const C_RUNTIME_LIB_PATH: &str = {:?};\n",
            lib_path.to_string_lossy()
        ),
    )
    .expect("write c_runtime_path.rs");

    // Cargo rebuilds when any C source changes.
    println!("cargo:rerun-if-changed=runtime/alloc.c");
    println!("cargo:rerun-if-changed=runtime/strings.c");
    println!("cargo:rerun-if-changed=runtime/lists.c");
    println!("cargo:rerun-if-changed=runtime/entry.c");
    println!("cargo:rerun-if-changed=runtime/shim.c");
    println!("cargo:rerun-if-changed=runtime/stack_maps.c");
    println!("cargo:rerun-if-changed=runtime/stack_maps_fallback.c");
    println!("cargo:rerun-if-changed=runtime/collector.c");
    println!("cargo:rerun-if-changed=runtime/verify.c");
    println!("cargo:rerun-if-changed=build.rs");
}
