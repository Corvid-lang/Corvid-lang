//! Source-splice substrate for delta-level isolation replay.
//!
//! Step 3c/N of `21-inv-H-5-stacked` builds minimal-causal-set
//! attribution on top of isolation replay: to prove *which* deltas
//! in a commit minimally reproduce a trace divergence, the engine
//! synthesizes source that differs from the commit's parent by
//! exactly a chosen subset of deltas, compiles it, and replays.
//! This module is the substrate underneath that work — the
//! primitive that takes a base source and a list of localized
//! edits, validates they don't overlap, and produces the
//! synthesized output.
//!
//! **This commit is the infrastructure only.** No delta classes
//! are implemented yet; the per-class splice computation lands in
//! subsequent `3c-annotations` / `3c-structural` / `3c-annotation-
//! or-derived` commits once the splice substrate is stable and
//! tested in isolation.
//!
//! Design principles (CTO-locked):
//!
//! - **Hybrid text/AST**. This substrate operates on byte ranges
//!   in source text. AST analysis lives in per-class isolators
//!   (future commits); they produce [`SpliceOp`] values, this
//!   module applies them. Region-level byte fidelity is
//!   preserved by construction — we don't re-emit anything the
//!   splice didn't touch.
//!
//! - **Overlap is a hard error**, not a last-write-wins. Two
//!   splices whose ranges overlap produce ambiguous output and
//!   signal a bug in whichever per-class isolator generated
//!   them. Caller handles via the `confidence:
//!   NonCompilingSubsetsSkipped` attribution path (subset skipped,
//!   attribution continues).
//!
//! - **Apply in reverse.** Splices are applied by `range.start`
//!   descending so earlier edits don't shift later ones' byte
//!   offsets. Output is byte-identical regardless of input order
//!   as long as ranges are non-overlapping.

use std::fmt;
use std::ops::Range;

use corvid_ast::File;

mod annotations;
mod ast_helpers;
mod structural;

/// One localized edit in source text. Per-class isolators compute
/// one or more of these per delta; the full set for an isolation
/// subset is aggregated and handed to [`apply_splices`].
#[derive(Debug, Clone)]
pub(crate) struct SpliceOp {
    /// Byte range in the parent source to replace. Empty ranges
    /// (`start == end`) are valid and represent pure insertions
    /// at that byte offset.
    pub range: Range<usize>,
    /// Replacement text. Empty string is valid and represents
    /// pure deletion of `range`.
    pub replacement: String,
}

/// Errors from [`apply_splices`]. Each variant carries enough
/// context for the caller's attribution record to explain why a
/// subset was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpliceError {
    /// Two splices' byte ranges overlap — ambiguous merge.
    /// Indicates a bug in the per-class isolator that produced
    /// them (or in the class-dispatch logic that composed a
    /// subset). Attribution for this subset surfaces as
    /// `NonCompilingSubsetsSkipped`.
    Overlap {
        first_range: Range<usize>,
        second_range: Range<usize>,
    },
    /// A splice's range extends past the source's end.
    OutOfBounds {
        range: Range<usize>,
        source_len: usize,
    },
    /// A splice's range is inverted (`start > end`).
    InvertedRange {
        range: Range<usize>,
    },
    /// A splice's range start or end lands in the middle of a
    /// UTF-8 multi-byte character boundary. Would produce invalid
    /// UTF-8 on replace_range.
    NotCharBoundary {
        offset: usize,
    },
}

impl fmt::Display for SpliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpliceError::Overlap {
                first_range,
                second_range,
            } => write!(
                f,
                "splice ranges overlap: {first_range:?} and {second_range:?}"
            ),
            SpliceError::OutOfBounds { range, source_len } => write!(
                f,
                "splice range {range:?} extends past source length {source_len}"
            ),
            SpliceError::InvertedRange { range } => {
                write!(f, "splice range {range:?} is inverted (start > end)")
            }
            SpliceError::NotCharBoundary { offset } => write!(
                f,
                "splice offset {offset} falls inside a UTF-8 multi-byte character"
            ),
        }
    }
}

impl std::error::Error for SpliceError {}

