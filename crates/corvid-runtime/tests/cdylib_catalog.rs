use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::process::Command;

use corvid_abi::{
    descriptor_from_embedded_section, descriptor_to_embedded_bytes, emit_catalog_abi,
    read_embedded_section_from_library, CorvidAbi, EmitOptions,
};
use corvid_codegen_cl::{build_library_to_disk, BuildTarget};
use corvid_resolve::resolve;
use corvid_runtime::{
    CorvidAgentHandle, CorvidApprovalDecision, CorvidApprovalRequired, CorvidCallStatus,
    CorvidFindAgentsResult, CorvidFindAgentsStatus, CorvidPreFlightStatus,
};
use corvid_runtime::approver_bridge::{CorvidApproverLoadStatus, CorvidPredicateResult, CorvidPredicateStatus};
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck, EffectRegistry};
use libloading::Library;
use tempfile::TempDir;

const CATALOG_SRC: &str = r#"
tool echo_string(value: String) -> String dangerous

prompt classify_prompt(text: String) -> String:
    """Classify the sentiment of {text}. Reply with positive, negative, or neutral."""

@budget($0.25)
pub extern "c"
agent classify(text: String) -> String:
    return classify_prompt(text)

agent helper_wrap(values: List<String>) -> String:
    return "wrapped"

pub extern "c"
agent call_helper(text: String) -> String:
    return helper_wrap([text])

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
    expected_descriptor: CorvidAbi,
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
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&workspace_root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    path
}

fn build_catalog_library() -> BuiltLibrary {
    let tokens = lex(CATALOG_SRC).expect("lex");
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
            source_path: "examples/cdylib_catalog_demo/src/classify.cor",
            source_text: CATALOG_SRC,
            compiler_version: "0.6.0-phase22",
            generated_at: "1970-01-01T00:00:00Z",
        },
    );
    let embedded = descriptor_to_embedded_bytes(&descriptor).expect("embed descriptor");

    let tmp = tempfile::tempdir().expect("tempdir");
    let requested = tmp.path().join("catalog_demo");
    let path = build_library_to_disk(
        &ir,
        "catalog_demo",
        &requested,
        BuildTarget::Cdylib,
        &[test_tools_lib_path().as_path()],
        Some(embedded.as_slice()),
    )
    .expect("build cdylib");

    BuiltLibrary {
        _temp: tmp,
        path,
        expected_descriptor: descriptor,
    }
}

fn load_string(ptr: *const c_char) -> String {
    unsafe { CStr::from_ptr(ptr).to_str().expect("utf8").to_owned() }
}

unsafe extern "C" fn reject_approver(
    _request: *const CorvidApprovalRequired,
    _user_data: *mut std::ffi::c_void,
) -> CorvidApprovalDecision {
    CorvidApprovalDecision::Reject
}

unsafe extern "C" fn accept_approver(
    _request: *const CorvidApprovalRequired,
    _user_data: *mut std::ffi::c_void,
) -> CorvidApprovalDecision {
    CorvidApprovalDecision::Accept
}

fn write_approver_source(dir: &TempDir, source: &str) -> PathBuf {
    let path = dir.path().join("approver.cor");
    std::fs::write(&path, source).expect("write approver source");
    path
}

#[test]
fn embedded_section_roundtrips_from_built_library() {
    let built = build_catalog_library();
    let section = read_embedded_section_from_library(&built.path).expect("read embedded section");
    let decoded = descriptor_from_embedded_section(&section).expect("decode descriptor");
    assert_eq!(decoded, built.expected_descriptor);
}

#[test]
fn two_builds_of_same_source_produce_identical_embedded_descriptor_sections() {
    let left = read_embedded_section_from_library(&build_catalog_library().path)
        .expect("left section");
    let right = read_embedded_section_from_library(&build_catalog_library().path)
        .expect("right section");
    assert_eq!(left.json, right.json);
    assert_eq!(left.sha256, right.sha256);
}

#[test]
fn corvid_abi_verify_matches_and_rejects_one_bit_flip() {
    let built = build_catalog_library();
    unsafe {
        let lib = Library::new(&built.path).expect("load library");
        let verify: libloading::Symbol<unsafe extern "C" fn(*const u8) -> i32> =
            lib.get(b"corvid_abi_verify").expect("resolve corvid_abi_verify");
        let section = read_embedded_section_from_library(&built.path).expect("embedded");
        assert_eq!(verify(section.sha256.as_ptr()), 1);
        let mut flipped = section.sha256;
        flipped[0] ^= 0x01;
        assert_eq!(verify(flipped.as_ptr()), 0);
        std::mem::forget(lib);
    }
}

