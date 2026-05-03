use super::*;
use crate::approvals::ProgrammaticApprover;
use crate::provenance::{ProvenanceChain, ProvenanceKind};
use crate::store::{StoreKind, StorePolicySet, StoreRecord};
use serde_json::json;
#[test]
fn runtime_store_api_persists_through_sqlite_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime-store.sqlite3");
    let runtime = Runtime::builder()
        .sqlite_store(&path)
        .expect("sqlite store")
        .build();

    runtime
        .store_put(
            StoreKind::Session,
            "Conversation",
            "thread-1",
            json!({"topic": "shipping"}),
        )
        .expect("put");
    assert_eq!(
        runtime
            .store_get(StoreKind::Session, "Conversation", "thread-1")
            .expect("get"),
        Some(json!({"topic": "shipping"}))
    );

    drop(runtime);
    let reopened = Runtime::builder()
        .sqlite_store(&path)
        .expect("sqlite store")
        .build();
    assert_eq!(
        reopened
            .store_get(StoreKind::Session, "Conversation", "thread-1")
            .expect("get after reopen"),
        Some(json!({"topic": "shipping"}))
    );
}

#[test]
fn runtime_store_record_api_preserves_provenance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime-store.sqlite3");
    let runtime = Runtime::builder()
        .sqlite_store(&path)
        .expect("sqlite store")
        .build();

    let written = runtime
        .store_put_record(
            StoreKind::Memory,
            "Profile",
            "user-1",
            StoreRecord::grounded(
                json!({"fact": "prefers morning updates"}),
                ProvenanceChain::with_retrieval("profile_search", 7),
            ),
        )
        .expect("put record");
    assert_eq!(written.revision, 1);

    let record = runtime
        .store_get_record(StoreKind::Memory, "Profile", "user-1")
        .expect("get record")
        .expect("record present");
    assert_eq!(record.value, json!({"fact": "prefers morning updates"}));
    let provenance = record.provenance.expect("provenance");
    assert_eq!(provenance.entries.len(), 1);
    assert_eq!(provenance.entries[0].kind, ProvenanceKind::Retrieval);
    assert_eq!(provenance.entries[0].name, "profile_search");
}

#[test]
fn runtime_store_record_api_rejects_stale_revision() {
    let runtime = Runtime::builder().build();
    let first = runtime
        .store_put_record(
            StoreKind::Memory,
            "Profile",
            "user-1",
            StoreRecord::plain(json!({"fact": "alpha"})),
        )
        .expect("put first");
    let second = runtime
        .store_put_record_if_revision(
            StoreKind::Memory,
            "Profile",
            "user-1",
            first.revision,
            StoreRecord::plain(json!({"fact": "beta"})),
        )
        .expect("put second");
    assert_eq!(second.revision, 2);

    let err = runtime
        .store_put_record_if_revision(
            StoreKind::Memory,
            "Profile",
            "user-1",
            first.revision,
            StoreRecord::plain(json!({"fact": "stale"})),
        )
        .expect_err("stale write must fail");
    match err {
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
}

#[test]
fn runtime_store_policy_ttl_expires_records_on_read() {
    let runtime = Runtime::builder().build();
    runtime
        .store_put(
            StoreKind::Session,
            "Conversation",
            "thread-1",
            json!({"topic": "shipping"}),
        )
        .expect("put");

    assert_eq!(
        runtime
            .store_get_record_with_policy(
                StoreKind::Session,
                "Conversation",
                "thread-1",
                &StorePolicySet::ttl_ms(0),
            )
            .expect("get with ttl"),
        None
    );
}

#[test]
fn runtime_store_policy_legal_hold_blocks_delete() {
    let runtime = Runtime::builder().build();
    runtime
        .store_put(
            StoreKind::Memory,
            "Profile",
            "user-1",
            json!({"fact": "protected"}),
        )
        .expect("put");

    let err = runtime
        .store_delete_with_policy(
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
}

#[tokio::test]
async fn runtime_store_policy_approval_required_allows_approved_write() {
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let policy = StorePolicySet {
        approval_required: true,
        approval_label: Some("RememberSensitiveFact".to_string()),
        ..StorePolicySet::default()
    };

    runtime
        .store_put_with_policy(
            StoreKind::Memory,
            "Profile",
            "user-1",
            json!({"fact": "sensitive"}),
            &policy,
        )
        .await
        .expect("approved write");
    assert_eq!(
        runtime
            .store_get(StoreKind::Memory, "Profile", "user-1")
            .expect("get"),
        Some(json!({"fact": "sensitive"}))
    );
}

#[tokio::test]
async fn runtime_store_policy_approval_required_blocks_denied_write() {
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();
    let policy = StorePolicySet {
        approval_required: true,
        approval_label: Some("RememberSensitiveFact".to_string()),
        ..StorePolicySet::default()
    };

    let err = runtime
        .store_put_with_policy(
            StoreKind::Memory,
            "Profile",
            "user-1",
            json!({"fact": "sensitive"}),
            &policy,
        )
        .await
        .expect_err("denied write");
    assert!(matches!(
        err,
        RuntimeError::ApprovalDenied { ref action } if action == "RememberSensitiveFact"
    ));
    assert_eq!(
        runtime
            .store_get(StoreKind::Memory, "Profile", "user-1")
            .expect("get"),
        None
    );
}

#[test]
fn runtime_store_policy_provenance_required_rejects_ungrounded_record() {
    let runtime = Runtime::builder().build();
    runtime
        .store_put(
            StoreKind::Memory,
            "Profile",
            "user-1",
            json!({"fact": "ungrounded"}),
        )
        .expect("put");
    let policy = StorePolicySet {
        provenance_required: true,
        ..StorePolicySet::default()
    };

    let err = runtime
        .store_get_record_with_policy(StoreKind::Memory, "Profile", "user-1", &policy)
        .expect_err("ungrounded read must fail");
    match err {
        RuntimeError::StorePolicyViolation { policy, .. } => {
            assert_eq!(policy, "provenance_required");
        }
        other => panic!("unexpected error: {other}"),
    }
}