/// Apply a set of splice ops to source text, producing synthesized
/// output. Validates each splice is well-formed (non-inverted, in
/// bounds, on char boundaries) and that no two splices overlap
/// before applying anything — either all splices land or none do,
/// so a bad subset can't leave the output half-edited.
///
/// Ordering: splices apply by `range.start` *descending*, which
/// means earlier byte offsets aren't shifted by later-indexed
/// edits. Output is a pure function of `(source, splices)`; the
/// input order of the `splices` vec doesn't affect the result.
pub(crate) fn apply_splices(
    source: &str,
    mut splices: Vec<SpliceOp>,
) -> Result<String, SpliceError> {
    if splices.is_empty() {
        return Ok(source.to_string());
    }

    // Validate each splice independently first.
    for splice in &splices {
        if splice.range.start > splice.range.end {
            return Err(SpliceError::InvertedRange {
                range: splice.range.clone(),
            });
        }
        if splice.range.end > source.len() {
            return Err(SpliceError::OutOfBounds {
                range: splice.range.clone(),
                source_len: source.len(),
            });
        }
        if !source.is_char_boundary(splice.range.start) {
            return Err(SpliceError::NotCharBoundary {
                offset: splice.range.start,
            });
        }
        if !source.is_char_boundary(splice.range.end) {
            return Err(SpliceError::NotCharBoundary {
                offset: splice.range.end,
            });
        }
    }

    // Sort by start position ascending so we can window-scan for
    // overlaps. Empty ranges (insertions) at the same offset are
    // ambiguous — treat as overlap since caller's intent is
    // unclear.
    splices.sort_by_key(|s| s.range.start);
    for window in splices.windows(2) {
        if window[0].range.end > window[1].range.start {
            return Err(SpliceError::Overlap {
                first_range: window[0].range.clone(),
                second_range: window[1].range.clone(),
            });
        }
        // Two zero-width insertions at the same offset — order
        // between them is undefined; reject as ambiguous.
        if window[0].range.start == window[1].range.start
            && window[0].range.end == window[0].range.start
            && window[1].range.end == window[1].range.start
        {
            return Err(SpliceError::Overlap {
                first_range: window[0].range.clone(),
                second_range: window[1].range.clone(),
            });
        }
    }

    // Apply in reverse so earlier splices' offsets aren't shifted
    // by later (higher-offset) ones.
    let mut result = source.to_string();
    for splice in splices.iter().rev() {
        result.replace_range(splice.range.clone(), &splice.replacement);
    }
    Ok(result)
}

// ---------------------------------------------------------------
// Per-class dispatch
// ---------------------------------------------------------------

/// A prepared source input for isolation: the original text plus
/// its parsed AST. Per-class isolators walk the AST to find the
/// node they care about and consult the source to copy text
/// ranges.
pub(crate) struct IsolationInput {
    pub source: String,
    pub file: File,
}

/// Errors from the isolation pipeline. Each variant carries
/// enough context for the caller's attribution record to
/// surface a specific reason for a non-minimal subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IsolationError {
    /// Lexing or parsing the input source failed. Attribution
    /// falls back to commit-level (`confidence: CommitLevel`).
    ParseFailure(Vec<String>),
    /// Delta-key prefix isn't handled by any registered class
    /// yet. Expected: will become rarer as more sub-commits of
    /// step 3c/N ship classes. Surfaces as `CommitLevel`.
    UnsupportedDeltaClass(String),
    /// Delta-key shape is malformed relative to its declared
    /// class. Indicates a bug in either the emitter or the
    /// dispatch — not a user error.
    MalformedDeltaKey { detail: String },
    /// Agent named in the delta key isn't present in the
    /// corresponding AST. Happens when the delta's lifecycle
    /// pair (e.g., `agent.added` / `agent.removed`) is expected
    /// in a side this isolator didn't know to check.
    AgentNotFound {
        name: String,
        /// `"parent"` or `"commit"` — which AST was missing it.
        side: &'static str,
    },
    /// The source state this isolator expected (e.g., a
    /// `@replayable` attribute on an agent) isn't present. Means
    /// either the delta key says one thing and source disagrees
    /// (emitter bug) or the isolator is looking in the wrong
    /// place.
    ExpectedStateNotFound { detail: String },
    /// Cross-case handling not yet implemented in this class.
    /// Attribution surfaces as `NonCompilingSubsetsSkipped`
    /// (commit-level fallback for this delta), not a failure of
    /// the whole attribution run.
    UnsupportedCrossCase { detail: String },
    /// Splice application failed — usually overlap with another
    /// isolator's output in the same subset.
    Splice(SpliceError),
}

impl fmt::Display for IsolationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IsolationError::ParseFailure(errs) => {
                write!(f, "source failed to parse: {} error(s)", errs.len())
            }
            IsolationError::UnsupportedDeltaClass(key) => {
                write!(f, "no isolator handles delta key `{key}`")
            }
            IsolationError::MalformedDeltaKey { detail } => {
                write!(f, "malformed delta key: {detail}")
            }
            IsolationError::AgentNotFound { name, side } => {
                write!(f, "agent `{name}` not found in {side} AST")
            }
            IsolationError::ExpectedStateNotFound { detail } => write!(f, "{detail}"),
            IsolationError::UnsupportedCrossCase { detail } => write!(f, "{detail}"),
            IsolationError::Splice(err) => write!(f, "splice application failed: {err}"),
        }
    }
}