#[test]
fn corvid_list_agents_lists_declaration_order_and_introspection_entries() {
    let built = build_catalog_library();
    unsafe {
        let lib = Library::new(&built.path).expect("load library");
        let list: libloading::Symbol<
            unsafe extern "C" fn(*mut CorvidAgentHandle, usize) -> usize,
        > = lib.get(b"corvid_list_agents").expect("resolve corvid_list_agents");

        let total = list(std::ptr::null_mut(), 0);
        assert!(total >= 10, "expected user agents + introspection entries");

        let mut handles = vec![
            CorvidAgentHandle {
                name: std::ptr::null(),
                symbol: std::ptr::null(),
                source_file: std::ptr::null(),
                source_line: 0,
                trust_tier: 0,
                cost_bound_usd: 0.0,
                reversible: 0,
                latency_instant: 0,
                replayable: 0,
                deterministic: 0,
                dangerous: 0,
                pub_extern_c: 0,
                requires_approval: 0,
                grounded_source_count: 0,
                param_count: 0,
            };
            total
        ];
        let copied = list(handles.as_mut_ptr(), handles.len());
        assert_eq!(copied, total);

        let names = handles.iter().map(|handle| load_string(handle.name)).collect::<Vec<_>>();
        assert_eq!(names[0], "__corvid_abi_descriptor_json");
        assert_eq!(names[1], "__corvid_abi_verify");
        assert_eq!(names[2], "__corvid_list_agents");
        assert_eq!(names[3], "__corvid_find_agents_where");
        assert!(names.contains(&"classify".to_string()));
        let classify = handles
            .iter()
            .find(|handle| load_string(handle.name) == "classify")
            .expect("classify handle");
        assert_eq!(classify.source_line, 9);

        let list_agents = handles
            .iter()
            .find(|handle| load_string(handle.name) == "__corvid_list_agents")
            .expect("list-agents handle");
        assert_eq!(list_agents.trust_tier, 0);
        assert_eq!(list_agents.cost_bound_usd, 0.0);
        assert_eq!(list_agents.reversible, 1);
        std::mem::forget(lib);
    }
}

#[test]
fn corvid_pre_flight_validates_args_and_rejects_unsupported_sigs_without_dispatching() {
    let built = build_catalog_library();
    unsafe {
        std::env::remove_var("CORVID_TEST_MOCK_LLM");
        std::env::remove_var("CORVID_TEST_MOCK_LLM_REPLIES");
        std::env::set_var("CORVID_MODEL", "mock-1");

        let lib = Library::new(&built.path).expect("load library");
        let preflight: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *const c_char, usize) -> corvid_runtime::CorvidPreFlight,
        > = lib.get(b"corvid_pre_flight").expect("resolve corvid_pre_flight");

        let classify = CString::new("classify").unwrap();
        let ok_args = CString::new("[\"great service\"]").unwrap();
        let ok = preflight(classify.as_ptr(), ok_args.as_ptr(), ok_args.as_bytes().len());
        assert_eq!(ok.status, CorvidPreFlightStatus::Ok);
        assert_eq!(ok.requires_approval, 0);
        assert!(!ok.effect_row_json.is_null());

        let wrong_arity = CString::new("[]").unwrap();
        let arity = preflight(classify.as_ptr(), wrong_arity.as_ptr(), wrong_arity.as_bytes().len());
        assert_eq!(arity.status, CorvidPreFlightStatus::BadArgs);
        assert!(load_string(arity.bad_args_message).contains("arity mismatch"));

        let wrong_type = CString::new("[42]").unwrap();
        let bad_type = preflight(classify.as_ptr(), wrong_type.as_ptr(), wrong_type.as_bytes().len());
        assert_eq!(bad_type.status, CorvidPreFlightStatus::BadArgs);

        let helper = CString::new("helper_wrap").unwrap();
        let helper_args = CString::new("[[\"a\"]]").unwrap();
        let unsupported = preflight(helper.as_ptr(), helper_args.as_ptr(), helper_args.as_bytes().len());
        assert_eq!(unsupported.status, CorvidPreFlightStatus::UnsupportedSig);
        std::mem::forget(lib);
    }
}

