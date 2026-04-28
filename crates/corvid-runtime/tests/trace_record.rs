use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;

use corvid_abi::{descriptor_to_embedded_bytes, emit_catalog_abi, EmitOptions};
use corvid_codegen_cl::{build_library_to_disk, BuildTarget};
use corvid_resolve::resolve;
use corvid_runtime::{CorvidApprovalRequired, CorvidCallStatus};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use corvid_types::{typecheck, EffectRegistry};
use libloading::Library;
use tempfile::TempDir;

const RECORD_SRC: &str = r#"
prompt classify_prompt(text: String) -> String:
    """Classify the sentiment of {text}. Reply with positive, negative, or neutral."""

@budget($0.25)
pub extern "c"
agent classify(text: String) -> String:
    return classify_prompt(text)
"#;

const APPROVAL_SRC: &str = r#"
tool echo_string(value: String) -> String dangerous

pub extern "c"
agent maybe_dangerous(flag: Bool, value: String) -> String:
    if flag:
        approve EchoString(value)
        return echo_string(value)
    return "skipped"
"#;

struct BuiltLibrary {
    _temp: TempDir,
    path: PathBuf,
}

fn build_record_library() -> BuiltLibrary {
    build_library_from_source(RECORD_SRC, "tests/trace_record/classify.cor", &[])
}

fn test_tools_lib_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.ancestors().nth(2).expect("workspace root").to_path_buf();
    let name = if cfg!(windows) {
        "corvid_test_tools.lib"
    } else {
        "libcorvid_test_tools.a"
    };
    let path = workspace_root.join("target").join("release").join(name);
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&workspace_root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    // Route the linker through `corvid_test_tools.lib` (which already
    // bundles `corvid-runtime` transitively) instead of pairing it
    // with the standalone `corvid_runtime.lib`. See
    // `corvid-codegen-cl::cdylib::runtime_staticlib_path`.
    unsafe {
        std::env::set_var("CORVID_RUNTIME_STATICLIB_OVERRIDE", &path);
    }
    path
}

fn build_approval_library() -> BuiltLibrary {
    let tools_lib = test_tools_lib_path();
    build_library_from_source(
        APPROVAL_SRC,
        "tests/trace_record/approval.cor",
        &[tools_lib.as_path()],
    )
}

fn build_library_from_source(
    source: &str,
    source_path: &str,
    extra_libs: &[&std::path::Path],
) -> BuiltLibrary {
    let tokens = lex(source).expect("lex");
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
    let registry = EffectRegistry::from_decls(&effect_decls);
    let ir = corvid_ir::lower(&file, &resolved, &checked);
    let descriptor = emit_catalog_abi(
        &file,
        &resolved,
        &checked,
        &ir,
        &registry,
        &EmitOptions {
            source_path,
            source_text: source,
            compiler_version: "0.6.0-phase22",
            generated_at: "1970-01-01T00:00:00Z",
        },
    );
    let embedded = descriptor_to_embedded_bytes(&descriptor).expect("embed descriptor");

    let tmp = tempfile::tempdir().expect("tempdir");
    let requested = tmp.path().join("record_demo");
    let path = build_library_to_disk(
        &ir,
        "record_demo",
        &requested,
        BuildTarget::Cdylib,
        extra_libs,
        Some(embedded.as_slice()),
    )
    .expect("build cdylib");

    BuiltLibrary { _temp: tmp, path }
}

#[test]
fn prompt_call_agent_records_trace_events_for_embedded_cdylib() {
    let built = build_record_library();
    let trace_dir = tempfile::tempdir().expect("trace tempdir");
    let trace_path = trace_dir.path().join("record.jsonl");

    unsafe {
        std::env::set_var("CORVID_MODEL", "mock-1");
        std::env::set_var("CORVID_TEST_MOCK_LLM", "1");
        std::env::set_var(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            "{\"classify_prompt\":\"positive\"}",
        );
        std::env::set_var("CORVID_TRACE_PATH", &trace_path);

        let lib = Library::new(&built.path).expect("load library");
        let call_agent: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                usize,
                *mut *mut c_char,
                *mut usize,
                *mut u64,
                *mut CorvidApprovalRequired,
            ) -> CorvidCallStatus,
        > = lib.get(b"corvid_call_agent").expect("resolve corvid_call_agent");
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            lib.get(b"corvid_free_result").expect("resolve corvid_free_result");

        let agent = CString::new("classify").unwrap();
        let args = CString::new("[\"great service\"]").unwrap();
        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::Ok);
        assert!(result_len > 0);
        assert_ne!(observation, 0);
        let json = CStr::from_ptr(result).to_str().expect("utf8 result");
        assert_eq!(json, "\"positive\"");
        free_result(result);

        let events = read_events_from_path(&trace_path).expect("read trace");
        validate_supported_schema(&events).expect("validate trace schema");
        assert!(
            matches!(events.first(), Some(TraceEvent::SchemaHeader { .. })),
            "trace should start with SchemaHeader, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TraceEvent::RunStarted { .. })),
            "trace should contain RunStarted, got {events:?}"
        );

        std::mem::forget(lib);
    }
}

