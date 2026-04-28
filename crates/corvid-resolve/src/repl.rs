//! REPL session wrapper around name resolution, with support for
//! declaration redefinition and dependency-tracked invalidation.

use crate::depgraph::{build_dep_graph, decl_name, DepGraph};
use crate::resolver::{resolve, Resolved};
use crate::scope::DefId;
use corvid_ast::{AgentDecl, Decl, File, Span};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct ReplResolveSession {
    decls: Vec<Decl>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTurn {
    pub file: File,
    pub resolved: Resolved,
}

/// Result of a redefinition: what changed and what's affected.
#[derive(Debug, Clone)]
pub struct RedefinitionResult {
    pub name: String,
    pub old_kind: String,
    pub new_kind: String,
    pub replaced_def_id: DefId,
    pub affected: HashSet<DefId>,
    pub affected_names: Vec<String>,
    pub resolved: ResolvedTurn,
    pub dep_graph: DepGraph,
}

impl ReplResolveSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decls(&self) -> &[Decl] {
        &self.decls
    }

    pub fn resolve_current(&self) -> ResolvedTurn {
        let file = File {
            decls: self.decls.clone(),
            span: Span::new(0, 0),
        };
        let resolved = resolve(&file);
        ResolvedTurn { file, resolved }
    }

    pub fn resolve_decl_turn(&self, decl: Decl) -> ResolvedTurn {
        let mut decls = self.decls.clone();
        decls.push(decl);
        let file = File {
            decls,
            span: Span::new(0, 0),
        };
        let resolved = resolve(&file);
        ResolvedTurn { file, resolved }
    }

    pub fn resolve_agent_turn(&self, agent: AgentDecl) -> ResolvedTurn {
        let mut decls = self.decls.clone();
        decls.push(Decl::Agent(agent));
        let file = File {
            decls,
            span: Span::new(0, 0),
        };
        let resolved = resolve(&file);
        ResolvedTurn { file, resolved }
    }

    pub fn commit_decl(&mut self, decl: Decl) {
        self.decls.push(decl);
    }

    /// Check whether a declaration with the given name already exists
    /// in the session.
    pub fn has_decl(&self, name: &str) -> bool {
        self.decls.iter().any(|d| decl_name(d) == Some(name))
    }

    /// Replace a declaration and compute the dependency cascade.
    /// Returns the redefinition result with affected declarations,
    /// or None if the name doesn't exist (caller should use commit_decl).
    pub fn redefine_decl(&mut self, new_decl: Decl) -> Option<RedefinitionResult> {
        let name = decl_name(&new_decl)?.to_string();

        let old_index = self
            .decls
            .iter()
            .position(|d| decl_name(d) == Some(&name))?;
        let old_kind = decl_kind_name(&self.decls[old_index]);

        // Replace the old declaration in the list.
        self.decls[old_index] = new_decl.clone();

        // Re-resolve the full file with the replacement.
        let file = File {
            decls: self.decls.clone(),
            span: Span::new(0, 0),
        };
        let resolved = resolve(&file);

        // Build the dependency graph from the new resolution.
        let dep_graph = build_dep_graph(&file, &resolved);

        // Find the DefId of the redefined declaration.
        let replaced_def_id = resolved.symbols.lookup_def(&name)?;

        // Compute affected declarations (direct + transitive dependents).
        let affected = dep_graph.transitive_dependents(replaced_def_id);
        let affected_names: Vec<String> = affected
            .iter()
            .filter_map(|&id| {
                let entry = resolved.symbols.get(id);
                Some(entry.name.clone())
            })
            .collect();

        let new_kind = decl_kind_name(&self.decls[old_index]);
        let turn = ResolvedTurn { file, resolved };

        Some(RedefinitionResult {
            name,
            old_kind,
            new_kind,
            replaced_def_id,
            affected,
            affected_names,
            resolved: turn,
            dep_graph,
        })
    }

    /// Build the current dependency graph.
    pub fn dep_graph(&self) -> DepGraph {
        let turn = self.resolve_current();
        build_dep_graph(&turn.file, &turn.resolved)
    }
}

fn decl_kind_name(decl: &Decl) -> String {
    match decl {
        Decl::Import(_) => "import".into(),
        Decl::Type(_) => "type".into(),
        Decl::Store(store) => store.kind.as_str().into(),
        Decl::Tool(_) => "tool".into(),
        Decl::Prompt(_) => "prompt".into(),
        Decl::Agent(_) => "agent".into(),
        Decl::Eval(_) => "eval".into(),
        Decl::Test(_) => "test".into(),
        Decl::Fixture(_) => "fixture".into(),
        Decl::Mock(_) => "mock".into(),
        Decl::Extend(_) => "extend".into(),
        Decl::Effect(_) => "effect".into(),
        Decl::Model(_) => "model".into(),
        Decl::Server(_) => "server".into(),
    }
}
