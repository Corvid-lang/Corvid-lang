//! Slice 13b smoke test — verifies the corvid-runtime staticlib's
//! C-ABI surface is reachable from a linked C program.
//!
//! Writes a hand-rolled C program that calls `corvid_runtime_init()`,
//! `corvid_runtime_probe()`, and `corvid_runtime_shutdown()` — the
//! three bridge functions Phase 13 exposes. Compiles + links it the
//! same way `link.rs` compiles Corvid programs, so any linker error
//! in the real code path surfaces here first.
//!
//! If this test fails, slice 13b's linkage is broken and no compiled
//! Corvid binary with tool/prompt support can run. The rest of
//! Phase 13 is blocked until this is green.
//!
//! Not part of the parity harness because it has no Corvid source —
//! it's a pure FFI contract test. Lives alongside parity.rs because
//! both verify end-to-end compilation + linking + execution, just
//! from different entry points.

use std::path::PathBuf;
use std::process::Command;

/// Source of the smoke-test C program. Inlined so the test is
/// self-contained — no fixtures directory, no risk of copy-paste drift.
///
/// Exercises every bridge function the slice 13b / 13c surface exposes:
///   - `corvid_runtime_probe`                 (pure, no runtime needed)
///   - `corvid_runtime_init`                  (eager init)
///   - `corvid_tool_call_sync_int`            (async dispatch via block_on)
///   - `corvid_runtime_shutdown`              (clean teardown)
///
/// The mock tool `smoke_answer` is registered via the
/// `CORVID_TEST_MOCK_INT_TOOLS` env var the harness sets before
/// spawning the binary. Env-var-based registration is the Phase 13
/// test pattern (see `ffi_bridge.rs::build_corvid_runtime`) — user-
/// facing tool registration is Phase 14's proc-macro registry.
const FFI_SMOKE_C: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Declarations of the corvid-runtime bridge surface. Match the
 * `#[no_mangle] pub extern "C"` signatures in
 * `crates/corvid-runtime/src/ffi_bridge.rs`. If either drifts, the
 * linker catches it — these declarations are the contract. */
extern int corvid_runtime_init(void);
extern long long corvid_runtime_probe(void);
extern long long corvid_tool_call_sync_int(const char* name_ptr, size_t name_len);
extern void corvid_runtime_shutdown(void);

/* i64::MIN in decimal — the error sentinel the bridge uses for
 * tool-not-found / non-integer-return / tool-errored. Hard-coded here
 * because C has no portable way to write INT64_MIN as a literal
 * without the LL suffix + limits.h. */
#define CORVID_TOOL_CALL_ERR (-9223372036854775807LL - 1)

int main(void) {
    long long v = corvid_runtime_probe();
    if (v != 42) {
        fprintf(stderr, "probe returned %lld, expected 42\n", v);
        return 1;
    }

    int rc = corvid_runtime_init();
    if (rc != 0) {
        fprintf(stderr, "corvid_runtime_init returned %d, expected 0\n", rc);
        return 2;
    }

    v = corvid_runtime_probe();
    if (v != 42) {
        fprintf(stderr, "probe after init returned %lld, expected 42\n", v);
        return 3;
    }

    /* Call the mock tool registered via env var. The harness set
     * `CORVID_TEST_MOCK_INT_TOOLS=smoke_answer:42` before exec; the
     * runtime parsed it during `corvid_runtime_init` and registered
     * a handler that returns 42. */
    const char* tool_name = "smoke_answer";
    long long tool_result = corvid_tool_call_sync_int(tool_name, strlen(tool_name));
    if (tool_result != 42) {
        fprintf(stderr, "tool call returned %lld, expected 42\n", tool_result);
        return 5;
    }

    /* Call an unregistered name — the bridge should return the error
     * sentinel without crashing or hanging. */
    const char* missing = "no_such_tool";
    long long missing_result =
        corvid_tool_call_sync_int(missing, strlen(missing));
    if (missing_result != CORVID_TOOL_CALL_ERR) {
        fprintf(stderr,
                "unknown-tool call returned %lld, expected error sentinel\n",
                missing_result);
        return 6;
    }

    corvid_runtime_shutdown();

    v = corvid_runtime_probe();
    if (v != 42) {
        fprintf(stderr, "probe after shutdown returned %lld, expected 42\n", v);
        return 4;
    }

    corvid_runtime_shutdown();

    printf("ok\n");
    return 0;
}
"#;