#[test]
fn prompt_call_agent_replays_recorded_trace_on_windows() {
    let built = build_record_library();
    let trace_dir = tempfile::tempdir().expect("trace tempdir");
    let record_path = trace_dir.path().join("record.jsonl");

    unsafe {
        std::env::set_var("CORVID_MODEL", "mock-1");
        std::env::set_var("CORVID_TEST_MOCK_LLM", "1");
        std::env::set_var(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            "{\"classify_prompt\":\"positive\"}",
        );
        std::env::set_var("CORVID_TRACE_PATH", &record_path);
        std::env::remove_var("CORVID_TRACE_DISABLE");
        std::env::remove_var("CORVID_REPLAY_TRACE_PATH");

        let lib = Library::new(&built.path).expect("load library");
        let call_agent: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                usize,
                *mut *mut c_char,
                *mut usize,
                *mut u64,
                *mut CorvidApprovalRequired,
            ) -> CorvidCallStatus,
        > = lib.get(b"corvid_call_agent").expect("resolve corvid_call_agent");
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            lib.get(b"corvid_free_result").expect("resolve corvid_free_result");

        let agent = CString::new("classify").unwrap();
        let args = CString::new("[\"great service\"]").unwrap();
        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::Ok);
        assert_eq!(CStr::from_ptr(result).to_str().unwrap(), "\"positive\"");
        assert_ne!(observation, 0);
        free_result(result);

        std::env::set_var("CORVID_REPLAY_TRACE_PATH", &record_path);
        std::env::set_var("CORVID_TRACE_DISABLE", "1");
        std::env::set_var("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"negative\"}");

        let mut replay_result = std::ptr::null_mut();
        let mut replay_result_len = 0usize;
        let mut replay_observation = 0u64;
        let mut replay_approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let replay_status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut replay_result,
            &mut replay_result_len,
            &mut replay_observation,
            &mut replay_approval,
        );
        assert_eq!(replay_status, CorvidCallStatus::Ok);
        assert_eq!(
            CStr::from_ptr(replay_result).to_str().unwrap(),
            "\"positive\""
        );
        assert_ne!(replay_observation, 0);
        free_result(replay_result);

        std::mem::forget(lib);
    }
}

#[test]
fn direct_exported_symbol_accepts_explicit_observation_pointer() {
    let built = build_record_library();

    unsafe {
        std::env::set_var("CORVID_MODEL", "mock-1");
        std::env::set_var("CORVID_TEST_MOCK_LLM", "1");
        std::env::set_var(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            "{\"classify_prompt\":\"positive\"}",
        );

        let lib = Library::new(&built.path).expect("load library");
        let classify: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *mut u64) -> *const c_char,
        > = lib.get(b"classify").expect("resolve classify");
        let free_string: libloading::Symbol<unsafe extern "C" fn(*const c_char)> =
            lib.get(b"corvid_free_string").expect("resolve corvid_free_string");

        let arg = CString::new("great service").unwrap();
        let mut observation = 0u64;
        let output = classify(arg.as_ptr(), &mut observation as *mut u64);
        let output_text = CStr::from_ptr(output).to_str().expect("utf8 output");
        assert_eq!(output_text, "positive");
        assert_ne!(observation, 0);
        free_string(output);

        std::mem::forget(lib);
    }
}

