//! Cranelift `ObjectModule` setup for the host target.
//!
//! AOT-first: we produce a relocatable object file (`.o` on Unix,
//! `.obj` on Windows) and hand it to the system linker. JIT is not on
//! the current native path.

use crate::errors::CodegenError;
use cranelift_codegen::isa::{self, CallConv};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_object::{ObjectBuilder, ObjectModule};

/// Construct an `ObjectModule` targeting the host triple with sensible
/// defaults: PIC, opt-level "speed", no verifier surprises.
pub fn make_host_object_module(module_name: &str) -> Result<ObjectModule, CodegenError> {
    let mut flag_builder = settings::builder();
    // Position-independent code — required by Mach-O and by most modern
    // Linux distros for executables.
    flag_builder
        .set("is_pic", "true")
        .map_err(|e| CodegenError::cranelift(format!("settings is_pic: {e}"), span()))?;
    flag_builder
        .set("opt_level", "speed")
        .map_err(|e| CodegenError::cranelift(format!("settings opt_level: {e}"), span()))?;
    // Keep the verifier on — we're in scaffolding; failing loud beats
    // shipping broken machine code.
    flag_builder
        .set("enable_verifier", "true")
        .map_err(|e| CodegenError::cranelift(format!("settings enable_verifier: {e}"), span()))?;
    // Preserve frame pointers so the cycle collector's stack walk can
    // follow RBP without needing
    // OS-specific unwind info (.pdata on Windows, .eh_frame on
    // Linux/macOS). Every refcounted-value-holding frame ends up
    // in the RBP chain, and for each frame we look up its return
    // PC in `corvid_stack_maps` (17c) to find live GC roots.
    //
    // Cost: ~1-2% runtime overhead from reserving RBP as the frame
    // pointer (Cranelift otherwise uses it as GPR). Acceptable
    // given the alternative (emitting + registering OS unwind info
    // at every function define) is a large scope expansion. A
    // future performance work can revisit once measurements warrant it.
    flag_builder
        .set("preserve_frame_pointers", "true")
        .map_err(|e| {
            CodegenError::cranelift(
                format!("settings preserve_frame_pointers: {e}"),
                span(),
            )
        })?;

    let flags = settings::Flags::new(flag_builder);
    let isa_builder = isa::lookup(target_lexicon::Triple::host())
        .map_err(|e| CodegenError::cranelift(format!("isa lookup: {e}"), span()))?;
    let isa = isa_builder
        .finish(flags)
        .map_err(|e| CodegenError::cranelift(format!("isa finish: {e}"), span()))?;

    let default_call_conv: CallConv = isa.default_call_conv();
    let _ = default_call_conv; // accessed via Signature construction later

    let builder = ObjectBuilder::new(
        isa,
        module_name.to_string(),
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| CodegenError::cranelift(format!("object builder: {e}"), span()))?;
    Ok(ObjectModule::new(builder))
}

fn span() -> corvid_ast::Span {
    corvid_ast::Span::new(0, 0)
}
