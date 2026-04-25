//! Corvid language-server core.
//!
//! Phase 24 starts with transport-independent analysis: callers provide an
//! open document snapshot and receive standard LSP diagnostics. The JSON-RPC
//! server, hover, completion, and workspace indexing build on this layer rather
//! than duplicating compiler calls.

mod analysis;
mod position;

pub use analysis::{analyze_document, AnalysisResult, DocumentSnapshot};
pub use position::{byte_span_to_lsp_range, byte_to_lsp_position};