/// The integration test: write the C program, compile + link it against
/// the corvid-runtime staticlib using the same cc-crate machinery `link.rs`
/// uses, run the binary, assert stdout == "ok" and exit code 0.
#[test]
fn ffi_bridge_init_probe_shutdown() {
    let tmp = tempfile::Builder::new()
        .prefix("corvid-ffi-smoke-")
        .tempdir()
        .expect("tempdir");
    let c_path = tmp.path().join("ffi_smoke.c");
    std::fs::write(&c_path, FFI_SMOKE_C).expect("write c source");

    // Locate the staticlib — same path link.rs computes at runtime.
    let staticlib_dir =
        std::path::Path::new(env!("CORVID_STATICLIB_DIR")).to_path_buf();
    let compiler = cc::Build::new()
        .opt_level(2)
        .cargo_metadata(false)
        .cargo_warnings(false)
        .host(&target_lexicon::HOST.to_string())
        .target(&target_lexicon::HOST.to_string())
        .try_get_compiler()
        .expect("compiler discovery");

    let out_bin = if cfg!(windows) {
        tmp.path().join("ffi_smoke.exe")
    } else {
        tmp.path().join("ffi_smoke")
    };

    let mut cmd = Command::new(compiler.path());
    for (k, v) in compiler.env() {
        cmd.env(k, v);
    }

    let staticlib_path = build_staticlib_path(&staticlib_dir, compiler.is_like_msvc());
    assert!(
        staticlib_path.exists(),
        "staticlib missing at `{}`; run `cargo build -p corvid-runtime --release` first",
        staticlib_path.display()
    );

    if compiler.is_like_msvc() {
        cmd.arg("/std:c11")
            .arg(format!(
                "/Fo{}{}",
                tmp.path().display(),
                std::path::MAIN_SEPARATOR
            ))
            .arg(&c_path)
            .arg(format!("/Fe:{}", out_bin.display()))
            .arg(&staticlib_path)
            .arg("/link")
            .arg("bcrypt.lib")
            .arg("advapi32.lib")
            .arg("kernel32.lib")
            .arg("ntdll.lib")
            .arg("userenv.lib")
            .arg("ws2_32.lib")
            .arg("dbghelp.lib")
            .arg("legacy_stdio_definitions.lib");
    } else {
        cmd.arg("-std=c11")
            .arg(&c_path)
            .arg(&staticlib_path)
            .arg("-lpthread")
            .arg("-ldl")
            .arg("-lm")
            .arg("-o")
            .arg(&out_bin);
        if cfg!(target_os = "macos") {
            cmd.arg("-framework").arg("Security");
            cmd.arg("-framework").arg("CoreFoundation");
            cmd.arg("-framework").arg("SystemConfiguration");
        } else if cfg!(target_os = "linux") {
            cmd.arg("-lutil");
        }
    }

    let link_out = cmd.output().expect("spawn compiler");
    assert!(
        link_out.status.success(),
        "link failed: stdout={} stderr={}",
        String::from_utf8_lossy(&link_out.stdout),
        String::from_utf8_lossy(&link_out.stderr),
    );

    // Register the mock tool the C program will call via the env-var
    // hook the runtime reads during corvid_runtime_init. `smoke_answer`
    // is defined to return 42 — any other value means the registration
    // path is broken. See ffi_bridge.rs::build_corvid_runtime.
    let run_out = Command::new(&out_bin)
        .env("CORVID_TEST_MOCK_INT_TOOLS", "smoke_answer:42")
        // Explicit approver: we don't want the binary asking the test
        // runner for stdin approval if anything trips the approve path.
        .env("CORVID_APPROVE_AUTO", "1")
        .output()
        .expect("run bin");
    let stdout = String::from_utf8_lossy(&run_out.stdout);
    let stderr = String::from_utf8_lossy(&run_out.stderr);
    assert!(
        run_out.status.success(),
        "smoke binary exited non-zero: status={:?} stdout={stdout} stderr={stderr}",
        run_out.status.code()
    );
    assert_eq!(
        stdout.trim(),
        "ok",
        "stdout mismatch: got {stdout} stderr {stderr}"
    );
}

fn build_staticlib_path(dir: &std::path::Path, msvc: bool) -> PathBuf {
    if msvc {
        dir.join("corvid_runtime.lib")
    } else {
        dir.join("libcorvid_runtime.a")
    }
}
