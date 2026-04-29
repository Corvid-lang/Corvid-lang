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
    fs::write(out.join("env.schema.json"), render_env_schema()).context("write env schema")?;
    fs::write(out.join("health.json"), render_health_config()).context("write health config")?;
    fs::write(out.join("migrate.sh"), render_migration_runner(app_name))
        .context("write migration runner")?;
    fs::write(
        out.join("startup-checks.md"),
        render_startup_checks(app_name),
    )
    .context("write startup checks")?;

    println!("deploy package: {}", out.display());
    println!("dockerfile: {}", out.join("Dockerfile").display());
    println!("oci metadata: {}", out.join("oci-labels.json").display());
    println!("env schema: {}", out.join("env.schema.json").display());
    println!("health config: {}", out.join("health.json").display());
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

fn render_env_schema() -> &'static str {
    r#"{
  "required": {
    "CORVID_APP_ENV": "local|staging|production",
    "CORVID_CONNECTOR_MODE": "mock|replay|real",
    "CORVID_DATABASE_URL": "sqlite:<path> or postgres://...",
    "CORVID_TRACE_DIR": "writable trace directory",
    "CORVID_REQUIRE_APPROVALS": "true"
  }
}
"#
}

fn render_health_config() -> &'static str {
    r#"{
  "health": "/healthz",
  "readiness": "/readyz",
  "metrics": "/metrics",
  "startup_checks": ["env", "migrations", "approvals", "trace_dir"]
}
"#
}

fn render_migration_runner(app_name: &str) -> String {
    format!(
        r#"#!/usr/bin/env sh
set -eu
corvid migrate status --dir examples/backend/{app_name}/migrations --database "$CORVID_DATABASE_URL"
corvid migrate up --dir examples/backend/{app_name}/migrations --database "$CORVID_DATABASE_URL"
"#
    )
}

fn render_startup_checks(app_name: &str) -> String {
    format!(
        r#"# Startup Checks

- `corvid check examples/backend/{app_name}/src/main.cor`
- `corvid migrate status --dir examples/backend/{app_name}/migrations --database "$CORVID_DATABASE_URL"`
- `CORVID_REQUIRE_APPROVALS=true`
- `CORVID_TRACE_DIR` exists and is writable
- `CORVID_CONNECTOR_MODE` is explicitly set
"#
    )
}
