use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ContractCommand {
    /// Print the canonical guarantee table.
    ///
    /// Default output is human-readable: one row per guarantee with
    /// id, kind, class (static / runtime-checked / out-of-scope),
    /// pipeline phase, and a one-line description. `--json` emits the
    /// full structured table including test references and (where
    /// applicable) the explicit `out_of_scope_reason` for non-defenses.
    /// The output is the single source of truth that `docs/core-semantics.md`
    /// is generated from in slice 35-D and that `corvid claim --explain`
    /// reports against in slice 35-I.
    List {
        /// Emit machine-readable JSON instead of the human-readable table.
        #[arg(long)]
        json: bool,
        /// Filter by class. Accepts `static`, `runtime_checked`, or
        /// `out_of_scope`. Repeatable; unspecified shows everything.
        #[arg(long, value_name = "CLASS")]
        class: Option<String>,
        /// Filter by kind (e.g. `approval`, `effect_row`, `grounded`,
        /// `budget`, `confidence`, `replay`, `provenance_trace`,
        /// `abi_descriptor`, `abi_attestation`, `platform`).
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
    },
    /// Regenerate `docs/core-semantics.md` from the canonical
    /// guarantee registry. Writes the rendered markdown to the given
    /// `OUTPUT` path (typically `docs/core-semantics.md`); CI fails on
    /// drift between the committed file and the live render, so this
    /// command is the only sanctioned way to evolve the spec doc when
    /// the registry changes.
    RegenDoc {
        /// Output path, e.g. `docs/core-semantics.md`.
        output: PathBuf,
    },
}