#[test]
fn generic_call_agent_handles_approval_required_path_on_windows() {
    let built = build_approval_library();

    unsafe {
        let lib = Library::new(&built.path).expect("load library");
        let call_agent: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                usize,
                *mut *mut c_char,
                *mut usize,
                *mut u64,
                *mut CorvidApprovalRequired,
            ) -> CorvidCallStatus,
        > = lib.get(b"corvid_call_agent").expect("resolve corvid_call_agent");

        let agent = CString::new("maybe_dangerous").unwrap();
        let args = CString::new("[true,\"vip\"]").unwrap();
        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            std::ptr::null_mut(),
        );
        assert_eq!(status, CorvidCallStatus::ApprovalRequired);
        assert!(result.is_null());
        assert_eq!(result_len, 0);
        assert_eq!(observation, 0);

        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };

        let status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::ApprovalRequired);
        assert!(result.is_null());
        assert_eq!(result_len, 0);
        assert_eq!(observation, 0);
        assert_eq!(CStr::from_ptr(approval.site_name).to_str().unwrap(), "EchoString");

        std::mem::forget(lib);
    }
}

// ----------------------------------------------------------------
// JSON-tagged trace payload coverage. These tests prove that
// non-scalar approve/tool/prompt args round-trip through the
// codegen-cl `'j'` slot: the encoder builds a Corvid String holding
// JSON, the descriptor pointer lands in the trace slot, and
// `decode_trace_values` parses it back into the structured
// `serde_json::Value` the trace event records. End-to-end via
// the cdylib path (no static-staticlib std duplication).
// ----------------------------------------------------------------

const APPROVE_STRUCT_SRC: &str = r#"
type Refund:
    id: String
    amount: Int

tool issue_refund(r: Refund) -> Int dangerous

pub extern "c"
agent run_refund(threshold: Int) -> Int:
    r = Refund("r-001", 42)
    approve IssueRefund(r)
    return threshold + 1
"#;

const APPROVE_LIST_SRC: &str = r#"
tool publish_batch(ids: List<String>) -> Int dangerous

pub extern "c"
agent run_publish(threshold: Int) -> Int:
    ids = ["a", "b", "c"]
    approve PublishBatch(ids)
    return threshold + 1
"#;

const APPROVE_OPTION_SRC: &str = r#"
tool maybe_send(name: Option<String>) -> Int dangerous

agent maybe_str(flag: Bool) -> Option<String>:
    if flag:
        return Some("vip")
    return None

pub extern "c"
agent run_maybe(threshold: Int) -> Int:
    present = maybe_str(true)
    approve MaybeSend(present)
    absent = maybe_str(false)
    approve MaybeSend(absent)
    return threshold + 1
"#;

fn approval_request_args_for(events: &[TraceEvent], label: &str) -> Vec<Vec<serde_json::Value>> {
    events
        .iter()
        .filter_map(|event| match event {
            TraceEvent::ApprovalRequest {
                label: l, args, ..
            } if l == label => Some(args.clone()),
            _ => None,
        })
        .collect()
}

/// Always-accept approver callback. Bypasses the catalog-level
/// upfront gate so the agent body runs and the in-body `approve`
/// statement reaches `corvid_approve_sync`, which is what feeds the
/// non-scalar JSON encoding through `emit_trace_payload`.
unsafe extern "C" fn always_accept(
    _request: *const CorvidApprovalRequired,
    _user_data: *mut std::ffi::c_void,
) -> i32 {
    // CorvidApprovalDecision::Accept = 0. Documented in
    // `approver_bridge.rs`; mirror the value here so the test does
    // not depend on importing the enum across the cdylib boundary.
    0
}

