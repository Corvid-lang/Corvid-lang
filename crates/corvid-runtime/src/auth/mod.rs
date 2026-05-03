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

    fn service_actor(id: &str, tenant_id: &str) -> AuthActor {
        AuthActor {
            actor_kind: "service".to_string(),
            auth_method: "api_key".to_string(),
            display_name: "CI".to_string(),
            permission_fingerprint: "sha256:service-scopes".to_string(),
            ..actor(id, tenant_id)
        }
    }

    #[test]
    fn session_runtime_resolves_actor_context_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        {
            let auth = SessionAuthRuntime::open(&path).unwrap();
            auth.upsert_actor(actor("user-1", "org-1")).unwrap();
            let session = auth
                .create_session(SessionCreate {
                    id: "sess-1".to_string(),
                    actor_id: "user-1".to_string(),
                    tenant_id: "org-1".to_string(),
                    raw_token: "raw-session-secret".to_string(),
                    issued_ms: 1_000,
                    expires_ms: 9_000,
                    csrf_binding_id: "csrf-1".to_string(),
                })
                .unwrap();
            assert_eq!(session.token_hash, hash_session_secret("raw-session-secret"));
            assert!(!session.token_hash.contains("raw-session-secret"));
        }

        let auth = SessionAuthRuntime::open(&path).unwrap();
        let resolved = auth
            .resolve_session(
                "raw-session-secret",
                "org-1",
                "trace-1",
                "replay-auth-1",
                5_000,
            )
            .unwrap();
        assert_eq!(resolved.actor.id, "user-1");
        assert_eq!(resolved.trace.tenant_id, "org-1");
        assert_eq!(resolved.trace.actor_id, "user-1");
        assert_eq!(resolved.trace.session_id, "sess-1");
        assert_eq!(resolved.trace.replay_key, "replay-auth-1");
        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].status, "allowed");
        assert_eq!(audit[0].session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn session_runtime_rejects_expired_revoked_and_cross_tenant_sessions() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        auth.create_session(SessionCreate {
            id: "sess-1".to_string(),
            actor_id: "user-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_token: "secret-1".to_string(),
            issued_ms: 1_000,
            expires_ms: 2_000,
            csrf_binding_id: "csrf-1".to_string(),
        })
        .unwrap();

        let expired = auth
            .resolve_session("secret-1", "org-1", "trace-expired", "replay-expired", 2_000)
            .unwrap_err();
        assert!(expired.to_string().contains("session expired"));
        let tenant = auth
            .resolve_session("secret-1", "org-2", "trace-tenant", "replay-tenant", 1_500)
            .unwrap_err();
        assert!(tenant.to_string().contains("tenant mismatch"));
        auth.revoke_session("sess-1", 1_600).unwrap();
        let revoked = auth
            .resolve_session("secret-1", "org-1", "trace-revoked", "replay-revoked", 1_700)
            .unwrap_err();
        assert!(revoked.to_string().contains("session revoked"));

        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 3);
        assert!(audit.iter().all(|event| event.status == "denied"));
        assert!(audit.iter().all(|event| {
            !event.reason.contains("secret-1") && !event.id.contains("secret-1")
        }));
    }

    #[test]
    fn session_rotation_invalidates_old_token_and_preserves_rotation_counter() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        auth.create_session(SessionCreate {
            id: "sess-1".to_string(),
            actor_id: "user-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_token: "old-secret".to_string(),
            issued_ms: 1_000,
            expires_ms: 5_000,
            csrf_binding_id: "csrf-1".to_string(),
        })
        .unwrap();

        let rotated = auth.rotate_session("sess-1", "new-secret", 8_000).unwrap();
        assert_eq!(rotated.rotation_counter, 1);
        assert_eq!(rotated.token_hash, hash_session_secret("new-secret"));
        assert!(auth
            .resolve_session("old-secret", "org-1", "trace-old", "replay-old", 2_000)
            .is_err());
        let resolved = auth
            .resolve_session("new-secret", "org-1", "trace-new", "replay-new", 2_000)
            .unwrap();
        assert_eq!(resolved.session.rotation_counter, 1);
    }

    #[test]
    fn api_key_runtime_resolves_service_actor_with_argon2_hash_and_redacted_audit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        {
            let auth = SessionAuthRuntime::open(&path).unwrap();
            auth.upsert_actor(service_actor("svc-1", "org-1")).unwrap();
            let key = auth
                .create_api_key(ApiKeyCreate {
                    id: "key-1".to_string(),
                    service_actor_id: "svc-1".to_string(),
                    tenant_id: "org-1".to_string(),
                    raw_key: "raw-api-key-secret".to_string(),
                    scope_fingerprint: "sha256:service-scopes".to_string(),
                    expires_ms: now_ms().saturating_add(60_000),
                })
                .unwrap();
            assert!(key.key_hash.starts_with("$argon2"));
            assert!(!key.key_hash.contains("raw-api-key-secret"));
            assert!(verify_api_key_secret("raw-api-key-secret", &key.key_hash).unwrap());
        }

        let auth = SessionAuthRuntime::open(&path).unwrap();
        let resolved = auth
            .resolve_api_key(
                "raw-api-key-secret",
                "org-1",
                "trace-key-1",
                "replay-key-1",
                now_ms(),
            )
            .unwrap();
        assert_eq!(resolved.actor.id, "svc-1");
        assert_eq!(resolved.actor.actor_kind, "service");
        assert_eq!(resolved.trace.api_key_id, "key-1");
        assert_eq!(resolved.trace.session_id, "");
        assert_eq!(resolved.api_key.last_used_ms, Some(resolved.api_key.updated_ms));
        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].event_kind, "api_key.resolve");
        assert_eq!(audit[0].api_key_id.as_deref(), Some("key-1"));
        assert_eq!(audit[0].status, "allowed");
    }

    #[test]
    fn api_key_runtime_rejects_wrong_tenant_revoked_expired_and_user_actors() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(service_actor("svc-1", "org-1")).unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        let expires_ms = now_ms().saturating_add(60_000);
        auth.create_api_key(ApiKeyCreate {
            id: "key-1".to_string(),
            service_actor_id: "svc-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_key: "secret-1".to_string(),
            scope_fingerprint: "sha256:service-scopes".to_string(),
            expires_ms,
        })
        .unwrap();
        let user_key = auth.create_api_key(ApiKeyCreate {
            id: "key-user".to_string(),
            service_actor_id: "user-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_key: "user-secret".to_string(),
            scope_fingerprint: "sha256:user-scopes".to_string(),
            expires_ms,
        });
        assert!(user_key
            .unwrap_err()
            .to_string()
            .contains("service actors"));

        let wrong_tenant = auth
            .resolve_api_key("secret-1", "org-2", "trace-tenant", "replay-tenant", now_ms())
            .unwrap_err();
        assert!(wrong_tenant.to_string().contains("key not found"));

        let expired = auth
            .resolve_api_key("secret-1", "org-1", "trace-expired", "replay-expired", expires_ms)
            .unwrap_err();
        assert!(expired.to_string().contains("key expired"));

        auth.revoke_api_key("key-1", now_ms()).unwrap();
        let revoked = auth
            .resolve_api_key("secret-1", "org-1", "trace-revoked", "replay-revoked", now_ms())
            .unwrap_err();
        assert!(revoked.to_string().contains("key not found"));
        let audit = auth.audit_events().unwrap();
        assert!(audit.iter().all(|event| {
            !event.reason.contains("secret-1") && !event.id.contains("secret-1")
        }));
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

        for (algorithm, failure) in [("none", "unsupported_algorithm"), ("HS256", "unsupported_algorithm")] {
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
        assert_eq!(diagnostic.failure_kind.as_deref(), Some("jwks_url_not_https"));
        assert!(diagnostic.redacted);
    }

    #[test]
    fn oauth_callback_state_is_hashed_single_use_and_restart_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let expires_ms = now_ms().saturating_add(60_000);
        {
            let auth = SessionAuthRuntime::open(&path).unwrap();
            auth.upsert_actor(actor("user-1", "org-1")).unwrap();
            let state = auth
                .create_oauth_state(OAuthStateCreate {
                    id: "oauth-state-1".to_string(),
                    provider: "google".to_string(),
                    tenant_id: "org-1".to_string(),
                    actor_id: "user-1".to_string(),
                    raw_state: "raw-oauth-state".to_string(),
                    pkce_verifier_ref: "pkce-ref-1".to_string(),
                    nonce_fingerprint: "sha256:nonce".to_string(),
                    expires_ms,
                    replay_key: "replay-oauth-1".to_string(),
                })
                .unwrap();
            assert_eq!(state.state_hash, hash_oauth_state("raw-oauth-state"));
            assert!(!state.state_hash.contains("raw-oauth-state"));
            assert_eq!(state.used_ms, None);
        }

        let auth = SessionAuthRuntime::open(&path).unwrap();
        let resolved = auth
            .resolve_oauth_callback("raw-oauth-state", "org-1", "trace-oauth-1", now_ms())
            .unwrap();
        assert_eq!(resolved.actor.id, "user-1");
        assert_eq!(resolved.trace.auth_method, "oauth");
        assert_eq!(resolved.trace.replay_key, "replay-oauth-1");
        assert!(resolved.state.used_ms.is_some());

        let replay = auth
            .resolve_oauth_callback("raw-oauth-state", "org-1", "trace-oauth-2", now_ms())
            .unwrap_err();
        assert!(replay.to_string().contains("state already used"));
        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().all(|event| {
            !event.reason.contains("raw-oauth-state") && !event.id.contains("raw-oauth-state")
        }));
    }

    #[test]
    fn oauth_callback_rejects_expired_and_cross_tenant_state() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        let expires_ms = now_ms().saturating_add(60_000);
        auth.create_oauth_state(OAuthStateCreate {
            id: "oauth-state-1".to_string(),
            provider: "github".to_string(),
            tenant_id: "org-1".to_string(),
            actor_id: "user-1".to_string(),
            raw_state: "state-1".to_string(),
            pkce_verifier_ref: "pkce-ref-1".to_string(),
            nonce_fingerprint: "sha256:nonce".to_string(),
            expires_ms,
            replay_key: "replay-oauth-1".to_string(),
        })
        .unwrap();

        let tenant = auth
            .resolve_oauth_callback("state-1", "org-2", "trace-tenant", now_ms())
            .unwrap_err();
        assert!(tenant.to_string().contains("tenant mismatch"));
        let expired = auth
            .resolve_oauth_callback("state-1", "org-1", "trace-expired", expires_ms)
            .unwrap_err();
        assert!(expired.to_string().contains("state expired"));
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
