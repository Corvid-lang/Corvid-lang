//! Runtime storage for `session` and `memory` declarations.
//!
//! The compiler-owned schema lives in the ABI. This module is the native
//! backing store that generated accessors call into: a typed accessor passes
//! `(kind, store, key, value)` and receives JSON wire values at the runtime
//! boundary. Native uses SQLite; tests and embedded hosts can use the in-memory
//! backend with the same contract.

use crate::errors::RuntimeError;
use crate::provenance::ProvenanceChain;
use corvid_abi::AbiStorePolicy;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreKind {
    Session,
    Memory,
}

impl StoreKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Memory => "memory",
        }
    }

    pub fn read_effect(&self) -> &'static str {
        match self {
            Self::Session => "reads_session",
            Self::Memory => "reads_memory",
        }
    }

    pub fn write_effect(&self) -> &'static str {
        match self {
            Self::Session => "writes_session",
            Self::Memory => "writes_memory",
        }
    }
}

pub trait StoreBackend: Send + Sync {
    fn get_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<StoreRecord>, RuntimeError>;
    fn put_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError>;
    fn put_record_if_revision(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError>;
    fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError>;

    fn get(&self, kind: StoreKind, store: &str, key: &str) -> Result<Option<Value>, RuntimeError> {
        Ok(self.get_record(kind, store, key)?.map(|record| record.value))
    }

    fn put(&self, kind: StoreKind, store: &str, key: &str, value: Value) -> Result<(), RuntimeError> {
        self.put_record(kind, store, key, StoreRecord::plain(value))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoreRecord {
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceChain>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub updated_at_ms: u64,
}

impl StoreRecord {
    pub fn plain(value: Value) -> Self {
        Self {
            value,
            provenance: None,
            revision: 0,
            updated_at_ms: 0,
        }
    }

    pub fn grounded(value: Value, provenance: ProvenanceChain) -> Self {
        Self {
            value,
            provenance: Some(provenance),
            revision: 0,
            updated_at_ms: 0,
        }
    }

    fn with_metadata(mut self, revision: u64, updated_at_ms: u64) -> Self {
        self.revision = revision;
        self.updated_at_ms = updated_at_ms;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorePolicySet {
    pub ttl_ms: Option<u64>,
    pub user_delete: bool,
    pub legal_hold: bool,
    pub approval_required: bool,
    pub privacy_tier: Option<String>,
}

impl StorePolicySet {
    pub fn from_abi_policies(policies: &[AbiStorePolicy]) -> Result<Self, RuntimeError> {
        let mut set = Self::default();
        for policy in policies {
            set.apply_policy(&policy.name, &policy.value)?;
        }
        Ok(set)
    }

    pub fn ttl_ms(ttl_ms: u64) -> Self {
        Self {
            ttl_ms: Some(ttl_ms),
            ..Self::default()
        }
    }

    pub fn legal_hold() -> Self {
        Self {
            legal_hold: true,
            ..Self::default()
        }
    }

    pub fn is_expired(&self, record: &StoreRecord, now_ms: u64) -> bool {
        self.ttl_ms
            .map(|ttl_ms| now_ms.saturating_sub(record.updated_at_ms) >= ttl_ms)
            .unwrap_or(false)
    }

    fn apply_policy(&mut self, name: &str, value: &Value) -> Result<(), RuntimeError> {
        match name {
            "retention" => self.ttl_ms = parse_ttl_policy(value)?,
            "ttl_ms" => self.ttl_ms = Some(parse_u64_policy(name, value)?),
            "user_delete" => self.user_delete = parse_bool_policy(name, value)?,
            "legal_hold" => self.legal_hold = parse_bool_policy(name, value)?,
            "approval_required" => self.approval_required = parse_bool_policy(name, value)?,
            "privacy_tier" => self.privacy_tier = Some(parse_string_policy(name, value)?),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct StoreManager {
    backend: Arc<dyn StoreBackend>,
}

impl Default for StoreManager {
    fn default() -> Self {
        Self::memory()
    }
}

impl StoreManager {
    pub fn new(backend: Arc<dyn StoreBackend>) -> Self {
        Self { backend }
    }

    pub fn memory() -> Self {
        Self::new(Arc::new(InMemoryStoreBackend::default()))
    }

    pub fn sqlite(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        Ok(Self::new(Arc::new(SqliteStoreBackend::open(path)?)))
    }

    pub fn get(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<Value>, RuntimeError> {
        self.backend.get(kind, store, key)
    }

    pub fn put(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        value: Value,
    ) -> Result<(), RuntimeError> {
        self.backend.put(kind, store, key, value)
    }

    pub fn get_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        self.backend.get_record(kind, store, key)
    }

    pub fn get_record_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        policy: &StorePolicySet,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        let record = self.backend.get_record(kind, store, key)?;
        if record
            .as_ref()
            .map(|record| policy.is_expired(record, crate::tracing::now_ms()))
            .unwrap_or(false)
        {
            self.backend.delete(kind, store, key)?;
            return Ok(None);
        }
        Ok(record)
    }

    pub fn put_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        self.backend.put_record(kind, store, key, record)
    }

    pub fn put_record_if_revision(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        self.backend
            .put_record_if_revision(kind, store, key, expected_revision, record)
    }

    pub fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        self.backend.delete(kind, store, key)
    }

    pub fn delete_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        policy: &StorePolicySet,
    ) -> Result<(), RuntimeError> {
        if policy.legal_hold {
            return Err(store_policy_violation(
                kind,
                store,
                key,
                "legal_hold",
                "record deletion is blocked while legal hold is active",
            ));
        }
        self.backend.delete(kind, store, key)
    }
}

#[derive(Default)]
pub struct InMemoryStoreBackend {
    values: Mutex<HashMap<StoreKey, StoreRecord>>,
}

impl StoreBackend for InMemoryStoreBackend {
    fn get_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        let values = self.values.lock().map_err(store_lock_error)?;
        Ok(values.get(&StoreKey::new(kind, store, key)).cloned())
    }

    fn put_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let mut values = self.values.lock().map_err(store_lock_error)?;
        let store_key = StoreKey::new(kind, store, key);
        let next_revision = values
            .get(&store_key)
            .map(|existing| existing.revision.saturating_add(1))
            .unwrap_or(1);
        let record = record.with_metadata(next_revision, crate::tracing::now_ms());
        values.insert(store_key, record.clone());
        Ok(record)
    }

    fn put_record_if_revision(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let mut values = self.values.lock().map_err(store_lock_error)?;
        let store_key = StoreKey::new(kind, store, key);
        let actual_revision = values.get(&store_key).map(|existing| existing.revision);
        if actual_revision != Some(expected_revision) {
            return Err(store_conflict(
                kind,
                store,
                key,
                expected_revision,
                actual_revision,
            ));
        }
        let record =
            record.with_metadata(expected_revision.saturating_add(1), crate::tracing::now_ms());
        values.insert(store_key, record.clone());
        Ok(record)
    }

    fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        let mut values = self.values.lock().map_err(store_lock_error)?;
        values.remove(&StoreKey::new(kind, store, key));
        Ok(())
    }
}

pub struct SqliteStoreBackend {
    conn: Mutex<rusqlite::Connection>,
}

impl SqliteStoreBackend {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let conn = rusqlite::Connection::open(path.as_ref()).map_err(sqlite_error)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS corvid_store (
                kind TEXT NOT NULL,
                store TEXT NOT NULL,
                key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                provenance_json TEXT,
                revision INTEGER NOT NULL DEFAULT 1,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (kind, store, key)
            );
            "#,
        )
        .map_err(sqlite_error)?;
        let _ = conn.execute("ALTER TABLE corvid_store ADD COLUMN provenance_json TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE corvid_store ADD COLUMN revision INTEGER NOT NULL DEFAULT 1",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl StoreBackend for SqliteStoreBackend {
    fn get_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        let conn = self.conn.lock().map_err(store_lock_error)?;
        let mut stmt = conn
            .prepare(
                "SELECT value_json, provenance_json, revision, updated_at_ms FROM corvid_store WHERE kind = ?1 AND store = ?2 AND key = ?3",
            )
            .map_err(sqlite_error)?;
        let row = stmt
            .query_row((kind.as_str(), store, key), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map(Some)
            .or_else(|err| {
                if matches!(err, rusqlite::Error::QueryReturnedNoRows) {
                    Ok(None)
                } else {
                    Err(sqlite_error(err))
                }
            })?;
        row.map(|(value_json, provenance_json, revision, updated_at_ms)| {
            let value = serde_json::from_str(&value_json).map_err(store_json_error)?;
            let provenance = provenance_json
                .map(|json| serde_json::from_str(&json).map_err(store_json_error))
                .transpose()?;
            Ok(StoreRecord {
                value,
                provenance,
                revision: revision.max(0) as u64,
                updated_at_ms: updated_at_ms.max(0) as u64,
            })
        })
        .transpose()
    }

    fn put_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let value_json = serde_json::to_string(&record.value).map_err(store_json_error)?;
        let provenance_json = record
            .provenance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(store_json_error)?;
        let conn = self.conn.lock().map_err(store_lock_error)?;
        let current_revision = sqlite_current_revision(&conn, kind, store, key)?;
        let next_revision = current_revision
            .map(|revision| revision.saturating_add(1))
            .unwrap_or(1);
        let updated_at_ms = crate::tracing::now_ms();
        conn.execute(
            r#"
            INSERT INTO corvid_store (kind, store, key, value_json, provenance_json, revision, updated_at_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(kind, store, key)
            DO UPDATE SET value_json = excluded.value_json,
                          provenance_json = excluded.provenance_json,
                          revision = excluded.revision,
                          updated_at_ms = excluded.updated_at_ms
            "#,
            (
                kind.as_str(),
                store,
                key,
                value_json,
                provenance_json,
                next_revision as i64,
                updated_at_ms as i64,
            ),
        )
        .map_err(sqlite_error)?;
        Ok(record.with_metadata(next_revision, updated_at_ms))
    }

    fn put_record_if_revision(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let value_json = serde_json::to_string(&record.value).map_err(store_json_error)?;
        let provenance_json = record
            .provenance
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(store_json_error)?;
        let conn = self.conn.lock().map_err(store_lock_error)?;
        let actual_revision = sqlite_current_revision(&conn, kind, store, key)?;
        if actual_revision != Some(expected_revision) {
            return Err(store_conflict(
                kind,
                store,
                key,
                expected_revision,
                actual_revision,
            ));
        }
        let next_revision = expected_revision.saturating_add(1);
        let updated_at_ms = crate::tracing::now_ms();
        conn.execute(
            r#"
            UPDATE corvid_store
            SET value_json = ?4,
                provenance_json = ?5,
                revision = ?6,
                updated_at_ms = ?7
            WHERE kind = ?1 AND store = ?2 AND key = ?3 AND revision = ?8
            "#,
            (
                kind.as_str(),
                store,
                key,
                value_json,
                provenance_json,
                next_revision as i64,
                updated_at_ms as i64,
                expected_revision as i64,
            ),
        )
        .map_err(sqlite_error)?;
        Ok(record.with_metadata(next_revision, updated_at_ms))
    }

    fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        let conn = self.conn.lock().map_err(store_lock_error)?;
        conn.execute(
            "DELETE FROM corvid_store WHERE kind = ?1 AND store = ?2 AND key = ?3",
            (kind.as_str(), store, key),
        )
        .map_err(sqlite_error)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StoreKey {
    kind: StoreKind,
    store: String,
    key: String,
}

impl StoreKey {
    fn new(kind: StoreKind, store: &str, key: &str) -> Self {
        Self {
            kind,
            store: store.to_string(),
            key: key.to_string(),
        }
    }
}

fn sqlite_error(err: rusqlite::Error) -> RuntimeError {
    RuntimeError::Other(format!("store sqlite error: {err}"))
}

fn store_json_error(err: serde_json::Error) -> RuntimeError {
    RuntimeError::Other(format!("store JSON error: {err}"))
}

fn store_lock_error<T>(err: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::Other(format!("store lock poisoned: {err}"))
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn store_conflict(
    kind: StoreKind,
    store: &str,
    key: &str,
    expected_revision: u64,
    actual_revision: Option<u64>,
) -> RuntimeError {
    RuntimeError::StoreConflict {
        kind: kind.as_str().to_string(),
        store: store.to_string(),
        key: key.to_string(),
        expected_revision,
        actual_revision,
    }
}

fn store_policy_violation(
    kind: StoreKind,
    store: &str,
    key: &str,
    policy: &str,
    message: &str,
) -> RuntimeError {
    RuntimeError::StorePolicyViolation {
        kind: kind.as_str().to_string(),
        store: store.to_string(),
        key: key.to_string(),
        policy: policy.to_string(),
        message: message.to_string(),
    }
}

fn sqlite_current_revision(
    conn: &rusqlite::Connection,
    kind: StoreKind,
    store: &str,
    key: &str,
) -> Result<Option<u64>, RuntimeError> {
    conn.query_row(
        "SELECT revision FROM corvid_store WHERE kind = ?1 AND store = ?2 AND key = ?3",
        (kind.as_str(), store, key),
        |row| row.get::<_, i64>(0),
    )
    .map(|revision| Some(revision.max(0) as u64))
    .or_else(|err| {
        if matches!(err, rusqlite::Error::QueryReturnedNoRows) {
            Ok(None)
        } else {
            Err(sqlite_error(err))
        }
    })
}

fn parse_ttl_policy(value: &Value) -> Result<Option<u64>, RuntimeError> {
    if value.is_null() {
        return Ok(None);
    }
    if let Some(ms) = value.as_u64() {
        return Ok(Some(ms));
    }
    let raw = value
        .as_str()
        .ok_or_else(|| policy_parse_error("retention", value, "TTL string or millisecond number"))?;
    if raw == "forever" || raw == "none" {
        return Ok(None);
    }
    let ttl = raw
        .strip_prefix("ttl_")
        .ok_or_else(|| policy_parse_error("retention", value, "`ttl_<number><unit>`"))?;
    parse_ttl_string(ttl).map(Some)
}

fn parse_ttl_string(raw: &str) -> Result<u64, RuntimeError> {
    let split = raw
        .find(|ch: char| !ch.is_ascii_digit())
        .ok_or_else(|| RuntimeError::Other(format!("invalid store retention policy `ttl_{raw}`")))?;
    let (amount, unit) = raw.split_at(split);
    let amount = amount
        .parse::<u64>()
        .map_err(|_| RuntimeError::Other(format!("invalid store retention amount `{amount}`")))?;
    let unit_ms = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => {
            return Err(RuntimeError::Other(format!(
                "invalid store retention unit `{unit}`"
            )))
        }
    };
    Ok(amount.saturating_mul(unit_ms))
}

fn parse_u64_policy(name: &str, value: &Value) -> Result<u64, RuntimeError> {
    value
        .as_u64()
        .ok_or_else(|| policy_parse_error(name, value, "unsigned integer"))
}

fn parse_bool_policy(name: &str, value: &Value) -> Result<bool, RuntimeError> {
    value
        .as_bool()
        .ok_or_else(|| policy_parse_error(name, value, "boolean"))
}

fn parse_string_policy(name: &str, value: &Value) -> Result<String, RuntimeError> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| policy_parse_error(name, value, "string"))
}

fn policy_parse_error(name: &str, value: &Value, expected: &str) -> RuntimeError {
    RuntimeError::Other(format!(
        "invalid store policy `{name}` value {value}: expected {expected}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ProvenanceKind;
    use serde_json::json;

    #[test]
    fn sqlite_store_persists_session_and_memory_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("corvid-store.sqlite3");
        let store = StoreManager::sqlite(&path).expect("open sqlite store");

        store
            .put(
                StoreKind::Session,
                "Conversation",
                "thread-1",
                json!({"cart": ["sku-1"]}),
            )
            .expect("put session");
        store
            .put(
                StoreKind::Memory,
                "Profile",
                "user-1",
                json!({"preference": "quiet"}),
            )
            .expect("put memory");

        drop(store);
        let reopened = StoreManager::sqlite(&path).expect("reopen sqlite store");
        assert_eq!(
            reopened
                .get(StoreKind::Session, "Conversation", "thread-1")
                .expect("get session"),
            Some(json!({"cart": ["sku-1"]}))
        );
        assert_eq!(
            reopened
                .get(StoreKind::Memory, "Profile", "user-1")
                .expect("get memory"),
            Some(json!({"preference": "quiet"}))
        );
    }

    #[test]
    fn sqlite_store_preserves_provenance_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("corvid-store.sqlite3");
        let store = StoreManager::sqlite(&path).expect("open sqlite store");
        let provenance = ProvenanceChain::with_retrieval("lookup_profile", 42);

        store
            .put_record(
                StoreKind::Memory,
                "Profile",
                "user-1",
                StoreRecord::grounded(json!({"fact": "likes quiet"}), provenance.clone()),
            )
            .expect("put grounded memory");

        let record = store
            .get_record(StoreKind::Memory, "Profile", "user-1")
            .expect("get record")
            .expect("record present");
        assert_eq!(record.value, json!({"fact": "likes quiet"}));
        assert_eq!(record.revision, 1);
        let restored = record.provenance.expect("provenance");
        assert_eq!(restored.entries.len(), 1);
        assert_eq!(restored.entries[0].kind, ProvenanceKind::Retrieval);
        assert_eq!(restored.entries[0].name, "lookup_profile");
    }

    #[test]
    fn sqlite_store_rejects_stale_revision_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("corvid-store.sqlite3");
        let store = StoreManager::sqlite(&path).expect("open sqlite store");

        let first = store
            .put_record(
                StoreKind::Memory,
                "Profile",
                "user-1",
                StoreRecord::plain(json!({"fact": "alpha"})),
            )
            .expect("put first");
        assert_eq!(first.revision, 1);

        let second = store
            .put_record_if_revision(
                StoreKind::Memory,
                "Profile",
                "user-1",
                first.revision,
                StoreRecord::plain(json!({"fact": "beta"})),
            )
            .expect("put second");
        assert_eq!(second.revision, 2);

        let stale = store
            .put_record_if_revision(
                StoreKind::Memory,
                "Profile",
                "user-1",
                first.revision,
                StoreRecord::plain(json!({"fact": "stale"})),
            )
            .expect_err("stale write must fail");
        match stale {
            RuntimeError::StoreConflict {
                expected_revision,
                actual_revision,
                ..
            } => {
                assert_eq!(expected_revision, 1);
                assert_eq!(actual_revision, Some(2));
            }
            other => panic!("unexpected error: {other}"),
        }

        assert_eq!(
            store
                .get(StoreKind::Memory, "Profile", "user-1")
                .expect("get current"),
            Some(json!({"fact": "beta"}))
        );
    }

