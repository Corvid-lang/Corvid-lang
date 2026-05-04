//! `corvid run` CLI dispatch — slice 14 / program-execution
//! surface, decomposed in Phase 20j-A1.
//!
//! Dispatches on the runtime target:
//!
//! - `auto` (default): native AOT tier when the IR is tool-free
//!   and uses only supported command-line boundary types, or when
//!   tool-using code has a companion tools staticlib provided;
//!   interpreter otherwise (with a stderr notice).
//! - `native`: forces the native AOT tier; errors if the program
//!   is not eligible.
//! - `interp` / `interpreter`: forces the interpreter tier.

use anyhow::{Context, Result};
use corvid_driver::{load_corvid_config_for, run_with_target, RunTarget};
use std::path::Path;

pub(crate) fn cmd_run(file: &Path, target: &str, tools_lib: Option<&Path>) -> Result<u8> {
    let configured_target;
    let target = if target == "auto" {
        configured_target = load_corvid_config_for(file)
            .and_then(|config| config.run.target)
            .unwrap_or_else(|| "auto".to_string());
        configured_target.as_str()
    } else {
        target
    };

    let rt = match target {
        "auto" => RunTarget::Auto,
        "native" => RunTarget::Native,
        "interp" | "interpreter" => RunTarget::Interpreter,
        other => anyhow::bail!(
            "unknown target `{other}`; valid: `auto` (default), `native`, `interpreter`"
        ),
    };
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    // Auto: native AOT tier when the IR is tool-free and uses only
    // supported command-line boundary types, or when tool-using code
    // has a companion tools staticlib provided.
    // Interpreter otherwise (with a stderr notice). Native-required
    // and interpreter-forced are the explicit overrides. See
    // `RunTarget` docs in corvid-driver for the exact semantics.
    run_with_target(file, rt, tools_lib)
        .with_context(|| format!("failed to run `{}`", file.display()))
}
