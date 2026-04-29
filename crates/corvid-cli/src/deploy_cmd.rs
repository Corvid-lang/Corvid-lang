use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Serialize)]
struct OciMetadata<'a> {
    image: &'a str,
    labels: OciLabels<'a>,
}

#[derive(Serialize)]
struct OciLabels<'a> {
    #[serde(rename = "org.opencontainers.image.title")]
    title: &'a str,
    #[serde(rename = "org.opencontainers.image.source")]
    source: String,
    #[serde(rename = "dev.corvid.app")]
    app: &'a str,
    #[serde(rename = "dev.corvid.package.source_sha256")]
    source_sha256: String,
}

pub fn run_package(app: &Path, out: &Path) -> Result<()> {
    let app_name = app
        .file_name()
        .and_then(|name| name.to_str())
        .context("app path must end in a valid directory name")?;
    let source = app.join("src").join("main.cor");
    let source_bytes =
        fs::read(&source).with_context(|| format!("read app source `{}`", source.display()))?;
    fs::create_dir_all(out)
        .with_context(|| format!("create deploy package `{}`", out.display()))?;

    fs::write(out.join("Dockerfile"), render_dockerfile(app_name)).context("write Dockerfile")?;

    let source_sha256 = hex::encode(Sha256::digest(&source_bytes));
    let metadata = OciMetadata {
        image: app_name,
        labels: OciLabels {
            title: app_name,
            source: source.display().to_string(),
            app: app_name,
            source_sha256,
        },
    };
    let metadata_json =
        serde_json::to_string_pretty(&metadata).context("serialize OCI metadata")?;
    fs::write(out.join("oci-labels.json"), metadata_json).context("write OCI metadata")?;

    println!("deploy package: {}", out.display());
    println!("dockerfile: {}", out.join("Dockerfile").display());
    println!("oci metadata: {}", out.join("oci-labels.json").display());
    Ok(())
}

fn render_dockerfile(app_name: &str) -> String {
    format!(
        r#"FROM rust:1.78-slim AS build
WORKDIR /workspace
COPY . .
RUN cargo build -p corvid-cli --release

FROM debian:bookworm-slim
WORKDIR /workspace
LABEL org.opencontainers.image.title="{app_name}"
LABEL dev.corvid.app="{app_name}"
COPY --from=build /workspace/target/release/corvid /usr/local/bin/corvid
COPY examples/backend/{app_name} examples/backend/{app_name}
COPY std std
HEALTHCHECK --interval=30s --timeout=10s --retries=3 CMD corvid check examples/backend/{app_name}/src/main.cor
CMD ["corvid", "run", "examples/backend/{app_name}/src/main.cor"]
"#
    )
}