/// Invoke the cdylib-exported agent through `corvid_call_agent`.
/// Registers an always-accept host approver before the call so the
/// catalog upfront gate passes and the agent body runs through the
/// in-body `approve` statement — that is the path that exercises
/// the JSON encoding under test.
fn run_agent_via_cdylib(built: &BuiltLibrary, agent_name: &str, args_json: &str) {
    unsafe {
        let lib = Library::new(&built.path).expect("load library");
        let register: libloading::Symbol<
            unsafe extern "C" fn(
                Option<
                    unsafe extern "C" fn(
                        *const CorvidApprovalRequired,
                        *mut std::ffi::c_void,
                    ) -> i32,
                >,
                *mut std::ffi::c_void,
            ),
        > = lib
            .get(b"corvid_register_approver")
            .expect("resolve corvid_register_approver");
        let clear: libloading::Symbol<unsafe extern "C" fn()> = lib
            .get(b"corvid_clear_approver")
            .expect("resolve corvid_clear_approver");
        let call_agent: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                usize,
                *mut *mut c_char,
                *mut usize,
                *mut u64,
                *mut CorvidApprovalRequired,
            ) -> CorvidCallStatus,
        > = lib.get(b"corvid_call_agent").expect("resolve corvid_call_agent");
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            lib.get(b"corvid_free_result").expect("resolve corvid_free_result");

        register(Some(always_accept), std::ptr::null_mut());

        let agent = CString::new(agent_name).unwrap();
        let args = CString::new(args_json).unwrap();
        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let status = call_agent(
            agent.as_ptr(),
            args.as_ptr(),
            args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        clear();
        assert_eq!(
            status,
            CorvidCallStatus::Ok,
            "agent `{agent_name}` did not return Ok after host accept"
        );
        if !result.is_null() {
            free_result(result);
        }
        std::mem::forget(lib);
    }
}

#[test]
fn approve_with_struct_arg_records_struct_as_json() {
    let built = build_library_from_source(
        APPROVE_STRUCT_SRC,
        "tests/trace_record/approve_struct.cor",
        &[],
    );
    let trace_dir = tempfile::tempdir().expect("trace tempdir");
    let trace_path = trace_dir.path().join("approve_struct.jsonl");
    unsafe {
        std::env::set_var("CORVID_TRACE_PATH", &trace_path);
        std::env::set_var("CORVID_APPROVE_AUTO", "1");
    }
    run_agent_via_cdylib(&built, "run_refund", "[5]");
    let events = read_events_from_path(&trace_path).expect("read trace");
    validate_supported_schema(&events).expect("validate trace");
    let approvals = approval_request_args_for(&events, "IssueRefund");
    assert_eq!(approvals.len(), 1, "expected one IssueRefund approval, got {approvals:?}");
    assert_eq!(
        approvals[0],
        vec![serde_json::json!({"id": "r-001", "amount": 42})],
        "struct arg should round-trip through 'j' slot as JSON object"
    );
}

#[test]
fn approve_with_list_arg_records_list_as_json() {
    let built = build_library_from_source(
        APPROVE_LIST_SRC,
        "tests/trace_record/approve_list.cor",
        &[],
    );
    let trace_dir = tempfile::tempdir().expect("trace tempdir");
    let trace_path = trace_dir.path().join("approve_list.jsonl");
    unsafe {
        std::env::set_var("CORVID_TRACE_PATH", &trace_path);
        std::env::set_var("CORVID_APPROVE_AUTO", "1");
    }
    run_agent_via_cdylib(&built, "run_publish", "[1]");
    let events = read_events_from_path(&trace_path).expect("read trace");
    validate_supported_schema(&events).expect("validate trace");
    let approvals = approval_request_args_for(&events, "PublishBatch");
    assert_eq!(approvals.len(), 1, "expected one PublishBatch approval");
    assert_eq!(
        approvals[0],
        vec![serde_json::json!(["a", "b", "c"])],
        "list arg should round-trip through 'j' slot as JSON array"
    );
}

#[test]
fn approve_with_option_arg_records_some_and_none_distinctly() {
    let built = build_library_from_source(
        APPROVE_OPTION_SRC,
        "tests/trace_record/approve_option.cor",
        &[],
    );
    let trace_dir = tempfile::tempdir().expect("trace tempdir");
    let trace_path = trace_dir.path().join("approve_option.jsonl");
    unsafe {
        std::env::set_var("CORVID_TRACE_PATH", &trace_path);
        std::env::set_var("CORVID_APPROVE_AUTO", "1");
    }
    run_agent_via_cdylib(&built, "run_maybe", "[1]");
    let events = read_events_from_path(&trace_path).expect("read trace");
    validate_supported_schema(&events).expect("validate trace");
    let approvals = approval_request_args_for(&events, "MaybeSend");
    assert_eq!(approvals.len(), 2, "expected two MaybeSend approvals (Some + None)");
    assert_eq!(
        approvals[0],
        vec![serde_json::json!("vip")],
        "Some(\"vip\") should record as JSON string"
    );
    assert_eq!(
        approvals[1],
        vec![serde_json::json!(null)],
        "None should record as JSON null"
    );
}
