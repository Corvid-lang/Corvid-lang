//! Durable-store dispatch methods on `Runtime` — the
//! `store_get` / `store_put` / `store_delete` family in three
//! variants (plain, with policy, with optimistic-locking
//! revision check). Plus the policy-side approval gate and the
//! per-op trace emitter.
//!
//! Each public method delegates to the `StoreManager` collaborator
//! and emits a `store` host event so traces show every read/write
//! attempt regardless of outcome. `approve_store_write_if_required`
//! consults the per-store `StorePolicySet` and, when an approver
//! gate is required, defers to `Runtime::approval_gate` (in
//! mod.rs).

use crate::errors::RuntimeError;
use crate::store::{StoreKind, StoreManager, StorePolicySet, StoreRecord};
use crate::tracing::now_ms;
use corvid_trace_schema::TraceEvent;

use super::Runtime;

impl Runtime {
    pub fn stores(&self) -> &StoreManager {
        &self.stores
    }

    pub fn store_get(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, RuntimeError> {
        let value = self.stores.get(kind, store, key)?;
        self.emit_store_event("get", kind, store, key, value.as_ref());
        Ok(value)
    }

    pub fn store_put(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), RuntimeError> {
        self.stores.put(kind, store, key, value)?;
        self.emit_store_event("put", kind, store, key, None);
        Ok(())
    }

    pub async fn store_put_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        value: serde_json::Value,
        policy: &StorePolicySet,
    ) -> Result<(), RuntimeError> {
        self.store_put_record_with_policy(kind, store, key, StoreRecord::plain(value), policy)
            .await?;
        Ok(())
    }

    pub fn store_get_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        let record = self.stores.get_record(kind, store, key)?;
        self.emit_store_event("get", kind, store, key, record.as_ref().map(|r| &r.value));
        Ok(record)
    }

    pub fn store_get_record_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        policy: &StorePolicySet,
    ) -> Result<Option<StoreRecord>, RuntimeError> {
        let record = self
            .stores
            .get_record_with_policy(kind, store, key, policy)?;
        self.emit_store_event("get", kind, store, key, record.as_ref().map(|r| &r.value));
        Ok(record)
    }

    pub fn store_put_record(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let record = self.stores.put_record(kind, store, key, record)?;
        self.emit_store_event("put", kind, store, key, None);
        Ok(record)
    }

    pub fn store_put_record_if_revision(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
    ) -> Result<StoreRecord, RuntimeError> {
        let record = self.stores.put_record_if_revision(
            kind,
            store,
            key,
            expected_revision,
            record,
        )?;
        self.emit_store_event("put", kind, store, key, None);
        Ok(record)
    }

    pub async fn store_put_record_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: StoreRecord,
        policy: &StorePolicySet,
    ) -> Result<StoreRecord, RuntimeError> {
        self.approve_store_write_if_required(kind, store, key, &record, policy)
            .await?;
        self.store_put_record(kind, store, key, record)
    }

    pub async fn store_put_record_if_revision_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        expected_revision: u64,
        record: StoreRecord,
        policy: &StorePolicySet,
    ) -> Result<StoreRecord, RuntimeError> {
        self.approve_store_write_if_required(kind, store, key, &record, policy)
            .await?;
        self.store_put_record_if_revision(kind, store, key, expected_revision, record)
    }

    pub fn store_delete(&self, kind: StoreKind, store: &str, key: &str) -> Result<(), RuntimeError> {
        self.stores.delete(kind, store, key)?;
        self.emit_store_event("delete", kind, store, key, None);
        Ok(())
    }

    pub fn store_delete_with_policy(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        policy: &StorePolicySet,
    ) -> Result<(), RuntimeError> {
        self.stores.delete_with_policy(kind, store, key, policy)?;
        self.emit_store_event("delete", kind, store, key, None);
        Ok(())
    }

    async fn approve_store_write_if_required(
        &self,
        kind: StoreKind,
        store: &str,
        key: &str,
        record: &StoreRecord,
        policy: &StorePolicySet,
    ) -> Result<(), RuntimeError> {
        if !policy.approval_required {
            return Ok(());
        }
        let label = policy.approval_label.as_deref().unwrap_or("StoreWrite");
        self.approval_gate(
            label,
            vec![
                serde_json::json!(kind.as_str()),
                serde_json::json!(store),
                serde_json::json!(key),
                record.value.clone(),
            ],
        )
        .await
    }

    fn emit_store_event(
        &self,
        op: &str,
        kind: StoreKind,
        store: &str,
        key: &str,
        value: Option<&serde_json::Value>,
    ) {
        self.tracer.emit(TraceEvent::HostEvent {
            ts_ms: now_ms(),
            run_id: self.tracer.run_id().to_string(),
            name: "store".to_string(),
            payload: serde_json::json!({
                "op": op,
                "kind": kind.as_str(),
                "store": store,
                "key": key,
                "effect": if op == "get" { kind.read_effect() } else { kind.write_effect() },
                "hit": value.is_some(),
            }),
        });
    }
}
