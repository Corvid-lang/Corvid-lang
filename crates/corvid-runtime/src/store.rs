//! Runtime storage for `session` and `memory` declarations.
//!
//! The compiler-owned schema lives in the ABI. This module is the native
//! backing store that generated accessors call into: a typed accessor passes
//! `(kind, store, key, value)` and receives JSON wire values at the runtime
//! boundary. Native uses SQLite; tests and embedded hosts can use the in-memory
//! backend with the same contract.

use crate::errors::RuntimeError;
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
    fn get(&self, kind: StoreKind, store: &str, key: &str) -> Result<Option<Value>, RuntimeError>;
    fn put(&self, kind: StoreKind, store: &str, key: &str, value: Value) -> Result<(), RuntimeError>;
    fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError>;
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

    pub fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        self.backend.delete(kind, store, key)
    }
}

#[derive(Default)]
pub struct InMemoryStoreBackend {
    values: Mutex<HashMap<StoreKey, Value>>,
}

impl StoreBackend for InMemoryStoreBackend {
    fn get(&self, kind: StoreKind, store: &str, key: &str) -> Result<Option<Value>, RuntimeError> {
        let values = self.values.lock().map_err(store_lock_error)?;
        Ok(values.get(&StoreKey::new(kind, store, key)).cloned())
    }

    fn put(&self, kind: StoreKind, store: &str, key: &str, value: Value) -> Result<(), RuntimeError> {
        let mut values = self.values.lock().map_err(store_lock_error)?;
        values.insert(StoreKey::new(kind, store, key), value);
        Ok(())
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
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (kind, store, key)
            );
            "#,
        )
        .map_err(sqlite_error)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl StoreBackend for SqliteStoreBackend {
    fn get(&self, kind: StoreKind, store: &str, key: &str) -> Result<Option<Value>, RuntimeError> {
        let conn = self.conn.lock().map_err(store_lock_error)?;
        let mut stmt = conn
            .prepare(
                "SELECT value_json FROM corvid_store WHERE kind = ?1 AND store = ?2 AND key = ?3",
            )
            .map_err(sqlite_error)?;
        let value_json = stmt
            .query_row((kind.as_str(), store, key), |row| row.get::<_, String>(0))
            .map(Some)
            .or_else(|err| {
                if matches!(err, rusqlite::Error::QueryReturnedNoRows) {
                    Ok(None)
                } else {
                    Err(sqlite_error(err))
                }
            })?;
        value_json
            .map(|json| serde_json::from_str(&json).map_err(store_json_error))
            .transpose()
    }

    fn put(&self, kind: StoreKind, store: &str, key: &str, value: Value) -> Result<(), RuntimeError> {
        let value_json = serde_json::to_string(&value).map_err(store_json_error)?;
        let conn = self.conn.lock().map_err(store_lock_error)?;
        conn.execute(
            r#"
            INSERT INTO corvid_store (kind, store, key, value_json, updated_at_ms)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(kind, store, key)
            DO UPDATE SET value_json = excluded.value_json,
                          updated_at_ms = excluded.updated_at_ms
            "#,
            (
                kind.as_str(),
                store,
                key,
                value_json,
                crate::tracing::now_ms() as i64,
            ),
        )
        .map_err(sqlite_error)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