impl std::error::Error for IsolationError {}

impl From<SpliceError> for IsolationError {
    fn from(err: SpliceError) -> Self {
        IsolationError::Splice(err)
    }
}

/// Parse source text into an `IsolationInput` ready for per-class
/// isolators to walk. Lexing and parse errors bubble up as
/// `ParseFailure` with the error messages flattened — callers
/// typically log and fall back to commit-level attribution.
pub(crate) fn prepare_isolation_input(source: &str) -> Result<IsolationInput, IsolationError> {
    let tokens = corvid_syntax::lex(source).map_err(|errs| {
        IsolationError::ParseFailure(errs.iter().map(|e| format!("{e:?}")).collect())
    })?;
    let (file, parse_errors) = corvid_syntax::parse_file(&tokens);
    if !parse_errors.is_empty() {
        return Err(IsolationError::ParseFailure(
            parse_errors.iter().map(|e| format!("{e:?}")).collect(),
        ));
    }
    Ok(IsolationInput {
        source: source.to_string(),
        file,
    })
}

/// Dispatch to the per-class isolator that handles `delta_key`.
/// Returns splice ops to be composed by the caller into a full
/// subset splice list.
pub(crate) fn compute_splices_for_delta(
    delta_key: &str,
    parent: &IsolationInput,
    commit: &IsolationInput,
) -> Result<Vec<SpliceOp>, IsolationError> {
    if delta_key.starts_with("agent.replayable_gained:")
        || delta_key.starts_with("agent.replayable_lost:")
    {
        return annotations::isolate_replayable(delta_key, parent, commit);
    }
    if delta_key.starts_with("agent.added:") || delta_key.starts_with("agent.removed:") {
        return structural::isolate_lifecycle(delta_key, parent, commit);
    }
    Err(IsolationError::UnsupportedDeltaClass(delta_key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splice(start: usize, end: usize, replacement: &str) -> SpliceOp {
        SpliceOp {
            range: start..end,
            replacement: replacement.to_string(),
        }
    }

    // -----------------------------------------------------------
    // Happy paths
    // -----------------------------------------------------------

    #[test]
    fn empty_splices_returns_source_unchanged() {
        let out = apply_splices("hello world", Vec::new()).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn single_replacement_mid_source() {
        let out = apply_splices("hello world", vec![splice(6, 11, "rust")]).unwrap();
        assert_eq!(out, "hello rust");
    }

    #[test]
    fn single_pure_insertion() {
        let out = apply_splices("hello", vec![splice(0, 0, "Well, ")]).unwrap();
        assert_eq!(out, "Well, hello");
    }

    #[test]
    fn single_pure_deletion() {
        let out = apply_splices("hello, world", vec![splice(5, 7, "")]).unwrap();
        assert_eq!(out, "helloworld");
    }

    #[test]
    fn multi_splice_non_overlapping_applied_correctly() {
        // Replace "hello" with "hi" at start, "world" with "rust"
        // at end. Order of input vec shouldn't matter.
        let out = apply_splices(
            "hello, world",
            vec![splice(7, 12, "rust"), splice(0, 5, "hi")],
        )
        .unwrap();
        assert_eq!(out, "hi, rust");
    }

    #[test]
    fn insert_at_source_end_is_allowed() {
        let source = "hello";
        let out = apply_splices(source, vec![splice(5, 5, "!")]).unwrap();
        assert_eq!(out, "hello!");
    }

    #[test]
    fn input_order_does_not_affect_output() {
        let source = "abcdefghij";
        // Splice A at bytes 1..2, B at bytes 5..6, C at bytes 8..9.
        let splice_a = splice(1, 2, "X");
        let splice_b = splice(5, 6, "Y");
        let splice_c = splice(8, 9, "Z");
        let a_b_c = apply_splices(
            source,
            vec![splice_a.clone(), splice_b.clone(), splice_c.clone()],
        )
        .unwrap();
        let c_a_b = apply_splices(
            source,
            vec![splice_c.clone(), splice_a.clone(), splice_b.clone()],
        )
        .unwrap();
        let b_c_a = apply_splices(source, vec![splice_b, splice_c, splice_a]).unwrap();
        assert_eq!(a_b_c, "aXcdeYghZj");
        assert_eq!(a_b_c, c_a_b);
        assert_eq!(a_b_c, b_c_a);
    }

    // -----------------------------------------------------------
    // Byte-stability: unchanged regions preserved exactly
    // -----------------------------------------------------------

    #[test]
    fn unchanged_prefix_and_suffix_preserved_byte_for_byte() {
        let source = "AAABBBCCC";
        let out = apply_splices(source, vec![splice(3, 6, "XX")]).unwrap();
        assert_eq!(out, "AAAXXCCC");
        assert_eq!(&out[0..3], "AAA");
        assert_eq!(&out[out.len() - 3..], "CCC");
    }

    #[test]
    fn whitespace_outside_splice_preserved_byte_for_byte() {
        // Splice touches only a non-whitespace region; tabs +
        // newlines around it must come through identically.
        let source = "  \n\t  agent foo() {}\n\t  ";
        let out = apply_splices(source, vec![splice(6, 11, "bar")]).unwrap();
        assert_eq!(out, "  \n\t  bar foo() {}\n\t  ");
    }

    // -----------------------------------------------------------
    // Overlap + validation errors
    // -----------------------------------------------------------

    #[test]
    fn overlap_detected_and_rejected() {
        let result = apply_splices(
            "abcdef",
            vec![splice(1, 3, "X"), splice(2, 4, "Y")],
        );
        matches!(result, Err(SpliceError::Overlap { .. }));
    }

    #[test]
    fn identical_ranges_detected_as_overlap() {
        let result = apply_splices(
            "abcdef",
            vec![splice(1, 3, "X"), splice(1, 3, "Y")],
        );
        matches!(result, Err(SpliceError::Overlap { .. }));
    }

    #[test]
    fn two_zero_width_insertions_at_same_offset_detected_as_overlap() {
        // Ambiguous order → overlap error, per module docs.
        let result = apply_splices(
            "abcdef",
            vec![splice(2, 2, "X"), splice(2, 2, "Y")],
        );
        matches!(result, Err(SpliceError::Overlap { .. }));
    }

    #[test]
    fn adjacent_splices_touching_at_boundary_allowed() {
        // Splice A: 1..3, splice B: 3..5. No overlap; edge touches
        // but no byte is claimed twice.
        let out = apply_splices(
            "abcdef",
            vec![splice(1, 3, "X"), splice(3, 5, "Y")],
        )
        .unwrap();
        assert_eq!(out, "aXYf");
    }

    #[test]
    fn zero_width_insertion_adjacent_to_deletion_allowed() {
        let out = apply_splices(
            "abcdef",
            vec![splice(2, 2, "X"), splice(2, 4, "")],
        )
        .unwrap();
        assert_eq!(out, "abXef");
    }

    #[test]
    fn out_of_bounds_range_rejected() {
        let result = apply_splices("abc", vec![splice(0, 10, "X")]);
        matches!(result, Err(SpliceError::OutOfBounds { .. }));
    }

    #[test]
    fn inverted_range_rejected() {
        let result = apply_splices("abc", vec![splice(3, 1, "X")]);
        matches!(result, Err(SpliceError::InvertedRange { .. }));
    }

    #[test]
    fn non_char_boundary_rejected() {
        // "é" is two bytes in UTF-8 (0xC3 0xA9). Offset 1 is
        // inside the char and must be rejected.
        let source = "café";
        let result = apply_splices(source, vec![splice(4, 5, "X")]);
        matches!(result, Err(SpliceError::NotCharBoundary { .. }));
    }

    // -----------------------------------------------------------
    // Determinism
    // -----------------------------------------------------------

    #[test]
    fn same_input_produces_byte_identical_output_across_runs() {
        let source = "some source with multiple edits applied";
        let splices = || {
            vec![
                splice(5, 11, "SOURCE"),
                splice(17, 25, "MULTIPLE"),
                splice(30, 37, "APPLIED"),
            ]
        };
        let a = apply_splices(source, splices()).unwrap();
        let b = apply_splices(source, splices()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn splice_all_of_source_produces_replacement_only() {
        let source = "original";
        let out = apply_splices(source, vec![splice(0, 8, "replaced")]).unwrap();
        assert_eq!(out, "replaced");
    }

    #[test]
    fn splice_error_implements_display_for_actionable_messages() {
        let err = SpliceError::Overlap {
            first_range: 0..5,
            second_range: 3..8,
        };
        let msg = format!("{err}");
        assert!(msg.contains("overlap"));
        // Error carries both ranges so a remediation narrative can
        // point at the specific conflicting splices.
        assert!(msg.contains("..5"));
        assert!(msg.contains("3.."));
    }
}
