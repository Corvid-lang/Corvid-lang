//! `corvid connectors` CLI subcommand surface — slice 41L,
//! decomposed in Phase 20j-S2.
//!
//! Wires the Phase 41 connector runtime into the `corvid` CLI.
//! Users get a top-level surface for inspecting available
//! connectors, validating their manifests, executing operations
//! against mock / replay / real modes, and managing OAuth2 token
//! lifecycle (PKCE init + force-refresh).
//!
//! Real-mode commands gate on `CORVID_PROVIDER_LIVE=1` so a
//! developer cannot accidentally fire a live request from a local
//! command — the same posture `ConnectorRuntime` enforces in code.
//! Webhook signature verification is implemented inline here as a
//! standalone primitive so a CI hook or `curl | corvid connectors
//! verify-webhook` pipeline can validate inbound payloads
//! independently of an HTTP server.
//!
//! The module is split per CLI surface (Phase 20j-S2):
//!
//! - [`list`] — `corvid connectors list` (read-only catalog).
//! - [`check`] — `corvid connectors check [--live]` (manifest
//!   validation; `--live` reserved for the drift narrator).
//! - [`run`] — `corvid connectors run` (mock/replay/real dispatch).
//! - [`oauth`] — `corvid connectors oauth init|rotate` (OAuth2
//!   PKCE init + token-refresh lifecycle).
//! - [`verify_webhook`] — `corvid connectors verify-webhook`
//!   (per-provider HMAC verification + raw fallback).
//! - `support` (private) — manifest catalog, runtime-error
//!   projection, list-row summary, OAuth2 PKCE primitives shared
//!   by every per-subcommand module.

pub mod check;
pub mod list;
pub mod oauth;
pub mod run;
mod support;
pub mod verify_webhook;

#[allow(unused_imports)]
pub use check::*;
#[allow(unused_imports)]
pub use list::*;
#[allow(unused_imports)]
pub use oauth::*;
#[allow(unused_imports)]
pub use run::*;
#[allow(unused_imports)]
pub use verify_webhook::*;
