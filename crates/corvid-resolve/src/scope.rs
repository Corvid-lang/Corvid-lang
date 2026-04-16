//! Symbol table, scope stack, and binding types.

use corvid_ast::Span;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stable ID of a top-level declaration within a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefId(pub u32);

/// Stable ID of a local binding (parameter or `x = ...`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LocalId(pub u32);

/// A reference produced by the resolver for each identifier use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Binding {
    /// Refers to a top-level declaration.
    Decl(DefId),
    /// Refers to a local binding (parameter or assignment result).
    Local(LocalId),
    /// Refers to a built-in (type name or language-level sentinel).
    BuiltIn(BuiltIn),
}

/// Names that are always in scope without a user declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuiltIn {
    // Primitive types.
    Int,
    Float,
    String,
    Bool,
    Nothing,
    Result,
    Option,
    Weak,
    Ok,
    Err,
    Some,
    None,
    WeakNew,
    WeakUpgrade,
    // Structural sentinels (surface as Idents today; real variants later).
    Break,
    Continue,
    Pass,
}

/// Kind of top-level declaration, for error messages and later passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclKind {
    Import,
    Type,
    Tool,
    Prompt,
    Agent,
}

/// An entry in the file-level symbol table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclEntry {
    pub id: DefId,
    pub name: String,
    pub kind: DeclKind,
    pub span: Span,
}

/// File-level symbol table. Populated in resolver pass 1.
#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    entries: Vec<DeclEntry>,
    by_name: HashMap<String, DefId>,
    builtins: HashMap<String, BuiltIn>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut t = SymbolTable::default();
        t.register_builtins();
        t
    }

    fn register_builtins(&mut self) {
        self.builtins.insert("Int".into(), BuiltIn::Int);
        self.builtins.insert("Float".into(), BuiltIn::Float);
        self.builtins.insert("String".into(), BuiltIn::String);
        self.builtins.insert("Bool".into(), BuiltIn::Bool);
        self.builtins.insert("Nothing".into(), BuiltIn::Nothing);
        self.builtins.insert("Result".into(), BuiltIn::Result);
        self.builtins.insert("Option".into(), BuiltIn::Option);
        self.builtins.insert("Weak".into(), BuiltIn::Weak);
        self.builtins.insert("Ok".into(), BuiltIn::Ok);
        self.builtins.insert("Err".into(), BuiltIn::Err);
        self.builtins.insert("Some".into(), BuiltIn::Some);
        self.builtins.insert("None".into(), BuiltIn::None);
        self.builtins.insert("Weak::new".into(), BuiltIn::WeakNew);
        self.builtins
            .insert("Weak::upgrade".into(), BuiltIn::WeakUpgrade);
        self.builtins.insert("break".into(), BuiltIn::Break);
        self.builtins.insert("continue".into(), BuiltIn::Continue);
        self.builtins.insert("pass".into(), BuiltIn::Pass);
    }

    /// Insert a top-level declaration.
    ///
    /// Returns `Ok(DefId)` on success. On duplicate, returns `Err(first_span)`
    /// — the caller records the duplicate error and proceeds.
    pub fn declare(
        &mut self,
        name: &str,
        kind: DeclKind,
        span: Span,
    ) -> Result<DefId, Span> {
        if let Some(existing_id) = self.by_name.get(name) {
            let existing = &self.entries[existing_id.0 as usize];
            return Err(existing.span);
        }
        let id = DefId(self.entries.len() as u32);
        self.entries.push(DeclEntry {
            id,
            name: name.to_string(),
            kind,
            span,
        });
        self.by_name.insert(name.to_string(), id);
        Ok(id)
    }

    /// Allocate a fresh `DefId` for a declaration that lives in a
    /// scoped table (NOT the file-level by-name namespace). Used for
    /// Methods inside `extend T:` blocks — they share names
    /// across types (`Point.distance`, `Line.distance`) so they can't
    /// go in the global by-name table, but they still need stable
    /// identity for downstream IR + diagnostics. Caller is responsible
    /// for storing the (scope, name) → DefId mapping in their own
    /// side table.
    pub fn allocate_def(
        &mut self,
        name: &str,
        kind: DeclKind,
        span: Span,
    ) -> DefId {
        let id = DefId(self.entries.len() as u32);
        self.entries.push(DeclEntry {
            id,
            name: name.to_string(),
            kind,
            span,
        });
        id
    }

    pub fn lookup(&self, name: &str) -> Option<Binding> {
        if let Some(&id) = self.by_name.get(name) {
            return Some(Binding::Decl(id));
        }
        if let Some(&b) = self.builtins.get(name) {
            return Some(Binding::BuiltIn(b));
        }
        None
    }

    /// Look up the `DefId` for a top-level declaration by name.
    pub fn lookup_def(&self, name: &str) -> Option<DefId> {
        self.by_name.get(name).copied()
    }

    pub fn entries(&self) -> &[DeclEntry] {
        &self.entries
    }

    pub fn get(&self, id: DefId) -> &DeclEntry {
        &self.entries[id.0 as usize]
    }
}

/// Lexical scope of local bindings (parameters and `x = ...`).
#[derive(Debug, Clone, Default)]
pub struct LocalScope {
    locals: HashMap<String, LocalId>,
}

impl LocalScope {
    pub fn insert(&mut self, name: &str, id: LocalId) {
        // Shadowing allowed: later insertions overwrite earlier ones.
        self.locals.insert(name.to_string(), id);
    }

    pub fn lookup(&self, name: &str) -> Option<LocalId> {
        self.locals.get(name).copied()
    }
}
