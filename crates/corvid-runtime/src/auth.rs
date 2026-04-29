use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;

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
pub struct AuthTraceContext {
    pub trace_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub auth_method: String,
    pub session_id: String,
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
pub struct AuthAuditEvent {
    pub id: String,
    pub event_kind: String,
    pub actor_id: Option<String>,
    pub tenant_id: Option<String>,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub status: String,
    pub reason: String,
    pub created_ms: u64,
}

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

    pub fn create_session(&self, input: SessionCreate) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", &input.id)?;
        validate_non_empty("actor id", &input.actor_id)?;
        validate_non_empty("tenant id", &input.tenant_id)?;
        validate_non_empty("session token", &input.raw_token)?;
        if input.expires_ms <= input.issued_ms {
            return Err(RuntimeError::Other(
                "session expiry must be after issue time".to_string(),
            ));
        }
        let actor = self
            .get_actor(&input.actor_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth actor `{}` not found", input.actor_id)))?;
        if actor.tenant_id != input.tenant_id {
            return Err(RuntimeError::Other(
                "session actor tenant mismatch".to_string(),
            ));
        }
        let token_hash = hash_session_secret(&input.raw_token);
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_sessions
                 (id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, null, ?8, ?8)",
                params![
                    input.id,
                    input.actor_id,
                    input.tenant_id,
                    token_hash,
                    input.issued_ms as i64,
                    input.expires_ms as i64,
                    input.csrf_binding_id,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to create session: {err}")))?;
        self.get_session(&input.id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{}` not found", input.id)))
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms
                 from auth_sessions where id = ?1",
                params![id],
                read_session_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read session: {err}")))
    }

    pub fn resolve_session(
        &self,
        raw_token: &str,
        expected_tenant_id: &str,
        trace_id: &str,
        replay_key: &str,
        at_ms: u64,
    ) -> Result<SessionResolution, RuntimeError> {
        validate_non_empty("session token", raw_token)?;
        validate_non_empty("tenant id", expected_tenant_id)?;
        validate_non_empty("trace id", trace_id)?;
        let token_hash = hash_session_secret(raw_token);
        let session = match self.get_session_by_hash(&token_hash)? {
            Some(session) => session,
            None => {
                self.insert_audit(
                    "session.resolve",
                    None,
                    Some(expected_tenant_id),
                    None,
                    Some(trace_id),
                    "denied",
                    "session token not found",
                )?;
                return Err(RuntimeError::Other(
                    "session resolve denied: token not found".to_string(),
                ));
            }
        };
        if session.revoked_ms.is_some() {
            self.audit_session_denied(&session, trace_id, "session revoked")?;
            return Err(RuntimeError::Other(
                "session resolve denied: session revoked".to_string(),
            ));
        }
        if at_ms >= session.expires_ms {
            self.audit_session_denied(&session, trace_id, "session expired")?;
            return Err(RuntimeError::Other(
                "session resolve denied: session expired".to_string(),
            ));
        }
        if session.tenant_id != expected_tenant_id {
            self.audit_session_denied(&session, trace_id, "tenant mismatch")?;
            return Err(RuntimeError::Other(
                "session resolve denied: tenant mismatch".to_string(),
            ));
        }
        let actor = self
            .get_actor(&session.actor_id)?
            .ok_or_else(|| RuntimeError::Other("session actor not found".to_string()))?;
        if actor.tenant_id != session.tenant_id {
            self.audit_session_denied(&session, trace_id, "actor tenant mismatch")?;
            return Err(RuntimeError::Other(
                "session resolve denied: actor tenant mismatch".to_string(),
            ));
        }
        let trace = AuthTraceContext {
            trace_id: trace_id.to_string(),
            tenant_id: session.tenant_id.clone(),
            actor_id: actor.id.clone(),
            auth_method: actor.auth_method.clone(),
            session_id: session.id.clone(),
            permission_fingerprint: actor.permission_fingerprint.clone(),
            replay_key: replay_key.to_string(),
        };
        self.insert_audit(
            "session.resolve",
            Some(&actor.id),
            Some(&session.tenant_id),
            Some(&session.id),
            Some(trace_id),
            "allowed",
            "session resolved",
        )?;
        Ok(SessionResolution {
            actor,
            session,
            trace,
        })
    }

    pub fn rotate_session(
        &self,
        session_id: &str,
        new_raw_token: &str,
        new_expires_ms: u64,
    ) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", session_id)?;
        validate_non_empty("session token", new_raw_token)?;
        let session = self
            .get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))?;
        if new_expires_ms <= session.issued_ms {
            return Err(RuntimeError::Other(
                "rotated session expiry must be after issue time".to_string(),
            ));
        }
        let now = now_ms();
        let token_hash = hash_session_secret(new_raw_token);
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_sessions
                 set token_hash = ?2, expires_ms = ?3, rotation_counter = rotation_counter + 1, revoked_ms = null, updated_ms = ?4
                 where id = ?1",
                params![session_id, token_hash, new_expires_ms as i64, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to rotate session: {err}")))?;
        self.get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))
    }

    pub fn revoke_session(&self, session_id: &str, at_ms: u64) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", session_id)?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_sessions set revoked_ms = ?2, updated_ms = ?2 where id = ?1",
                params![session_id, at_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to revoke session: {err}")))?;
        self.get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))
    }

    pub fn audit_events(&self) -> Result<Vec<AuthAuditEvent>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, event_kind, actor_id, tenant_id, session_id, trace_id, status, reason, created_ms
                 from auth_audit_events order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare auth audit list: {err}")))?;
        let rows = stmt
            .query_map([], read_audit_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list auth audit events: {err}")))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode auth audit event: {err}"))
            })?);
        }
        Ok(events)
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
                create table if not exists auth_audit_events (
                    id text primary key,
                    event_kind text not null,
                    actor_id text,
                    tenant_id text,
                    session_id text,
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

    fn get_session_by_hash(&self, token_hash: &str) -> Result<Option<SessionRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms
                 from auth_sessions where token_hash = ?1",
                params![token_hash],
                read_session_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read session by token: {err}")))
    }

    fn audit_session_denied(
        &self,
        session: &SessionRecord,
        trace_id: &str,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        self.insert_audit(
            "session.resolve",
            Some(&session.actor_id),
            Some(&session.tenant_id),
            Some(&session.id),
            Some(trace_id),
            "denied",
            reason,
        )
    }

    fn insert_audit(
        &self,
        event_kind: &str,
        actor_id: Option<&str>,
        tenant_id: Option<&str>,
        session_id: Option<&str>,
        trace_id: Option<&str>,
        status: &str,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        let now = now_ms();
        let id = format!("auth_audit_{now}_{}", stable_suffix(event_kind, session_id, trace_id));
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_audit_events
                 (id, event_kind, actor_id, tenant_id, session_id, trace_id, status, reason, created_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    event_kind,
                    actor_id,
                    tenant_id,
                    session_id,
                    trace_id,
                    status,
                    reason,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert auth audit event: {err}")))?;
        Ok(())
    }
}

pub fn hash_session_secret(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"corvid-auth-session-v1:");
    hasher.update(raw_token.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), RuntimeError> {
    if value.trim().is_empty() {
        Err(RuntimeError::Other(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

fn stable_suffix(event_kind: &str, session_id: Option<&str>, trace_id: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_kind.as_bytes());
    hasher.update(b":");
    hasher.update(session_id.unwrap_or("").as_bytes());
    hasher.update(b":");
    hasher.update(trace_id.unwrap_or("").as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_string()
}

fn read_actor_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthActor> {
    Ok(AuthActor {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        display_name: row.get(2)?,
        actor_kind: row.get(3)?,
        auth_method: row.get(4)?,
        assurance_level: row.get(5)?,
        role_fingerprint: row.get(6)?,
        permission_fingerprint: row.get(7)?,
        created_ms: row.get::<_, i64>(8)? as u64,
        updated_ms: row.get::<_, i64>(9)? as u64,
    })
}

fn read_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        actor_id: row.get(1)?,
        tenant_id: row.get(2)?,
        token_hash: row.get(3)?,
        issued_ms: row.get::<_, i64>(4)? as u64,
        expires_ms: row.get::<_, i64>(5)? as u64,
        rotation_counter: row.get::<_, i64>(6)? as u64,
        csrf_binding_id: row.get(7)?,
        revoked_ms: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
        created_ms: row.get::<_, i64>(9)? as u64,
        updated_ms: row.get::<_, i64>(10)? as u64,
    })
}

fn read_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthAuditEvent> {
    Ok(AuthAuditEvent {
        id: row.get(0)?,
        event_kind: row.get(1)?,
        actor_id: row.get(2)?,
        tenant_id: row.get(3)?,
        session_id: row.get(4)?,
        trace_id: row.get(5)?,
        status: row.get(6)?,
        reason: row.get(7)?,
        created_ms: row.get::<_, i64>(8)? as u64,
    })
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
}
