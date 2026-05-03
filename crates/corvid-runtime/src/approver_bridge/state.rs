use super::*;

#[derive(Debug, Clone)]
pub(super) struct RegisteredApprover {
    pub(super) source_path: PathBuf,
    pub(super) abi: AbiAgent,
    pub(super) program: MiniApproverProgram,
    pub(super) display_budget_usd: f64,
    pub(super) signature_json: String,
    pub(super) name_c: CString,
    pub(super) symbol_c: CString,
    pub(super) source_file_c: CString,
    pub(super) signature_json_c: CString,
}

pub(crate) struct RegisteredApproverOverlay {
    pub abi: AbiAgent,
    pub display_budget_usd: f64,
    pub signature_json: String,
    pub name_ptr: *const c_char,
    pub symbol_ptr: *const c_char,
    pub source_file_ptr: *const c_char,
    pub signature_json_ptr: *const c_char,
    pub signature_json_len: usize,
}

fn state() -> &'static Mutex<Option<RegisteredApprover>> {
    static STATE: OnceLock<Mutex<Option<RegisteredApprover>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

pub fn register_approver_from_source(
    source_path: &Path,
    max_budget_usd_per_call: f64,
) -> Result<(), ApproverLoadError> {
    let mut compiled = compile_approver_source(source_path)?;
    validate_approver_safety(&compiled.abi, max_budget_usd_per_call)?;
    compiled.display_budget_usd = compiled
        .abi
        .budget
        .as_ref()
        .map(|budget| budget.usd_per_call)
        .or_else(|| {
            if max_budget_usd_per_call > 0.0 {
                Some(max_budget_usd_per_call)
            } else {
                None
            }
        })
        .unwrap_or(f64::NAN);
    *state().lock().unwrap() = Some(compiled);
    Ok(())
}

pub fn clear_registered_approver() {
    *state().lock().unwrap() = None;
}

pub(crate) fn registered_approver_overlay() -> Option<RegisteredApproverOverlay> {
    let guard = state().lock().unwrap();
    let registered = guard.as_ref()?;
    Some(RegisteredApproverOverlay {
        abi: registered.abi.clone(),
        display_budget_usd: registered.display_budget_usd,
        signature_json: registered.signature_json.clone(),
        name_ptr: registered.name_c.as_ptr(),
        symbol_ptr: registered.symbol_c.as_ptr(),
        source_file_ptr: registered.source_file_c.as_ptr(),
        signature_json_ptr: registered.signature_json_c.as_ptr(),
        signature_json_len: registered.signature_json_c.as_bytes().len(),
    })
}

pub fn evaluate_registered_approver(
    site: &ApprovalSiteInput,
    args: &[Value],
) -> Result<Option<ApprovalDecisionInfo>, String> {
    let approver = state().lock().unwrap().clone();
    let Some(approver) = approver else {
        return Ok(None);
    };
    let decision = approver.program.evaluate(site, args)?;
    Ok(Some(ApprovalDecisionInfo {
        accepted: decision.accepted,
        decider: format!(
            "corvid-agent:{}",
            approver.source_path.to_string_lossy().replace('\\', "/")
        ),
        rationale: decision.rationale,
    }))
}
