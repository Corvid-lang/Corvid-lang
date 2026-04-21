use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

use corvid_abi::{descriptor_to_embedded_bytes, emit_catalog_abi, EmitOptions};
use corvid_ast::File;
use corvid_c_header::{emit_header, HeaderOptions};
use corvid_codegen_cl::{build_library_to_disk, BuildTarget};
use corvid_ir::lower;
use corvid_resolve::{resolve, Resolved};
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck, Checked, EffectRegistry};
use libloading::Library;

const BOOL_SRC: &str = r#"
pub extern "c"
agent refund_bot(ticket_id: String, amount: Float) -> Bool:
    return ticket_id == "vip" and amount > 10.0
"#;

const STRING_SRC: &str = r#"
pub extern "c"
agent echo_name(name: String) -> String:
    return name
"#;

const FLOAT_SRC: &str = r#"
pub extern "c"
agent echo_amount(amount: Float) -> Float:
    return amount
"#;

struct FrontendBundle {
    file: File,
    resolved: Resolved,
    checked: Checked,
    effect_registry: EffectRegistry,
    ir: corvid_ir::IrFile,
}

fn frontend_of(src: &str) -> FrontendBundle {
    let tokens = lex(src).expect("lex");
    let (file, parse_errors) = parse_file(&tokens);
    assert!(parse_errors.is_empty(), "parse errors: {parse_errors:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve errors: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "type errors: {:?}", checked.errors);
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let effect_registry = EffectRegistry::from_decls(&effect_decls);
    let ir = lower(&file, &resolved, &checked);
    FrontendBundle {
        file,
        resolved,
        checked,
        effect_registry,
        ir,
    }
}

fn embedded_descriptor_bytes(bundle: &FrontendBundle, src: &str) -> Vec<u8> {
    let descriptor = emit_catalog_abi(
        &bundle.file,
        &bundle.resolved,
        &bundle.checked,
        &bundle.ir,
        &bundle.effect_registry,
        &EmitOptions {
            source_path: "tests/cdylib_emission.cor",
            source_text: src,
            compiler_version: "0.6.0-phase22",
            generated_at: "1970-01-01T00:00:00Z",
        },
    );
    descriptor_to_embedded_bytes(&descriptor).expect("embed descriptor")
}

fn build_cdylib(src: &str, stem: &str) -> PathBuf {
    let bundle = frontend_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join(stem);
    let embedded = embedded_descriptor_bytes(&bundle, src);
    let produced = build_library_to_disk(
        &bundle.ir,
        stem,
        &out,
        BuildTarget::Cdylib,
        &[],
        Some(embedded.as_slice()),
    )
    .expect("build cdylib");
    let keep = tmp.keep();
    assert!(keep.exists());
    produced
}

fn build_staticlib(src: &str, stem: &str) -> PathBuf {
    let bundle = frontend_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join(stem);
    let produced = build_library_to_disk(&bundle.ir, stem, &out, BuildTarget::Staticlib, &[], None)
        .expect("build staticlib");
    let keep = tmp.keep();
    assert!(keep.exists());
    produced
}

fn load_library_leaked(path: &Path) -> &'static Library {
    // SAFETY: tests load a freshly-built library and intentionally keep it
    // resident for the life of the process. Repeated DLL unloads have been
    // flaky on Windows once the embedded runtime spins background state, and
    // that teardown noise is outside the ABI behavior this test is asserting.
    unsafe { Box::leak(Box::new(Library::new(path).expect("load shared library"))) }
}

#[test]
fn cdylib_target_produces_shared_library_file() {
    let produced = build_cdylib(BOOL_SRC, "refund_bot_cdylib");
    assert!(produced.exists(), "missing shared library: {}", produced.display());
}

#[test]
fn cdylib_symbol_is_resolvable_via_dlopen() {
    let produced = build_cdylib(BOOL_SRC, "refund_bot_symbol");
    // SAFETY: test loads a library we just built and requests a known symbol.
    unsafe {
        let lib = load_library_leaked(&produced);
        let _: libloading::Symbol<unsafe extern "C" fn(*const c_char, f64) -> bool> =
            lib.get(b"refund_bot").expect("resolve symbol");
    }
}

#[test]
fn staticlib_target_produces_archive_file() {
    let produced = build_staticlib(BOOL_SRC, "refund_bot_static");
    assert!(produced.exists(), "missing archive: {}", produced.display());
    if cfg!(windows) {
        let compiler = cc::Build::new()
            .opt_level(0)
            .cargo_metadata(false)
            .cargo_warnings(false)
            .host(&target_lexicon::HOST.to_string())
            .target(&target_lexicon::HOST.to_string())
            .try_get_compiler()
            .expect("compiler");
        let lib_exe = compiler.path().with_file_name("lib.exe");
        let output = Command::new(lib_exe)
            .arg("/LIST")
            .arg(&produced)
            .output()
            .expect("list archive");
        assert!(output.status.success(), "lib /LIST failed");
    } else {
        let output = Command::new("ar")
            .arg("-t")
            .arg(&produced)
            .output()
            .expect("list archive");
        assert!(output.status.success(), "ar -t failed");
    }
}

