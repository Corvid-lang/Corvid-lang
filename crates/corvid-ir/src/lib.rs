//! Intermediate representation for the Corvid compiler.
//!
//! Post-typecheck, pre-codegen. Desugared, normalized, and carrying
//! resolved references plus attached types.
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

mod imports;
pub mod lower;
pub mod types;

pub use lower::{lower, lower_with_modules};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{Backoff, BackpressurePolicy, Effect};
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;

    fn lower_src(src: &str) -> IrFile {
        let tokens = lex(src).expect("lex");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
        let checked = typecheck(&file, &resolved);
        assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
        lower(&file, &resolved, &checked)
    }

    #[path = "lower_basic.rs"]
    mod lower_basic;

    #[path = "lower_effects.rs"]
    mod lower_effects;

    #[path = "lower_replay.rs"]
    mod lower_replay;

    #[path = "lower_imports.rs"]
    mod lower_imports;

    #[path = "types.rs"]
    mod types;
}
