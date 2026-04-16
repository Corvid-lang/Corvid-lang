//! Corvid proc-macros.
//!
//! Only surface today is `#[tool("name")]`. Applied to an `async fn` in
//! a user Rust crate, it generates the typed C-ABI bridge that
//! Cranelift-compiled Corvid code links against.
//!
//! # What the macro does
//!
//! Given:
//!
//! ```ignore
//! #[tool("get_order")]
//! async fn get_order(id: String) -> Order { ... }
//! ```
//!
//! it emits:
//!
//! 1. The user's `async fn` unchanged — other Rust code keeps calling
//!    it the normal way (used by the interpreter tier, by tests, and
//!    by any Rust code that happens to live in the same crate).
//! 2. A `#[no_mangle] pub extern "C" fn __corvid_tool_get_order(...)`
//!    whose signature uses `#[repr(C)]` Corvid ABI types (e.g.
//!    `CorvidString` in place of `String`, plain `i64` in place of an
//!    `i64` Corvid Int). The wrapper converts args into native Rust
//!    types, calls the user's `async fn` through `block_on` on the
//!    runtime's tokio handle, and converts the result back out.
//! 3. An `inventory::submit!` block registering a `ToolMetadata` entry
//!    so the runtime can build its effect-policy table and tracer
//!    registry at startup.
//!
//! # What the macro does NOT do
//!
//! - It doesn't decide which Rust types map to which Corvid ABI types
//!   — that mapping lives in `corvid-runtime::abi` and is shared with
//!   the codegen side. The macro just calls the conversion traits.
//! - It doesn't do error handling beyond what the user's `async fn`
//!   already does. Tools whose `async fn` returns `T` cannot fail at
//!   the Corvid level today — the macro does not support a
//!   `Result<T, E>` return path yet.
//! - It doesn't support sync `fn` (only `async fn`). Users wrap a
//!   sync body in `async { ... }` trivially; keeping the macro
//!   async-only means one codepath to test and maintain.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, LitStr, Pat, ReturnType, Type};