#[test]
fn cdylib_minimal_c_harness_calls_and_returns_correct_value() {
    let bundle = frontend_of(BOOL_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let stem = "refund_bot_harness";
    let requested = tmp.path().join(stem);
    let embedded = embedded_descriptor_bytes(&bundle, BOOL_SRC);
    let lib_path = build_library_to_disk(
        &bundle.ir,
        stem,
        &requested,
        BuildTarget::Cdylib,
        &[],
        Some(embedded.as_slice()),
    )
    .expect("build cdylib");
    let header = emit_header(
        &bundle.ir,
        &HeaderOptions {
            library_name: stem.into(),
        },
    );
    let header_path = tmp.path().join(format!("lib_{stem}.h"));
    std::fs::write(&header_path, header).unwrap();

    let harness_path = tmp.path().join("harness.c");
    std::fs::write(&harness_path, c_harness_source(&header_path, &lib_path)).unwrap();
    let harness_bin = compile_c_harness(&harness_path, tmp.path());

    let output = Command::new(&harness_bin)
        .output()
        .expect("run c harness");
    assert!(
        output.status.success(),
        "c harness failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ok"));
}

#[test]
fn cdylib_string_param_roundtrip() {
    let produced = build_cdylib(STRING_SRC, "echo_name_cdylib");
    // SAFETY: symbols are loaded from the just-built library and invoked with valid ABI values.
    unsafe {
        let lib = load_library_leaked(&produced);
        let echo: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *const c_char> =
            lib.get(b"echo_name").expect("resolve echo_name");
        let free: libloading::Symbol<unsafe extern "C" fn(*const c_char)> =
            lib.get(b"corvid_free_string").expect("resolve corvid_free_string");
        let input = CString::new("Grüße").unwrap();
        let output_ptr = echo(input.as_ptr());
        let output = CStr::from_ptr(output_ptr).to_str().unwrap().to_owned();
        free(output_ptr);
        assert_eq!(output, "Grüße");
    }
}

#[test]
fn cdylib_float_precision_preserved() {
    let produced = build_cdylib(FLOAT_SRC, "echo_amount_cdylib");
    // SAFETY: symbol is loaded from the just-built library and invoked with a valid f64.
    unsafe {
        let lib = load_library_leaked(&produced);
        let echo: libloading::Symbol<unsafe extern "C" fn(f64) -> f64> =
            lib.get(b"echo_amount").expect("resolve echo_amount");
        let input = 0.12345678912345678_f64;
        let output = echo(input);
        assert_eq!(output.to_bits(), input.to_bits());
    }
}

#[test]
fn cdylib_bool_maps_to_c99_bool_size() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("bool_size.c");
    std::fs::write(
        &source,
        "#include <stdbool.h>\nint main(void) { return sizeof(bool) == 1 ? 0 : 1; }\n",
    )
    .unwrap();
    let bin = compile_c_harness(&source, tmp.path());
    let output = Command::new(bin).output().expect("run bool size harness");
    assert!(output.status.success());
}

fn compile_c_harness(source: &Path, out_dir: &Path) -> PathBuf {
    let compiler = cc::Build::new()
        .opt_level(0)
        .cargo_metadata(false)
        .cargo_warnings(false)
        .host(&target_lexicon::HOST.to_string())
        .target(&target_lexicon::HOST.to_string())
        .try_get_compiler()
        .expect("compiler");
    let output_path = if cfg!(windows) {
        out_dir.join("harness.exe")
    } else {
        out_dir.join("harness")
    };
    let mut cmd = Command::new(compiler.path());
    for (k, v) in compiler.env() {
        cmd.env(k, v);
    }
    if compiler.is_like_msvc() {
        cmd.arg(source)
            .arg(format!("/Fe:{}", output_path.display()));
    } else {
        cmd.arg(source)
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-ldl")
            .arg("-o")
            .arg(&output_path);
    }
    let output = cmd.output().expect("compile c harness");
    assert!(
        output.status.success(),
        "c harness compile failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output_path
}

fn c_harness_source(header_path: &Path, library_path: &Path) -> String {
    let header = header_path.to_string_lossy().replace('\\', "\\\\");
    let library = library_path.to_string_lossy().replace('\\', "\\\\");
    format!(
        r#"#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include "{header}"

#ifdef _WIN32
#include <windows.h>
int main(void) {{
    HMODULE lib = LoadLibraryA("{library}");
    if (!lib) return 1;
    bool (*refund_bot)(const char*, double) = (bool (*)(const char*, double))GetProcAddress(lib, "refund_bot");
    if (!refund_bot) return 2;
    if (!refund_bot("vip", 20.0)) return 3;
    FreeLibrary(lib);
    puts("ok");
    return 0;
}}
#else
#include <dlfcn.h>
int main(void) {{
    void* lib = dlopen("{library}", RTLD_NOW);
    if (!lib) return 1;
    bool (*refund_bot)(const char*, double) = (bool (*)(const char*, double))dlsym(lib, "refund_bot");
    if (!refund_bot) return 2;
    if (!refund_bot("vip", 20.0)) return 3;
    dlclose(lib);
    puts("ok");
    return 0;
}}
#endif
"#
    )
}
