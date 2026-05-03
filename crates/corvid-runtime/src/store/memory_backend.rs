use super::*;

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
        let record = record.with_metadata(
            expected_revision.saturating_add(1),
            crate::tracing::now_ms(),
        );
        values.insert(store_key, record.clone());
        Ok(record)
    }

    fn delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        let mut values = self.values.lock().map_err(store_lock_error)?;
        values.remove(&StoreKey::new(kind, store, key));
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
