//! Thin REPL session wrapper around one-shot name resolution.

use crate::resolver::{resolve, Resolved};
use corvid_ast::{AgentDecl, Decl, File};
use corvid_ast::Span;

#[derive(Debug, Clone, Default)]
pub struct ReplResolveSession {
    decls: Vec<Decl>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTurn {
    pub file: File,
    pub resolved: Resolved,
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
}