#[test]
fn corvid_call_agent_handles_happy_path_bad_args_and_approval_flow() {
    let built = build_catalog_library();
    unsafe {
        std::env::set_var("CORVID_MODEL", "mock-1");
        std::env::set_var("CORVID_TEST_MOCK_LLM", "1");
        std::env::set_var("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}");

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
        let register: libloading::Symbol<
            unsafe extern "C" fn(
                Option<
                    unsafe extern "C" fn(
                        *const CorvidApprovalRequired,
                        *mut std::ffi::c_void,
                    ) -> CorvidApprovalDecision,
                >,
                *mut std::ffi::c_void,
            ),
        > = lib
            .get(b"corvid_register_approver")
            .expect("resolve corvid_register_approver");
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            lib.get(b"corvid_free_result").expect("resolve corvid_free_result");

        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };

        let classify = CString::new("classify").unwrap();
        let ok_args = CString::new("[\"great service\"]").unwrap();
        let status = call_agent(
            classify.as_ptr(),
            ok_args.as_ptr(),
            ok_args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::Ok);
        assert_eq!(result_len, "\"positive\"".len());
        assert_ne!(observation, 0);
        let result_json = CStr::from_ptr(result).to_str().unwrap().to_owned();
        free_result(result);
        assert_eq!(result_json, "\"positive\"");

        let bad_args = CString::new("[]").unwrap();
        let status = call_agent(
            classify.as_ptr(),
            bad_args.as_ptr(),
            bad_args.as_bytes().len(),
            &mut std::ptr::null_mut(),
            &mut result_len,
            std::ptr::null_mut(),
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::BadArgs);

        let dangerous = CString::new("maybe_dangerous").unwrap();
        let dangerous_args = CString::new("[true,\"vip\"]").unwrap();
        register(None, std::ptr::null_mut());
        let status = call_agent(
            dangerous.as_ptr(),
            dangerous_args.as_ptr(),
            dangerous_args.as_bytes().len(),
            &mut std::ptr::null_mut(),
            &mut result_len,
            std::ptr::null_mut(),
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::ApprovalRequired);
        assert_eq!(load_string(approval.site_name), "EchoString");

        register(Some(reject_approver), std::ptr::null_mut());
        let status = call_agent(
            dangerous.as_ptr(),
            dangerous_args.as_ptr(),
            dangerous_args.as_bytes().len(),
            &mut std::ptr::null_mut(),
            &mut result_len,
            std::ptr::null_mut(),
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::ApprovalRequired);

        register(Some(accept_approver), std::ptr::null_mut());
        let mut approved_result = std::ptr::null_mut();
        let status = call_agent(
            dangerous.as_ptr(),
            dangerous_args.as_ptr(),
            dangerous_args.as_bytes().len(),
            &mut approved_result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(status, CorvidCallStatus::Ok);
        assert_ne!(observation, 0);
        let approved_json = CStr::from_ptr(approved_result).to_str().unwrap().to_owned();
        free_result(approved_result);
        assert_eq!(approved_json, "\"vip\"");

        register(None, std::ptr::null_mut());
        std::mem::forget(lib);
    }
}

