pub mod auth;
pub mod gmail;
pub mod manifest;
pub mod rate_limit;
pub mod runtime;
pub mod test_kit;
pub mod trace;

pub use auth::{ConnectorAuthError, ConnectorAuthState};
pub use gmail::{
    gmail_manifest, GmailConnector, GmailMessageMetadata, GmailSearchRequest,
    GMAIL_CONNECTOR_MANIFEST,
};
pub use manifest::{
    parse_connector_manifest, validate_connector_manifest, ConnectorManifest,
    ConnectorManifestError, ConnectorMode, ConnectorReplayPolicy, ConnectorScope,
    ConnectorScopeApproval, ConnectorValidationReport, RateLimitDeclaration, RedactionRule,
    ReplayDeclaration,
};
pub use rate_limit::{ConnectorRateLimit, ConnectorRateLimitDecision, ConnectorRateLimiter};
pub use runtime::{
    ConnectorRequest, ConnectorResponse, ConnectorRuntime, ConnectorRuntimeError,
    ConnectorRuntimeMode,
};
pub use test_kit::{
    parse_connector_fixture, run_connector_fixture, ConnectorFixture, ConnectorFixtureReport,
};
pub use trace::ConnectorTraceEvent;
