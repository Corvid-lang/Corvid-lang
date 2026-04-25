//! Corvid language-server core.
//!
//! Phase 24 starts with transport-independent analysis: callers provide an
//! open document snapshot and receive standard LSP diagnostics. The JSON-RPC
//! server, hover, completion, and workspace indexing build on this layer rather
//! than duplicating compiler calls.

mod analysis;
mod completion;
mod hover;
mod navigation;
mod position;
mod server;
mod transport;

pub use analysis::{analyze_document, AnalysisResult, DocumentSnapshot};
pub use completion::completion_at;
pub use hover::hover_at;
pub use navigation::{
    definition_range_at, references_at, rename_ranges_at, workspace_symbols_for_document,
};
pub use position::{byte_span_to_lsp_range, byte_to_lsp_position, lsp_position_to_byte};
pub use server::{LanguageServerState, ServerMessage};
pub use transport::{run_stdio_server, LspTransportError};
