#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthActor {
    pub id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub actor_kind: String,
    pub auth_method: String,
    pub assurance_level: String,
    pub role_fingerprint: String,
    pub permission_fingerprint: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub actor_id: String,
    pub tenant_id: String,
    pub token_hash: String,
    pub issued_ms: u64,
    pub expires_ms: u64,
    pub rotation_counter: u64,
    pub csrf_binding_id: String,
    pub revoked_ms: Option<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCreate {
    pub id: String,
    pub actor_id: String,
    pub tenant_id: String,
    pub raw_token: String,
    pub issued_ms: u64,
    pub expires_ms: u64,
    pub csrf_binding_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRecord {
    pub id: String,
    pub service_actor_id: String,
    pub tenant_id: String,
    pub key_hash: String,
    pub scope_fingerprint: String,
    pub expires_ms: u64,
    pub last_used_ms: Option<u64>,
    pub revoked_ms: Option<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyCreate {
    pub id: String,
    pub service_actor_id: String,
    pub tenant_id: String,
    pub raw_key: String,
    pub scope_fingerprint: String,
    pub expires_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthTraceContext {
    pub trace_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub auth_method: String,
    pub session_id: String,
    pub api_key_id: String,
    pub permission_fingerprint: String,
    pub replay_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionResolution {
    pub actor: AuthActor,
    pub session: SessionRecord,
    pub trace: AuthTraceContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyResolution {
    pub actor: AuthActor,
    pub api_key: ApiKeyRecord,
    pub trace: AuthTraceContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtVerificationContract {
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
    pub algorithm: String,
    pub required_tenant_claim: String,
    pub required_subject_claim: String,
    pub clock_skew_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtContractDiagnostic {
    pub valid: bool,
    pub failure_kind: Option<String>,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthStateCreate {
    pub id: String,
    pub provider: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub raw_state: String,
    pub pkce_verifier_ref: String,
    pub nonce_fingerprint: String,
    pub expires_ms: u64,
    pub replay_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthStateRecord {
    pub id: String,
    pub provider: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub state_hash: String,
    pub pkce_verifier_ref: String,
    pub nonce_fingerprint: String,
    pub expires_ms: u64,
    pub replay_key: String,
    pub used_ms: Option<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallbackResolution {
    pub actor: AuthActor,
    pub state: OAuthStateRecord,
    pub trace: AuthTraceContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequirement {
    pub tenant_id: String,
    pub permission: String,
    pub permission_fingerprint: String,
    pub surface_kind: String,
    pub surface_id: String,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    pub actor_id: String,
    pub tenant_id: String,
    pub permission: String,
    pub surface_kind: String,
    pub surface_id: String,
    pub trace_id: String,
    pub reason: String,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAuditEvent {
    pub id: String,
    pub event_kind: String,
    pub actor_id: Option<String>,
    pub tenant_id: Option<String>,
    pub session_id: Option<String>,
    pub api_key_id: Option<String>,
    pub trace_id: Option<String>,
    pub status: String,
    pub reason: String,
    pub created_ms: u64,
}
