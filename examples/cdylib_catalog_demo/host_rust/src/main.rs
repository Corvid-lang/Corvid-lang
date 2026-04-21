use libloading::Library;
use std::env;
use std::ffi::{c_char, CString};

#[repr(C)]
#[derive(Clone, Copy)]
struct CorvidAgentHandle {
    name: *const c_char,
    symbol: *const c_char,
    source_file: *const c_char,
    source_line: u32,
    trust_tier: u8,
    cost_bound_usd: f64,
    reversible: u8,
    latency_instant: u8,
    replayable: u8,
    deterministic: u8,
    dangerous: u8,
    pub_extern_c: u8,
    requires_approval: u8,
    grounded_source_count: u32,
    param_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CorvidPreFlight {
    status: u32,
    cost_bound_usd: f64,
    requires_approval: u8,
    effect_row_json: *const c_char,
    grounded_source_set_json: *const c_char,
    bad_args_message: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CorvidApprovalRequired {
    site_name: *const c_char,
    predicate_json: *const c_char,
    args_json: *const c_char,
    rationale_prompt: *const c_char,
}

fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let library_path = args.next().expect("usage: host <library> <expected-hash-hex>");
    let hash_hex = args.next().expect("usage: host <library> <expected-hash-hex>");

    env::set_var("CORVID_MODEL", "mock-1");
    env::set_var("CORVID_TEST_MOCK_LLM", "1");
    env::set_var("CORVID_TEST_MOCK_LLM_REPLIES", r#"{"classify_prompt":"positive"}"#);

    let library = Box::leak(Box::new(unsafe { Library::new(&library_path)? }));
    unsafe {
        let verify: libloading::Symbol<unsafe extern "C" fn(*const u8) -> i32> =
            library.get(b"corvid_abi_verify")?;
        let list: libloading::Symbol<unsafe extern "C" fn(*mut CorvidAgentHandle, usize) -> usize> =
            library.get(b"corvid_list_agents")?;
        let preflight: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *const c_char, usize) -> CorvidPreFlight,
        > = library.get(b"corvid_pre_flight")?;
        let call: libloading::Symbol<
            unsafe extern "C" fn(
                *const c_char,
                *const c_char,
                usize,
                *mut *mut c_char,
                *mut usize,
                *mut CorvidApprovalRequired,
            ) -> u32,
        > = library.get(b"corvid_call_agent")?;
        let free_result: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> =
            library.get(b"corvid_free_result")?;

        let expected = hex::decode(hash_hex)?;
        println!("verified={}", verify(expected.as_ptr()));

        let count = list(std::ptr::null_mut(), 0);
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
            count
        ];
        list(handles.as_mut_ptr(), handles.len());
        println!("agent_count={count}");

        let name = CString::new("classify")?;
        let args_json = CString::new("[\"I loved the support experience\"]")?;
        let pf = preflight(name.as_ptr(), args_json.as_ptr(), args_json.as_bytes().len());
        println!(
            "preflight_status={} cost_bound_usd={:.2} requires_approval={}",
            pf.status, pf.cost_bound_usd, pf.requires_approval
        );

        let mut result = std::ptr::null_mut();
        let mut result_len = 0usize;
        let mut approval = CorvidApprovalRequired {
            site_name: std::ptr::null(),
            predicate_json: std::ptr::null(),
            args_json: std::ptr::null(),
            rationale_prompt: std::ptr::null(),
        };
        let status = call(
            name.as_ptr(),
            args_json.as_ptr(),
            args_json.as_bytes().len(),
            &mut result,
            &mut result_len,
            &mut approval,
        );
        let result_text = std::ffi::CStr::from_ptr(result).to_str()?.to_owned();
        println!("call_status={status} result={result_text}");
        free_result(result);
    }

    Ok(())
}
