//! Grounded-value attestations exposed at the FFI boundary.
//!
//! The attestation store is the owner of grounded handles: it tracks
//! provenance chains, confidence, and leak accounting behind a single
//! process-global slotmap. The C-ABI wrappers only translate integer
//! handles to these operations; they do not own any lifetime logic.

use crate::provenance::ProvenanceChain;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use slotmap::{DefaultKey, Key, SlotMap};

pub const NULL_GROUNDED_HANDLE: u64 = 0;

#[derive(Debug)]
pub struct GroundedAttestation {
    pub provenance: Arc<ProvenanceChain>,
    pub confidence: f64,
}

impl GroundedAttestation {
    pub fn source_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.provenance.entries {
            if !matches!(entry.kind, crate::provenance::ProvenanceKind::Retrieval) {
                continue;
            }
            if !out.iter().any(|name| name == &entry.name) {
                out.push(entry.name.clone());
            }
        }
        out
    }
}

impl Default for GroundedHandleStore {
    fn default() -> Self {
        Self {
            handles: SlotMap::with_key(),
            string_attestations: HashMap::new(),
        }
    }
}

pub fn attach_string_attestation(string_ptr: usize, attestation: Arc<GroundedAttestation>) {
    if string_ptr == 0 {
        return;
    }
    let mut store = store().lock().unwrap();
    store.string_attestations.insert(string_ptr, attestation);
}

pub fn set_last_scalar_attestation(attestation: Arc<GroundedAttestation>) {
    LAST_SCALAR_ATTESTATION.with(|slot| {
        *slot.borrow_mut() = Some(attestation);
    });
}

pub fn register_handle_for_string_ptr(string_ptr: usize) -> u64 {
    if string_ptr == 0 {
        return NULL_GROUNDED_HANDLE;
    }
    let mut store = store().lock().unwrap();
    let Some(attestation) = store.string_attestations.remove(&string_ptr) else {
        return NULL_GROUNDED_HANDLE;
    };
    store.handles.insert(attestation).data().as_ffi()
}

pub fn register_handle_for_last_scalar() -> u64 {
    LAST_SCALAR_ATTESTATION.with(|slot| {
        let Some(attestation) = slot.borrow_mut().take() else {
            return NULL_GROUNDED_HANDLE;
        };
        let mut store = store().lock().unwrap();
        store.handles.insert(attestation).data().as_ffi()
    })
}

pub fn sources_for_handle(handle: u64) -> Option<Vec<String>> {
    if handle == NULL_GROUNDED_HANDLE {
        return None;
    }
    let key = DefaultKey::from(slotmap::KeyData::from_ffi(handle));
    let store = store().lock().unwrap();
    store.handles.get(key).map(|attestation| attestation.source_names())
}

pub fn confidence_for_handle(handle: u64) -> Option<f64> {
    if handle == NULL_GROUNDED_HANDLE {
        return None;
    }
    let key = DefaultKey::from(slotmap::KeyData::from_ffi(handle));
    let store = store().lock().unwrap();
    store.handles.get(key).map(|attestation| attestation.confidence)
}

pub fn release_handle(handle: u64) -> bool {
    if handle == NULL_GROUNDED_HANDLE {
        return true;
    }
    let key = DefaultKey::from(slotmap::KeyData::from_ffi(handle));
    let mut store = store().lock().unwrap();
    store.handles.remove(key).is_some()
}

pub fn emit_debug_leak_warning() {
    if !cfg!(debug_assertions) {
        return;
    }
    let store = store().lock().unwrap();
    let leaked = store.handles.len();
    if leaked > 0 {
        eprintln!(
            "warning: grounded handle store shut down with {leaked} unreleased grounded handle(s)"
        );
    }
}
struct GroundedHandleStore {
    handles: SlotMap<DefaultKey, Arc<GroundedAttestation>>,
    string_attestations: HashMap<usize, Arc<GroundedAttestation>>,
}

static STORE: OnceLock<Mutex<GroundedHandleStore>> = OnceLock::new();

thread_local! {
    static LAST_SCALAR_ATTESTATION: RefCell<Option<Arc<GroundedAttestation>>> = const { RefCell::new(None) };
}

fn store() -> &'static Mutex<GroundedHandleStore> {
    STORE.get_or_init(|| Mutex::new(GroundedHandleStore::default()))
}

fn canonicalize_confidence(confidence: f64) -> f64 {
    if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

pub fn make_attestation(provenance: ProvenanceChain, confidence: f64) -> Arc<GroundedAttestation> {
    Arc::new(GroundedAttestation {
        provenance: Arc::new(provenance),
        confidence: canonicalize_confidence(confidence),
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ProvenanceChain;

    #[test]
    fn release_zero_is_noop() {
        assert!(release_handle(NULL_GROUNDED_HANDLE));
    }

    #[test]
    fn string_attestation_roundtrips_through_handle() {
        let attestation = make_attestation(ProvenanceChain::with_retrieval("lookup", 1), 0.75);
        attach_string_attestation(42, attestation);
        let handle = register_handle_for_string_ptr(42);
        assert_ne!(handle, NULL_GROUNDED_HANDLE);
        assert_eq!(sources_for_handle(handle).unwrap(), vec!["lookup".to_string()]);
        assert!((confidence_for_handle(handle).unwrap() - 0.75).abs() < 1e-9);
        assert!(release_handle(handle));
    }

    #[test]
    fn stale_handle_fails_after_release() {
        let attestation = make_attestation(ProvenanceChain::with_retrieval("lookup", 1), 1.0);
        attach_string_attestation(7, attestation);
        let handle = register_handle_for_string_ptr(7);
        assert!(release_handle(handle));
        assert!(!release_handle(handle));
    }

    #[test]
    fn scalar_attestation_roundtrips() {
        let attestation = make_attestation(ProvenanceChain::with_retrieval("classify", 2), 0.5);
        set_last_scalar_attestation(attestation);
        let handle = register_handle_for_last_scalar();
        assert_ne!(handle, NULL_GROUNDED_HANDLE);
        assert_eq!(sources_for_handle(handle).unwrap(), vec!["classify".to_string()]);
        assert!((confidence_for_handle(handle).unwrap() - 0.5).abs() < 1e-9);
        assert!(release_handle(handle));
    }
}