    #[test]
    fn store_policy_set_parses_abi_policies() {
        let policies = vec![
            AbiStorePolicy {
                name: "retention".to_string(),
                value: json!("ttl_24h"),
            },
            AbiStorePolicy {
                name: "legal_hold".to_string(),
                value: json!(true),
            },
            AbiStorePolicy {
                name: "privacy_tier".to_string(),
                value: json!("restricted"),
            },
        ];

        let policy = StorePolicySet::from_abi_policies(&policies).expect("parse policies");
        assert_eq!(policy.ttl_ms, Some(86_400_000));
        assert!(policy.legal_hold);
        assert_eq!(policy.privacy_tier.as_deref(), Some("restricted"));
    }

    #[test]
    fn store_policy_ttl_expires_records_on_read() {
        let store = StoreManager::memory();
        store
            .put(
                StoreKind::Session,
                "Conversation",
                "thread-1",
                json!({"topic": "shipping"}),
            )
            .expect("put");

        assert_eq!(
            store
                .get_record_with_policy(
                    StoreKind::Session,
                    "Conversation",
                    "thread-1",
                    &StorePolicySet::ttl_ms(0),
                )
                .expect("get with ttl"),
            None
        );
        assert_eq!(
            store
                .get(StoreKind::Session, "Conversation", "thread-1")
                .expect("get after expiry"),
            None
        );
    }

    #[test]
    fn store_policy_legal_hold_blocks_delete() {
        let store = StoreManager::memory();
        store
            .put(
                StoreKind::Memory,
                "Profile",
                "user-1",
                json!({"fact": "protected"}),
            )
            .expect("put");

        let err = store
            .delete_with_policy(
                StoreKind::Memory,
                "Profile",
                "user-1",
                &StorePolicySet::legal_hold(),
            )
            .expect_err("delete must be blocked");
        match err {
            RuntimeError::StorePolicyViolation { policy, .. } => {
                assert_eq!(policy, "legal_hold");
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            store
                .get(StoreKind::Memory, "Profile", "user-1")
                .expect("record still present"),
            Some(json!({"fact": "protected"}))
        );
    }
}
