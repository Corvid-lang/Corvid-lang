use super::*;

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
        let _ = conn.execute(
            "ALTER TABLE corvid_store ADD COLUMN provenance_json TEXT",
            [],
        );
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

fn sqlite_error(err: rusqlite::Error) -> RuntimeError {
    RuntimeError::Other(format!("store sqlite error: {err}"))
}

fn store_json_error(err: serde_json::Error) -> RuntimeError {
    RuntimeError::Other(format!("store JSON error: {err}"))
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
