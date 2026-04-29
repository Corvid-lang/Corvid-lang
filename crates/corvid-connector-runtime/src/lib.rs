pub mod auth;
pub mod calendar;
pub mod files;
pub mod github_real;
pub mod gmail;
pub mod gmail_real;
pub mod manifest;
pub mod ms365;
pub mod oauth2_refresh;
pub mod rate_limit;
pub mod real_client;
pub mod runtime;
pub mod slack;
pub mod slack_real;
pub mod tasks;
pub mod test_kit;
pub mod trace;

pub use auth::{ConnectorAuthError, ConnectorAuthState, ConnectorRefreshTokenState};
pub use calendar::{
    calendar_manifest, CalendarAvailabilityRequest, CalendarAvailabilitySlot,
    CalendarCancelRequest, CalendarConnector, CalendarEvent, CalendarEventReadRequest,
    CalendarWriteReceipt, CalendarWriteRequest, CALENDAR_CONNECTOR_MANIFEST,
};
pub use files::{
    file_manifest, FileConnector, FileIndexRequest, FileMetadata, FileReadRequest, FileSnippet,
    FileWriteKind, FileWriteReceipt, FileWriteRequest, FILE_CONNECTOR_MANIFEST,
};
pub use github_real::{
    github_pat_real_client, github_pat_real_client_with_base, GitHubEndpoints,
    StaticBearerResolver, GITHUB_API_BASE,
};
pub use gmail_real::{gmail_real_client, GmailEndpoints, GMAIL_API_BASE};
pub use slack_real::{slack_real_client, SlackEndpoints, SLACK_API_BASE};
pub use gmail::{
    gmail_manifest, GmailConnector, GmailDraftRequest, GmailMessageMetadata, GmailSearchRequest,
    GmailSendRequest, GmailWriteReceipt, GMAIL_CONNECTOR_MANIFEST,
};
pub use manifest::{
    parse_connector_manifest, validate_connector_manifest, ConnectorManifest,
    ConnectorManifestError, ConnectorMode, ConnectorReplayPolicy, ConnectorScope,
    ConnectorScopeApproval, ConnectorValidationReport, RateLimitDeclaration, RedactionRule,
    ReplayDeclaration,
};
pub use ms365::{
    ms365_manifest, Ms365CalendarEvent, Ms365CalendarEventsRequest, Ms365Connector,
    Ms365MailMessage, Ms365MailSearchRequest, MS365_CONNECTOR_MANIFEST,
};
pub use oauth2_refresh::{
    InMemoryOAuth2Store, OAuth2RefreshHook, OAuth2RefreshResolver, OAuth2TokenStore,
    OAuth2Tokens, ReqwestRefreshHook,
};
pub use rate_limit::{ConnectorRateLimit, ConnectorRateLimitDecision, ConnectorRateLimiter};
pub use real_client::{
    parse_retry_after_header, BearerTokenError, BearerTokenResolver, ConnectorRealClient,
    OperationEndpoints, RealCallContext, RealCallPlan, RefuseRealMode, ReqwestRealClient,
};
pub use runtime::{
    ConnectorRequest, ConnectorResponse, ConnectorRuntime, ConnectorRuntimeError,
    ConnectorRuntimeMode,
};
pub use slack::{
    slack_manifest, SlackConnector, SlackDraftRequest, SlackMessage, SlackReadRequest,
    SlackSendRequest, SlackThreadRequest, SlackWriteReceipt, SLACK_CONNECTOR_MANIFEST,
};
pub use tasks::{
    task_manifest, GitHubIssueSearchRequest, LinearIssueSearchRequest, TaskConnector, TaskIssue,
    TaskWriteKind, TaskWriteReceipt, TaskWriteRequest, TASK_CONNECTOR_MANIFEST,
};
pub use test_kit::{
    parse_connector_fixture, run_connector_fixture, ConnectorFixture, ConnectorFixtureReport,
};
pub use trace::ConnectorTraceEvent;
