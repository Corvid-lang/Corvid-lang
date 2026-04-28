//! Cranelift-based native codegen for Corvid.
//!
//! AOT-first: IR → relocatable object file → system linker →
//! `target/bin/<stem>[.exe]`. No JIT detour. The interpreter in
//! `corvid-vm` remains the oracle — the parity harness
//! (`tests/parity.rs`) runs every fixture through both tiers and
//! asserts identical results.
//!
//! Overflow policy: every `Int` arithmetic op uses Cranelift's
//! `sadd_overflow` / `ssub_overflow` / `smul_overflow` and branches to
//! a runtime handler (`corvid_runtime_overflow`, linked from the C
//! shim) on overflow. Division and modulo also trap on a zero divisor.
//! This matches the interpreter's `Arithmetic("integer overflow")`
//! semantics byte-for-byte.
//!
//! See `ARCHITECTURE.md` §4 (pipeline).

#![forbid(unsafe_code)]

pub mod cdylib;
pub mod dataflow;
pub mod dup_drop;
pub mod errors;
pub mod latency_rc;
pub mod link;
pub mod lowering;
pub mod module;
pub mod ownership;
pub mod pair_elim;
pub mod scope_reduce;
pub mod target;

pub use errors::{CodegenError, CodegenErrorKind};
pub use target::BuildTarget;

use corvid_ir::IrFile;
use cranelift_module::{DataDescription, Linkage, Module};
use std::path::{Path, PathBuf};

/// Compile `ir` to a relocatable object file at `object_path`. If
/// `entry_agent_name` is provided, the object exports a `corvid_entry`
/// trampoline symbol pointing at that agent — the C shim's link target.
pub fn compile_to_object(
    ir: &IrFile,
    module_name: &str,
    object_path: &Path,
    entry_agent_name: Option<&str>,
    embedded_descriptor: Option<&[u8]>,
    embedded_attestation: Option<&[u8]>,
) -> Result<(), CodegenError> {
    // Run the ownership analysis pass before codegen.
    // Output is a transformed IrFile with borrow_sig populated on
    // every agent. Codegen downstream consults borrow_sig at call
    // sites to skip the callee-entry retain + scope-exit release
    // pair for Borrowed parameters.
    let (ir_analyzed, _summaries) = ownership::analyze(ir.clone());

    let mut module = module::make_host_object_module(module_name)?;
    let _func_ids = lowering::lower_file(&ir_analyzed, &mut module, entry_agent_name)?;
    if let Some(bytes) = embedded_descriptor {
        define_embedded_descriptor(&mut module, bytes)?;
    }
    if let Some(bytes) = embedded_attestation {
        define_embedded_attestation(&mut module, bytes)?;
    }
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
///
/// `extra_tool_libs` holds additional `.lib` / `.a` paths the linker
/// should pull in. Native tool dispatch uses this for the tool-implementation
/// staticlib: `cargo build` produces `libuser_tools.a`, the path goes
/// here, and the linker resolves `__corvid_tool_<name>` symbols
/// emitted by `IrCallKind::Tool` codegen. An empty list means no
/// user-provided tools — tool-using programs will fail to link with
/// an unresolved-symbol error, which is the correct outcome. The
/// driver surfaces a friendlier error earlier.
pub fn build_native_to_disk(
    ir: &IrFile,
    module_name: &str,
    bin_path: &Path,
    extra_tool_libs: &[&Path],
) -> Result<PathBuf, CodegenError> {
    let entry = pick_entry_agent(ir)?;
    // The native command-line boundary currently supports the four
    // scalar types. Structs and lists at the entry boundary remain
    // blocked until a dedicated serialization layer exists. The
    // codegen-emitted main itself enforces the same boundary; this
    // guard surfaces the error earlier with the agent name.
    for p in &entry.params {
        if matches!(
            &p.ty,
            corvid_types::Type::Struct(_) | corvid_types::Type::List(_)
        ) {
            return Err(CodegenError::not_supported(
                format!(
                    "entry agent `{}` parameter `{}` is `{}` — the native command-line boundary currently supports only Int/Bool/Float/String; structured input needs a dedicated serialization layer (use a wrapper agent that takes a String and parses internally)",
                    entry.name,
                    p.name,
                    p.ty.display_name()
                ),
                p.span,
            ));
        }
    }
    if matches!(
        &entry.return_ty,
        corvid_types::Type::Struct(_) | corvid_types::Type::List(_)
    ) {
        return Err(CodegenError::not_supported(
            format!(
                "entry agent `{}` returns `{}` — the native command-line boundary currently supports only Int/Bool/Float/String returns; structured output needs a dedicated serialization layer",
                entry.name,
                entry.return_ty.display_name()
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
    compile_to_object(ir, module_name, &object_path, Some(&entry.name), None, None)?;
    link::link_binary(&object_path, &entry.name, &out_bin, extra_tool_libs)?;
    Ok(out_bin)
}

pub fn build_library_to_disk(
    ir: &IrFile,
    module_name: &str,
    output_path: &Path,
    target: BuildTarget,
    extra_tool_libs: &[&Path],
    embedded_descriptor: Option<&[u8]>,
    embedded_attestation: Option<&[u8]>,
) -> Result<PathBuf, CodegenError> {
    cdylib::build_library_to_disk(
        ir,
        module_name,
        output_path,
        target,
        extra_tool_libs,
        embedded_descriptor,
        embedded_attestation,
    )
}

fn define_embedded_attestation(
    module: &mut cranelift_object::ObjectModule,
    bytes: &[u8],
) -> Result<(), CodegenError> {
    let data_id = module
        .declare_data(
            corvid_abi::CORVID_ABI_ATTESTATION_SYMBOL,
            Linkage::Export,
            false,
            false,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare embedded attestation: {e}"), span_zero())
        })?;
    let mut desc = DataDescription::new();
    desc.set_align(8);
    desc.define(bytes.to_vec().into_boxed_slice());
    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(format!("define embedded attestation: {e}"), span_zero())
    })
}

fn define_embedded_descriptor(
    module: &mut cranelift_object::ObjectModule,
    bytes: &[u8],
) -> Result<(), CodegenError> {
    let data_id = module
        .declare_data(
            corvid_abi::CORVID_ABI_DESCRIPTOR_SYMBOL,
            Linkage::Export,
            false,
            false,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare embedded descriptor: {e}"), span_zero())
        })?;
    let mut desc = DataDescription::new();
    desc.set_align(8);
    desc.define(bytes.to_vec().into_boxed_slice());
    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(format!("define embedded descriptor: {e}"), span_zero())
    })
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
