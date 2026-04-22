use std::sync::Arc;

use slotmap::{DefaultKey, Key, SlotMap};

pub struct AttestationStore<T> {
    label: &'static str,
    handles: SlotMap<DefaultKey, Arc<T>>,
}

impl<T> AttestationStore<T> {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            handles: SlotMap::with_key(),
        }
    }

    pub fn insert(&mut self, value: Arc<T>) -> u64 {
        self.handles.insert(value).data().as_ffi()
    }

    pub fn get(&self, handle: u64) -> Option<&Arc<T>> {
        self.handles.get(DefaultKey::from(slotmap::KeyData::from_ffi(handle)))
    }

    pub fn remove(&mut self, handle: u64) -> bool {
        self.handles
            .remove(DefaultKey::from(slotmap::KeyData::from_ffi(handle)))
            .is_some()
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn emit_debug_leak_warning(&self) {
        if !cfg!(debug_assertions) {
            return;
        }
        let leaked = self.handles.len();
        if leaked > 0 {
            eprintln!(
                "warning: {} shut down with {leaked} unreleased handle(s)",
                self.label
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AttestationStore;
    use std::sync::Arc;

    #[test]
    fn insert_get_remove_roundtrips() {
        let mut store = AttestationStore::new("test store");
        let handle = store.insert(Arc::new(42_u64));
        assert_eq!(store.get(handle).map(|value| **value), Some(42));
        assert!(store.remove(handle));
        assert!(store.get(handle).is_none());
    }

    #[test]
    fn stale_handle_fails_cleanly() {
        let mut store = AttestationStore::new("test store");
        let handle = store.insert(Arc::new(7_u64));
        assert!(store.remove(handle));
        assert!(!store.remove(handle));
    }
}