#[test]
fn corvid_find_agents_where_filters_in_live_catalog_order() {
    let built = build_catalog_library();
    unsafe {
        let lib = Library::new(&built.path).expect("load library");
        let list: libloading::Symbol<
            unsafe extern "C" fn(*mut CorvidAgentHandle, usize) -> usize,
        > = lib.get(b"corvid_list_agents").expect("resolve corvid_list_agents");
        let find: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, usize, *mut usize, usize) -> CorvidFindAgentsResult,
        > = lib
            .get(b"corvid_find_agents_where")
            .expect("resolve corvid_find_agents_where");

        let total = list(std::ptr::null_mut(), 0);
        let mut handles = vec![
            CorvidAgentHandle {
                name: std::ptr::null(),
                symbol: std::ptr::null(),
                source_file: std::ptr::null(),
                source_line: 0,
                trust_tier: 0,
                cost_bound_usd: 0.0,
                reversible: 0,
                latency_instant: 0,
                replayable: 0,
                deterministic: 0,
                dangerous: 0,
                pub_extern_c: 0,
                requires_approval: 0,
                grounded_source_count: 0,
                param_count: 0,
            };
            total
        ];
        list(handles.as_mut_ptr(), handles.len());

        let filter = CString::new(
            r#"{"all":[{"dim":"trust_tier","op":"le","value":"autonomous"},{"dim":"dangerous","op":"eq","value":false}]}"#,
        )
        .unwrap();
        let mut indices = vec![usize::MAX; total];
        let outcome = find(
            filter.as_ptr(),
            filter.as_bytes().len(),
            indices.as_mut_ptr(),
            indices.len(),
        );
        assert_eq!(outcome.status, CorvidFindAgentsStatus::Ok);
        assert!(outcome.matched_count > 0);
        let names = indices[..outcome.matched_count]
            .iter()
            .map(|idx| load_string(handles[*idx].name))
            .collect::<Vec<_>>();
        assert!(names.contains(&"classify".to_string()));
        assert!(!names.contains(&"maybe_dangerous".to_string()));

        let bad_filter = CString::new(r#"{"dim":"dangerous","op":"le","value":true}"#).unwrap();
        let bad = find(
            bad_filter.as_ptr(),
            bad_filter.as_bytes().len(),
            indices.as_mut_ptr(),
            indices.len(),
        );
        assert_eq!(bad.status, CorvidFindAgentsStatus::OpMismatch);
        assert!(!bad.error_message.is_null());

        let missing_filter = CString::new(r#"{"dim":"does_not_exist","op":"eq","value":true}"#).unwrap();
        let missing = find(
            missing_filter.as_ptr(),
            missing_filter.as_bytes().len(),
            indices.as_mut_ptr(),
            indices.len(),
        );
        assert_eq!(missing.status, CorvidFindAgentsStatus::UnknownDimension);
        assert!(!missing.error_message.is_null());
        std::mem::forget(lib);
    }
}

#[test]
fn corvid_source_approver_registration_and_predicate_eval_work() {
    let built = build_catalog_library();
    let approver_path = write_approver_source(
        &built._temp,
        r#"
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    if site.label == "EchoString":
        return ApprovalDecision(true, "approved")
    return ApprovalDecision(false, "rejected")
"#,
    );
    unsafe {
        std::env::set_var("CORVID_MODEL", "mock-1");
        std::env::set_var("CORVID_TEST_MOCK_LLM", "1");
        std::env::set_var("CORVID_TEST_MOCK_LLM_REPLIES", "{\"classify_prompt\":\"positive\"}");

        let lib = Library::new(&built.path).expect("load library");
        let register_source: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, f64, *mut *mut c_char) -> CorvidApproverLoadStatus,
        > = lib
            .get(b"corvid_register_approver_from_source")
            .expect("resolve corvid_register_approver_from_source");
        let clear_source: libloading::Symbol<unsafe extern "C" fn()> =
            lib.get(b"corvid_clear_approver").expect("resolve corvid_clear_approver");
        let predicate_json: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *mut usize) -> *const c_char,
        > = lib
            .get(b"corvid_approval_predicate_json")
            .expect("resolve corvid_approval_predicate_json");
        let predicate_eval: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *const c_char, usize) -> CorvidPredicateResult,
        > = lib
            .get(b"corvid_evaluate_approval_predicate")
            .expect("resolve corvid_evaluate_approval_predicate");
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
        let list_agents: libloading::Symbol<
            unsafe extern "C" fn(*mut CorvidAgentHandle, usize) -> usize,
        > = lib.get(b"corvid_list_agents").expect("resolve corvid_list_agents");
        let signature_json: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *mut usize) -> *const c_char,
        > = lib
            .get(b"corvid_agent_signature_json")
            .expect("resolve corvid_agent_signature_json");
        let verify: libloading::Symbol<unsafe extern "C" fn(*const u8) -> i32> =
            lib.get(b"corvid_abi_verify").expect("resolve corvid_abi_verify");
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            lib.get(b"corvid_free_result").expect("resolve corvid_free_result");

        let baseline_count = list_agents(std::ptr::null_mut(), 0);
        let approver_path_c = CString::new(approver_path.to_string_lossy().to_string()).unwrap();
        let mut error = std::ptr::null_mut();
        let status = register_source(approver_path_c.as_ptr(), 1.0, &mut error);
        assert_eq!(status, CorvidApproverLoadStatus::Ok);
        assert!(error.is_null());

        let with_approver_count = list_agents(std::ptr::null_mut(), 0);
        assert_eq!(with_approver_count, baseline_count + 1);
        let mut handles = vec![
            CorvidAgentHandle {
                name: std::ptr::null(),
                symbol: std::ptr::null(),
                source_file: std::ptr::null(),
                source_line: 0,
                trust_tier: 0,
                cost_bound_usd: 0.0,
                reversible: 0,
                latency_instant: 0,
                replayable: 0,
                deterministic: 0,
                dangerous: 0,
                pub_extern_c: 0,
                requires_approval: 0,
                grounded_source_count: 0,
                param_count: 0,
            };
            with_approver_count
        ];
        let copied = list_agents(handles.as_mut_ptr(), handles.len());
        assert_eq!(copied, with_approver_count);
        let overlay_index = handles
            .iter()
            .position(|handle| load_string(handle.name) == "__corvid_approver")
            .expect("overlay approver present");
        assert_eq!(overlay_index, 7);
        assert_eq!(load_string(handles[overlay_index].symbol), "__corvid_approver");
        assert_eq!(handles[overlay_index].trust_tier, 0);
        assert_eq!(handles[overlay_index].dangerous, 0);
        assert_eq!(handles[overlay_index].pub_extern_c, 0);
        assert_eq!(handles[overlay_index].requires_approval, 0);
        assert_eq!(handles[overlay_index].param_count, 3);

        let approver_name = CString::new("__corvid_approver").unwrap();
        let mut sig_len = 0usize;
        let sig_ptr = signature_json(approver_name.as_ptr(), &mut sig_len);
        assert!(!sig_ptr.is_null());
        let sig_text = CStr::from_ptr(sig_ptr).to_str().unwrap();
        let sig: corvid_abi::AbiAgent = serde_json::from_str(sig_text).expect("approver abi json");
        assert_eq!(sig.name, "__corvid_approver");
        assert_eq!(sig.symbol, "__corvid_approver");
        assert!(sig_len > 0);

        let section = read_embedded_section_from_library(&built.path).expect("embedded");
        assert_eq!(verify(section.sha256.as_ptr()), 1);

        let label = CString::new("EchoString").unwrap();
        let mut json_len = 0usize;
        let predicate = predicate_json(label.as_ptr(), &mut json_len);
        assert!(!predicate.is_null());
        let predicate_text = CStr::from_ptr(predicate).to_str().unwrap();
        assert!(predicate_text.contains("\"requires_approval\""));
        assert!(json_len > 0);

        let args = CString::new("[\"vip\"]").unwrap();
        let eval = predicate_eval(label.as_ptr(), args.as_ptr(), args.as_bytes().len());
        assert_eq!(eval.status, CorvidPredicateStatus::Ok);
        assert_eq!(eval.requires_approval, 1);

        let dangerous = CString::new("maybe_dangerous").unwrap();
        let dangerous_args = CString::new("[true,\"vip\"]").unwrap();
        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut observation = 0u64;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let call_status = call_agent(
            dangerous.as_ptr(),
            dangerous_args.as_ptr(),
            dangerous_args.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut observation,
            &mut approval,
        );
        assert_eq!(call_status, CorvidCallStatus::Ok);
        assert_ne!(observation, 0);
        let approved_json = CStr::from_ptr(result).to_str().unwrap().to_owned();
        free_result(result);
        assert_eq!(approved_json, "\"vip\"");

        let direct_status = call_agent(
            approver_name.as_ptr(),
            dangerous_args.as_ptr(),
            dangerous_args.as_bytes().len(),
            &mut std::ptr::null_mut(),
            &mut result_len,
            std::ptr::null_mut(),
            &mut approval,
        );
        assert_eq!(direct_status, CorvidCallStatus::UnsupportedSig);

        let replacement_path = write_approver_source(
            &built._temp,
            r#"
@replayable
@deterministic
@budget($0.25)
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    return ApprovalDecision(true, "replacement")
"#,
        );
        let replacement_c = CString::new(replacement_path.to_string_lossy().to_string()).unwrap();
        let status = register_source(replacement_c.as_ptr(), 1.0, &mut error);
        assert_eq!(status, CorvidApproverLoadStatus::Ok);
        let recounted = list_agents(std::ptr::null_mut(), 0);
        assert_eq!(recounted, baseline_count + 1);
        let mut replaced = vec![handles[0]; recounted];
        let _ = list_agents(replaced.as_mut_ptr(), replaced.len());
        let replaced_handle = replaced
            .iter()
            .find(|handle| load_string(handle.name) == "__corvid_approver")
            .expect("replaced approver");
        assert_eq!(replaced_handle.cost_bound_usd, 0.25);
        assert_eq!(replaced_handle.replayable, 1);
        assert_eq!(replaced_handle.deterministic, 1);

        clear_source();
        let cleared_count = list_agents(std::ptr::null_mut(), 0);
        assert_eq!(cleared_count, baseline_count);
        let null_sig = signature_json(approver_name.as_ptr(), &mut sig_len);
        assert!(null_sig.is_null());
        std::mem::forget(lib);
    }
}