/// `#[tool("name")]` — mark an async fn as a Corvid tool implementation.
///
/// The string argument is the name the Corvid declaration references.
/// It does NOT have to match the Rust fn name — users are free to call
/// the Rust fn `get_order_impl` and register it for Corvid's
/// `get_order` — but keeping them aligned is recommended.
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let name_lit = parse_macro_input!(attr as LitStr);
    let fn_item = parse_macro_input!(item as ItemFn);
    match expand_tool(name_lit, fn_item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_tool(name_lit: LitStr, f: ItemFn) -> syn::Result<TokenStream2> {
    // `#[tool]` only accepts `async fn`. Sync fns
    // are wrappable in `async { ... }` trivially — rejecting them here
    // prevents accidental "my tool isn't async so it can't await the
    // LLM" foot-guns down the line.
    if f.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &f.sig.fn_token,
            "#[tool] requires `async fn` — wrap synchronous bodies in `async { ... }` if you don't need to await anything",
        ));
    }

    let tool_name = name_lit.value();
    let fn_ident = f.sig.ident.clone();
    // Wrapper symbol = `__corvid_tool_<tool-name>`. Using the declared
    // tool name (not the Rust fn name) keeps the linker lookup aligned
    // with Corvid source — codegen emits `call "__corvid_tool_<name>"`.
    let wrapper_ident = format_ident!(
        "__corvid_tool_{}",
        mangle_tool_name(&tool_name)
    );
    let wrapper_symbol = wrapper_ident.to_string();

    // Collect (native_type, abi_type, arg_name) for each parameter.
    let mut wrapper_params: Vec<TokenStream2> = Vec::new();
    let mut arg_conversions: Vec<TokenStream2> = Vec::new();
    let mut call_args: Vec<TokenStream2> = Vec::new();

    for (idx, input) in f.sig.inputs.iter().enumerate() {
        let (arg_name, native_ty) = match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "#[tool] fns can't take `self` — they're free functions, not methods",
                ));
            }
            FnArg::Typed(pt) => {
                let name = match &*pt.pat {
                    Pat::Ident(i) => i.ident.clone(),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "#[tool] fn parameters must be plain identifiers (not patterns)",
                        ));
                    }
                };
                let ty = (*pt.ty).clone();
                (name, ty)
            }
        };

        let abi_ty = abi_type_for(&native_ty)?;
        let native_ty_tokens = quote! { #native_ty };
        wrapper_params.push(quote! { #arg_name: #abi_ty });

        // Conversion: every Corvid ABI type implements `Into<NativeT>`
        // (defined in `corvid_runtime::abi`). The conversion may copy
        // (e.g. CorvidString -> String copies bytes) and may release
        // refcounts (CorvidString into conversion releases the
        // caller's +0 reference — per the +0 ABI the wrapper
        // retains on entry and releases here).
        arg_conversions.push(quote! {
            let #arg_name: #native_ty_tokens =
                ::corvid_runtime::abi::FromCorvidAbi::from_corvid_abi(#arg_name);
        });
        call_args.push(quote! { #arg_name });

        // `idx` unused today; kept to make future arity-diagnostics
        // straightforward.
        let _ = idx;
    }

    // Return type: `async fn` expands to a Future whose Output is the
    // declared return. Extract the Output type (or `()` for no return).
    let (return_is_unit, native_ret_ty) = match &f.sig.output {
        ReturnType::Default => (true, None),
        ReturnType::Type(_, ty) => (false, Some((**ty).clone())),
    };

    let wrapper_ret_ty = match &native_ret_ty {
        None => quote! { () },
        Some(ty) => {
            let abi = abi_type_for(ty)?;
            quote! { #abi }
        }
    };

    let arity = f.sig.inputs.len();

    let call_expr = if return_is_unit {
        quote! {
            __handle.block_on(async { #fn_ident(#(#call_args),*).await });
        }
    } else {
        quote! {
            let __result = __handle.block_on(async { #fn_ident(#(#call_args),*).await });
            ::corvid_runtime::abi::IntoCorvidAbi::into_corvid_abi(__result)
        }
    };

    let expanded = quote! {
        // 1. The user's async fn, unchanged.
        #f

        // 2. Typed C-ABI wrapper. Linker-visible symbol name is the
        //    literal `__corvid_tool_<mangled-name>`; codegen emits a
        //    direct call to this symbol.
        #[no_mangle]
        pub extern "C" fn #wrapper_ident(#(#wrapper_params),*) -> #wrapper_ret_ty {
            // Grab the tokio handle. Panics if `corvid_runtime_init`
            // hasn't run — contract is that main calls it first
            // whenever `ir_uses_runtime(ir)` returned true.
            let __handle = ::corvid_runtime::ffi_bridge::tokio_handle();
            #(#arg_conversions)*
            #call_expr
        }

        // 3. Metadata registration. `corvid_runtime_init` collects
        //    every entry at startup to build the effect-policy table.
        //    Never on the dispatch hot path.
        ::corvid_runtime::inventory::submit! {
            ::corvid_runtime::ToolMetadata {
                name: #tool_name,
                symbol: #wrapper_symbol,
                arity: #arity,
            }
        }
    };

    Ok(expanded)
}

/// Map a Rust type appearing in a `#[tool]` signature to its Corvid
/// ABI type. Tool signatures currently support scalar types plus
/// String; Struct and List at the tool ABI are not implemented yet.
fn abi_type_for(ty: &Type) -> syn::Result<TokenStream2> {
    let ts = quote! { #ty }.to_string().replace(' ', "");
    match ts.as_str() {
        "i64" => Ok(quote! { i64 }),
        "f64" => Ok(quote! { f64 }),
        "bool" => Ok(quote! { bool }),
        "String" => Ok(quote! { ::corvid_runtime::abi::CorvidString }),
        other => Err(syn::Error::new_spanned(
            ty,
            format!(
                "#[tool] signatures currently support only `i64` (Corvid Int), `f64` (Float), `bool` (Bool), and `String`. Got `{other}`. Struct/List arguments and returns are not implemented yet."
            ),
        )),
    }
}

/// The wrapper symbol name embeds the tool name. Tool names that aren't
/// valid C identifiers get their non-alphanumeric chars replaced with
/// underscores. Tool names are typically snake_case identifiers
/// anyway; mangling exists for robustness, not because anyone writes
/// a tool named `"with spaces!"`.
fn mangle_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
