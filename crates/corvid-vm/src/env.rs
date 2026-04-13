//! Lexical environment: `LocalId` → `Value`.
//!
//! v0.5 treats each function (agent/prompt) body as a single flat scope —
//! matching the resolver's current model. When we introduce closures,
//! this type gains a frame stack.

use crate::value::Value;
use corvid_resolve::LocalId;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Env {
    locals: HashMap<LocalId, Value>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            locals: HashMap::new(),
        }
    }

    /// Bind `id` to `value`. Shadows any prior binding at the same id
    /// (consistent with Corvid's assignment semantics).
    pub fn bind(&mut self, id: LocalId, value: Value) {
        self.locals.insert(id, value);
    }

    /// Return a clone of the value bound at `id`, or `None` if unbound.
    pub fn lookup(&self, id: LocalId) -> Option<Value> {
        self.locals.get(&id).cloned()
    }
}
