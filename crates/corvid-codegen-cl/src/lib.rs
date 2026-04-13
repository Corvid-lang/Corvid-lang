//! Cranelift-based native codegen for Corvid.
//!
//! AOT-first: IR → relocatable object file → system linker →
//! `target/bin/<stem>[.exe]`. No JIT detour. The interpreter in
//! `corvid-vm` remains the oracle — slice-12a lowering is tested by
//! parity harness (`tests/parity.rs`) that runs every fixture through
//! both tiers and asserts identical results.
//!
//! Slice 12a supports Int-only, pure-computation agents (plus
//! recursive agent-to-agent calls). Everything else raises
//! `CodegenError::NotSupported` with the slice that adds it.
//!
//! Overflow policy: every `Int` arithmetic op uses Cranelift's
//! `sadd_overflow` / `ssub_overflow` / `smul_overflow` and branches to
//! a runtime handler (`corvid_runtime_overflow`, linked from the C
//! shim) on overflow. Division and modulo also trap on a zero divisor.
//! This matches the interpreter's `Arithmetic("integer overflow")`
//! semantics byte-for-byte.
//!
//! See `ARCHITECTURE.md` §4 (pipeline) and `ROADMAP.md` Phase 12.

#![forbid(unsafe_code)]

pub mod errors;
pub mod link;
pub mod lowering;
pub mod module;

pub use errors::{CodegenError, CodegenErrorKind};

use corvid_ir::IrFile;
use std::path::{Path, PathBuf};

/// Compile `ir` to a relocatable object file at `object_path`. If
/// `entry_agent_name` is provided, the object exports a `corvid_entry`
/// trampoline symbol pointing at that agent — the C shim's link target.
pub fn compile_to_object(
    ir: &IrFile,
    module_name: &str,
    object_path: &Path,
    entry_agent_name: Option<&str>,
) -> Result<(), CodegenError> {
    let mut module = module::make_host_object_module(module_name)?;
    let _func_ids = lowering::lower_file(ir, &mut module, entry_agent_name)?;
    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| CodegenError::cranelift(format!("object emit: {e}"), span_zero()))?;
    if let Some(parent) = object_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CodegenError::io(format!("create {}: {e}", parent.display())))?;
    }
    std::fs::write(object_path, bytes)
        .map_err(|e| CodegenError::io(format!("write {}: {e}", object_path.display())))?;
    Ok(())
}

/// Compile + link `ir` into a native binary at `bin_path`. Picks the
/// entry agent automatically: the sole agent, or the one named `main`
/// if multiple are present.
pub fn build_native_to_disk(
    ir: &IrFile,
    module_name: &str,
    bin_path: &Path,
) -> Result<PathBuf, CodegenError> {
    let entry = pick_entry_agent(ir)?;
    // Parameter-less entry only — argv decoding requires `String` and a
    // shim variant; both arrive together in slice 12h.
    if !entry.params.is_empty() {
        return Err(CodegenError::not_supported(
            format!(
                "entry agent `{}` has {} parameter(s); `corvid build --target=native` requires a parameter-less entry — parameterised entries arrive in slice 12h alongside argv handling",
                entry.name,
                entry.params.len()
            ),
            entry.span,
        ));
    }
    // Float entry-agent returns need a different print format in the C
    // shim (or a second shim variant). Defer to slice 12i alongside
    // argv decoding (when the shim already grows).
    if matches!(&entry.return_ty, corvid_types::Type::Float) {
        return Err(CodegenError::not_supported(
            format!(
                "entry agent `{}` returns `Float` — the C shim's `printf(\"%lld\")` only handles Int/Bool; Float entry returns arrive in slice 12i",
                entry.name
            ),
            entry.span,
        ));
    }
    // String entry-agent returns: the shim can't print a String pointer
    // meaningfully, and a String return would need its own print path.
    // Defer to slice 12i alongside argv decoding.
    if matches!(&entry.return_ty, corvid_types::Type::String) {
        return Err(CodegenError::not_supported(
            format!(
                "entry agent `{}` returns `String` — slice 12i adds the C-shim variant that decodes argv and prints non-Int returns",
                entry.name
            ),
            entry.span,
        ));
    }
    let out_bin = link::binary_path_for(
        bin_path.parent().unwrap_or(Path::new(".")),
        bin_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("program"),
    );
    let obj_dir = tempfile::Builder::new()
        .prefix("corvid-obj-")
        .tempdir()
        .map_err(|e| CodegenError::io(format!("tempdir: {e}")))?;
    let object_path = obj_dir.path().join(format!("{module_name}.o"));
    compile_to_object(ir, module_name, &object_path, Some(&entry.name))?;
    link::link_binary(&object_path, &entry.name, &out_bin)?;
    Ok(out_bin)
}

fn pick_entry_agent(ir: &IrFile) -> Result<&corvid_ir::IrAgent, CodegenError> {
    if ir.agents.is_empty() {
        return Err(CodegenError::not_supported(
            "no agents declared — compiled binaries need an entry agent",
            span_zero(),
        ));
    }
    if ir.agents.len() == 1 {
        return Ok(&ir.agents[0]);
    }
    if let Some(main) = ir.agents.iter().find(|a| a.name == "main") {
        return Ok(main);
    }
    Err(CodegenError::not_supported(
        format!(
            "multiple agents declared and none is named `main`; available: {}",
            ir.agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        span_zero(),
    ))
}

fn span_zero() -> corvid_ast::Span {
    corvid_ast::Span::new(0, 0)
}
