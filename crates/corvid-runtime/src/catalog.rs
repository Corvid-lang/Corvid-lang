mod filter;
pub use filter::find_agents_where;
mod invoke;
use invoke::{
    build_invoker, cost_bound_for, finite_option, is_introspection_agent, CatalogInvoker,
};
pub use invoke::{call_agent, pre_flight};
mod descriptor;
pub use descriptor::{
    agent_signature_json, descriptor_hash, descriptor_json, descriptor_json_ptr, list_agents,
    verify_hash,
};
use descriptor::{catalog, AgentCatalogEntry};
pub(crate) use descriptor::{catalog_approval_sites, list_agent_handles_owned};
mod types;
pub use types::{
    CorvidAgentHandle, CorvidApprovalDecision, CorvidApprovalRequired, CorvidApproverFn,
    CorvidCallStatus, CorvidFindAgentsResult, CorvidPreFlight, CorvidPreFlightStatus,
    CorvidTrustTier, OwnedApprovalRequired, OwnedCallOutcome, OwnedFindAgentsOutcome,
    OwnedPreFlight,
};
pub(crate) use types::{ScalarAbiType, ScalarInvocation, ScalarInvoker, ScalarReturnType};
