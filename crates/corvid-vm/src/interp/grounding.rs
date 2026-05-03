use crate::value::Value;
use corvid_ir::IrTool;

pub(super) fn tool_has_retrieval_effect(tool: &IrTool) -> bool {
    tool.effect_names.iter().any(|effect| effect == "retrieval")
}
pub(super) fn maybe_ground_tool_result(tool: &IrTool, callee_name: &str, value: Value) -> Value {
    if !tool_has_retrieval_effect(tool) {
        return value;
    }

    let chain = crate::ProvenanceChain::with_retrieval(callee_name, corvid_runtime::now_ms());
    Value::Grounded(crate::value::GroundedValue::new(value, chain))
}
