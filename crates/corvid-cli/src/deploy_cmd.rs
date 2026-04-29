use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use corvid_abi::{load_signing_key, sign_envelope, KeySource};
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
    fs::write(out.join("oci-labels.json"), &metadata_json).context("write OCI metadata")?;
    fs::write(out.join("env.schema.json"), render_env_schema()).context("write env schema")?;
    fs::write(out.join("health.json"), render_health_config()).context("write health config")?;
    fs::write(out.join("migrate.sh"), render_migration_runner(app_name))
        .context("write migration runner")?;
    fs::write(
        out.join("startup-checks.md"),
        render_startup_checks(app_name),
    )
    .context("write startup checks")?;
    let attestation = render_attestation(app_name, &metadata_json)?;
    fs::write(out.join("build-attestation.dsse.json"), attestation)
        .context("write build attestation")?;
    fs::write(out.join("VERIFY.md"), render_verify_docs()).context("write verification docs")?;

    println!("deploy package: {}", out.display());
    println!("dockerfile: {}", out.join("Dockerfile").display());
    println!("oci metadata: {}", out.join("oci-labels.json").display());
    println!("env schema: {}", out.join("env.schema.json").display());
    println!("health config: {}", out.join("health.json").display());
    println!(
        "attestation: {}",
        out.join("build-attestation.dsse.json").display()
    );
    Ok(())
}

pub fn run_compose(app: &Path, out: &Path) -> Result<()> {
    let app_name = app
        .file_name()
        .and_then(|name| name.to_str())
        .context("app path must end in a valid directory name")?;
    fs::create_dir_all(out)
        .with_context(|| format!("create compose deploy dir `{}`", out.display()))?;
    fs::write(out.join("docker-compose.yml"), render_compose(app_name))
        .context("write docker-compose.yml")?;
    fs::write(out.join(".env.example"), render_compose_env(app_name))
        .context("write compose env")?;
    println!(
        "compose manifest: {}",
        out.join("docker-compose.yml").display()
    );
    println!("env example: {}", out.join(".env.example").display());
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

fn render_attestation(app_name: &str, metadata_json: &str) -> Result<String> {
    let signing_key = std::env::var("CORVID_DEPLOY_SIGNING_KEY")
        .context("CORVID_DEPLOY_SIGNING_KEY is required for deploy package attestation")?;
    let key = load_signing_key(&KeySource::Env(signing_key))
        .map_err(|err| anyhow::anyhow!("load deploy signing key: {err}"))?;
    let payload = format!(
        "{{\"schema\":\"corvid.deploy.attestation.v1\",\"app\":\"{app_name}\",\"oci\":{metadata_json}}}"
    );
    let envelope = sign_envelope(
        payload.as_bytes(),
        "application/vnd.corvid.deploy.attestation.v1+json",
        &key,
        "deploy-package",
    );
    serde_json::to_string_pretty(&envelope).context("serialize deploy attestation")
}

fn render_verify_docs() -> &'static str {
    r#"# Deploy Package Verification

`build-attestation.dsse.json` is a DSSE envelope over the package's OCI metadata.

Verification requirements:

- Payload type: `application/vnd.corvid.deploy.attestation.v1+json`
- Signing key source: `CORVID_DEPLOY_SIGNING_KEY` during packaging
- The payload's source SHA-256 must match `oci-labels.json`
- The image/app label must match the packaged app directory
"#
}

fn render_compose(app_name: &str) -> String {
    format!(
        r#"services:
  {app_name}:
    build:
      context: ../../..
      dockerfile: examples/backend/{app_name}/deploy/Dockerfile
    environment:
      CORVID_APP_ENV: local
      CORVID_CONNECTOR_MODE: mock
      CORVID_DATABASE_URL: sqlite:/data/{app_name}.db
      CORVID_TRACE_DIR: /data/traces
      CORVID_REQUIRE_APPROVALS: "true"
    ports:
      - "8080:8080"
    volumes:
      - {app_name}-data:/data
    healthcheck:
      test: ["CMD", "corvid", "check", "examples/backend/{app_name}/src/main.cor"]
      interval: 30s
      timeout: 10s
      retries: 3

volumes:
  {app_name}-data:
"#
    )
}

fn render_compose_env(app_name: &str) -> String {
    format!(
        r#"CORVID_APP_ENV=local
CORVID_CONNECTOR_MODE=mock
CORVID_DATABASE_URL=sqlite:target/{app_name}.db
CORVID_TRACE_DIR=target/traces
CORVID_REQUIRE_APPROVALS=true
"#
    )
}
