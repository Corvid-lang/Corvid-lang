use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

mod api_keys;
mod approvals;
mod audit;
mod oauth;
mod records;
mod sessions;
pub use api_keys::{hash_api_key_secret, verify_api_key_secret};
pub use approvals::{authorize_trace_permission, validate_jwt_verification_contract};
pub use oauth::hash_oauth_state;
pub use records::*;
pub use sessions::hash_session_secret;
use sessions::read_actor_row;

pub struct SessionAuthRuntime {
    conn: Mutex<Connection>,
}

impl SessionAuthRuntime {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let conn = Connection::open(path.as_ref())
            .map_err(|err| RuntimeError::Other(format!("failed to open auth db: {err}")))?;
        let runtime = Self {
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        Ok(runtime)
    }

    pub fn open_in_memory() -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory()
            .map_err(|err| RuntimeError::Other(format!("failed to open auth db: {err}")))?;
        let runtime = Self {
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        Ok(runtime)
    }

    pub fn upsert_actor(&self, actor: AuthActor) -> Result<AuthActor, RuntimeError> {
        validate_non_empty("actor id", &actor.id)?;
        validate_non_empty("tenant id", &actor.tenant_id)?;
        validate_non_empty("actor kind", &actor.actor_kind)?;
        validate_non_empty("auth method", &actor.auth_method)?;
        let now = now_ms();
        let created_ms = if actor.created_ms == 0 {
            now
        } else {
            actor.created_ms
        };
        let updated_ms = if actor.updated_ms == 0 {
            now
        } else {
            actor.updated_ms
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_actors
                 (id, tenant_id, display_name, actor_kind, auth_method, assurance_level, role_fingerprint, permission_fingerprint, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 on conflict(id) do update set
                   tenant_id = excluded.tenant_id,
                   display_name = excluded.display_name,
                   actor_kind = excluded.actor_kind,
                   auth_method = excluded.auth_method,
                   assurance_level = excluded.assurance_level,
                   role_fingerprint = excluded.role_fingerprint,
                   permission_fingerprint = excluded.permission_fingerprint,
                   updated_ms = excluded.updated_ms",
                params![
                    actor.id,
                    actor.tenant_id,
                    actor.display_name,
                    actor.actor_kind,
                    actor.auth_method,
                    actor.assurance_level,
                    actor.role_fingerprint,
                    actor.permission_fingerprint,
                    created_ms as i64,
                    updated_ms as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to upsert actor: {err}")))?;
        self.get_actor(&actor.id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth actor `{}` not found", actor.id)))
    }

    pub fn get_actor(&self, id: &str) -> Result<Option<AuthActor>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, tenant_id, display_name, actor_kind, auth_method, assurance_level, role_fingerprint, permission_fingerprint, created_ms, updated_ms
                 from auth_actors where id = ?1",
                params![id],
                read_actor_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read actor: {err}")))
    }

    fn init(&self) -> Result<(), RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .execute_batch(
                "create table if not exists auth_actors (
                    id text primary key,
                    tenant_id text not null,
                    display_name text not null,
                    actor_kind text not null,
                    auth_method text not null,
                    assurance_level text not null,
                    role_fingerprint text not null,
                    permission_fingerprint text not null,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists auth_actors_tenant on auth_actors(tenant_id);
                create table if not exists auth_sessions (
                    id text primary key,
                    actor_id text not null,
                    tenant_id text not null,
                    token_hash text not null unique,
                    issued_ms integer not null,
                    expires_ms integer not null,
                    rotation_counter integer not null,
                    csrf_binding_id text not null,
                    revoked_ms integer,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists auth_sessions_actor on auth_sessions(actor_id);
                create index if not exists auth_sessions_tenant on auth_sessions(tenant_id);
                create table if not exists auth_api_keys (
                    id text primary key,
                    service_actor_id text not null,
                    tenant_id text not null,
                    key_hash text not null,
                    scope_fingerprint text not null,
                    expires_ms integer not null,
                    last_used_ms integer,
                    revoked_ms integer,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists auth_api_keys_tenant on auth_api_keys(tenant_id);
                create index if not exists auth_api_keys_actor on auth_api_keys(service_actor_id);
                create table if not exists auth_oauth_states (
                    id text primary key,
                    provider text not null,
                    tenant_id text not null,
                    actor_id text not null,
                    state_hash text not null unique,
                    pkce_verifier_ref text not null,
                    nonce_fingerprint text not null,
                    expires_ms integer not null,
                    replay_key text not null,
                    used_ms integer,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists auth_oauth_states_tenant on auth_oauth_states(tenant_id);
                create index if not exists auth_oauth_states_actor on auth_oauth_states(actor_id);
                create table if not exists auth_audit_events (
                    id text primary key,
                    event_kind text not null,
                    actor_id text,
                    tenant_id text,
                    session_id text,
                    api_key_id text,
                    trace_id text,
                    status text not null,
                    reason text not null,
                    created_ms integer not null
                );
                create index if not exists auth_audit_tenant on auth_audit_events(tenant_id);
                create index if not exists auth_audit_session on auth_audit_events(session_id);",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to initialize auth schema: {err}")))
    }
}

pub(super) fn validate_non_empty(label: &str, value: &str) -> Result<(), RuntimeError> {
    if value.trim().is_empty() {
        Err(RuntimeError::Other(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str, tenant_id: &str) -> AuthActor {
        AuthActor {
            id: id.to_string(),
            tenant_id: tenant_id.to_string(),
            display_name: "Ada".to_string(),
            actor_kind: "user".to_string(),
            auth_method: "session".to_string(),
            assurance_level: "aal1".to_string(),
            role_fingerprint: "sha256:roles".to_string(),
            permission_fingerprint: "sha256:permissions".to_string(),
            created_ms: 1,
            updated_ms: 1,
        }
    }

    #[test]
    fn jwt_contract_validation_accepts_production_algorithms_and_redacts_failures() {
        let contract = JwtVerificationContract {
            issuer: "https://issuer.example".to_string(),
            audience: "corvid-api".to_string(),
            jwks_url: "https://issuer.example/.well-known/jwks.json".to_string(),
            algorithm: "RS256".to_string(),
            required_tenant_claim: "tenant".to_string(),
            required_subject_claim: "sub".to_string(),
            clock_skew_ms: 60_000,
        };
        let ok = validate_jwt_verification_contract(&contract);
        assert!(ok.valid);
        assert_eq!(ok.failure_kind, None);
        assert!(ok.redacted);

        for (algorithm, failure) in [
            ("none", "unsupported_algorithm"),
            ("HS256", "unsupported_algorithm"),
        ] {
            let mut bad = contract.clone();
            bad.algorithm = algorithm.to_string();
            let diagnostic = validate_jwt_verification_contract(&bad);
            assert!(!diagnostic.valid);
            assert_eq!(diagnostic.failure_kind.as_deref(), Some(failure));
            assert!(diagnostic.redacted);
        }

        let mut insecure = contract.clone();
        insecure.jwks_url = "http://issuer.example/jwks.json".to_string();
        let diagnostic = validate_jwt_verification_contract(&insecure);
        assert!(!diagnostic.valid);
        assert_eq!(
            diagnostic.failure_kind.as_deref(),
            Some("jwks_url_not_https")
        );
        assert!(diagnostic.redacted);
    }

    #[test]
    fn permission_propagation_binds_actor_tenant_trace_and_surface() {
        let actor = actor("user-1", "org-1");
        let trace = AuthTraceContext {
            trace_id: "trace-1".to_string(),
            tenant_id: "org-1".to_string(),
            actor_id: "user-1".to_string(),
            auth_method: "session".to_string(),
            session_id: "sess-1".to_string(),
            api_key_id: String::new(),
            permission_fingerprint: "sha256:permissions".to_string(),
            replay_key: "replay-1".to_string(),
        };
        let requirement = PermissionRequirement {
            tenant_id: "org-1".to_string(),
            permission: "CanReviewEmail".to_string(),
            permission_fingerprint: "sha256:permissions".to_string(),
            surface_kind: "job".to_string(),
            surface_id: "email_triage_job".to_string(),
            trace_id: "trace-1".to_string(),
        };
        let allowed = authorize_trace_permission(&actor, &trace, &requirement);
        assert!(allowed.allowed);
        assert_eq!(allowed.surface_kind, "job");
        assert_eq!(allowed.reason, "permission propagated");
        assert!(allowed.redacted);

        let mut cross_tenant = requirement.clone();
        cross_tenant.tenant_id = "org-2".to_string();
        let denied = authorize_trace_permission(&actor, &trace, &cross_tenant);
        assert!(!denied.allowed);
        assert!(denied.reason.contains("tenant"));

        let mut stale_trace = trace.clone();
        stale_trace.permission_fingerprint = "sha256:old".to_string();
        let denied = authorize_trace_permission(&actor, &stale_trace, &requirement);
        assert!(!denied.allowed);
        assert!(denied.reason.contains("trace permission"));
    }
}
