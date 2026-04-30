//! `corvid build` CLI dispatch — slice 13 / artifact emission
//! surface, decomposed in Phase 20j-A1.
//!
//! Two entry points:
//!
//! - [`cmd_build`] dispatches on `--target`
//!   (`python` / `wasm` / `native` / `cdylib` / `staticlib` /
//!   `typescript` / `server`) and writes the per-target
//!   artifact through `corvid_driver::build_*_to_disk` helpers.
//!   Optional flags emit a generated header file, the ABI
//!   descriptor, or a signed DSSE attestation.
//! - [`cmd_build_library`] is the shared helper for the
//!   library targets that write a `.so` / `.dll` / `.dylib`.

use anyhow::{Context, Result};
use corvid_driver::{
    build_native_to_disk, build_server_to_disk, build_target_to_disk, build_to_disk,
    build_wasm_to_disk, render_all_pretty, BuildTarget,
};
use std::path::Path;

pub(crate) fn cmd_build(
    file: &Path,
    target: &str,
    tools_lib: Option<&Path>,
    header: bool,
    abi_descriptor: bool,
    all_artifacts: bool,
    sign_key_path: Option<&Path>,
    key_id: Option<&str>,
) -> Result<u8> {
    let header = header || all_artifacts;
    let abi_descriptor = abi_descriptor || all_artifacts;
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    if sign_key_path.is_some() && target != "cdylib" {
        anyhow::bail!(
            "`--sign` is only valid for `--target=cdylib` — descriptor attestations are bound to the embedded cdylib descriptor symbol"
        );
    }
    let extra_libs_owned: Vec<&Path> = tools_lib.iter().copied().collect();
    match target {
        "python" | "py" => {
            if tools_lib.is_some() {
                anyhow::bail!("`--with-tools-lib` is only valid for `native`, `cdylib`, and `staticlib` targets");
            }
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_to_disk(file)
                .with_context(|| format!("failed to build `{}`", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "native" => {
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_native_to_disk(file, &extra_libs_owned)
                .with_context(|| format!("failed to build `{}` (native)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "server" => {
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_server_to_disk(file, &extra_libs_owned)
                .with_context(|| format!("failed to build `{}` (server)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                if let Some(handler) = out.handler_path {
                    println!("handler: {}", handler.display());
                }
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "wasm" => {
            if tools_lib.is_some() {
                anyhow::bail!(
                    "`--with-tools-lib` is not valid for `wasm` until the Phase 23 host-capability ABI lands"
                );
            }
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_wasm_to_disk(file)
                .with_context(|| format!("failed to build `{}` (wasm)", file.display()))?;
            if let Some(path) = out.wasm_path {
                println!("built: {} -> {}", file.display(), path.display());
                if let Some(js) = out.js_loader_path {
                    println!("loader: {}", js.display());
                }
                if let Some(types) = out.ts_types_path {
                    println!("types: {}", types.display());
                }
                if let Some(manifest) = out.manifest_path {
                    println!("manifest: {}", manifest.display());
                }
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "cdylib" => cmd_build_library(
            file,
            BuildTarget::Cdylib,
            &extra_libs_owned,
            header,
            abi_descriptor,
            sign_key_path,
            key_id,
        ),
        "staticlib" => {
            if abi_descriptor {
                anyhow::bail!(
                    "`--abi-descriptor` and `--all-artifacts` are only valid for `cdylib`"
                );
            }
            cmd_build_library(
                file,
                BuildTarget::Staticlib,
                &extra_libs_owned,
                header,
                false,
                None,
                None,
            )
        }
        other => {
            anyhow::bail!(
                "unknown target `{other}`; valid: `python` (default), `native`, `server`, `wasm`, `cdylib`, `staticlib`"
            )
        }
    }
}

fn cmd_build_library(
    file: &Path,
    target: BuildTarget,
    tools_libs: &[&Path],
    header: bool,
    abi_descriptor: bool,
    sign_key_path: Option<&Path>,
    key_id: Option<&str>,
) -> Result<u8> {
    // Resolve the signing key once at flag-parse time. The driver
    // stays string-typed; key parsing belongs at the CLI boundary
    // so failure modes surface with `--sign`'s context.
    let signing = match sign_key_path {
        Some(path) => {
            let key =
                corvid_abi::load_signing_key(&corvid_abi::KeySource::Path(path.to_path_buf()))
                    .with_context(|| format!("loading --sign key from `{}`", path.display()))?;
            Some(corvid_driver::SigningRequest {
                key,
                key_id: key_id.unwrap_or("build-key").to_string(),
            })
        }
        None => match std::env::var("CORVID_SIGNING_KEY") {
            Ok(value) if !value.is_empty() => {
                let key = corvid_abi::load_signing_key(&corvid_abi::KeySource::Env(value))
                    .context("loading signing key from CORVID_SIGNING_KEY env var")?;
                Some(corvid_driver::SigningRequest {
                    key,
                    key_id: key_id.unwrap_or("build-key").to_string(),
                })
            }
            _ => None,
        },
    };
    let out = build_target_to_disk(file, target, header, abi_descriptor, tools_libs, signing)
        .with_context(|| {
            format!(
                "failed to build `{}` ({})",
                file.display(),
                match target {
                    BuildTarget::Native => "native",
                    BuildTarget::Cdylib => "cdylib",
                    BuildTarget::Staticlib => "staticlib",
                }
            )
        })?;
    if let Some(path) = out.output_path {
        println!("built: {} -> {}", file.display(), path.display());
        if let Some(header_path) = out.header_path {
            println!("header: {}", header_path.display());
        }
        if let Some(abi_descriptor_path) = out.abi_descriptor_path {
            println!("abi descriptor: {}", abi_descriptor_path.display());
        }
        if out.signed {
            println!("attestation: signed (CORVID_ABI_ATTESTATION embedded)");
        }
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
        Ok(1)
    }
}

