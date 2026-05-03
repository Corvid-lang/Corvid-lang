//! Top-level clap argument tree — `Cli` struct + `Command` enum
//! + every secondary `*Command` subcommand enum that wasn't
//! extracted to a per-group submodule.
//!
//! Per the Phase 20j-A1 plan, this file's responsibility is "the
//! whole CLI argument tree" — which is itself a grab-bag (17
//! enums covering disjoint command groups). The Tier B audit
//! candidate `cli/root.rs` notes call out splitting these
//! per-group as follow-up; for now they live together so
//! `corvid-cli/src/main.rs` can collapse to its entry-point
//! shape (slice 20j-A1 commit 12).
//!
//! Per-group arg-tree submodules already extracted in earlier
//! commits: `cli::jobs`, `cli::migrate`, `cli::observe`,
//! `cli::package`. The 16 remaining enums in this file are
//! candidates for the same per-group treatment in Tier B.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub use super::approvals::ApprovalsCommand;
pub use super::auth::{AuthCommand, AuthKeysCommand};
pub use super::bench::BenchCommand;
pub use super::claim::ClaimCommand;
pub use super::connectors::{ConnectorsCommand, ConnectorsOauthCommand};
pub use super::contract::ContractCommand;
pub use super::deploy::DeployCommand;
use super::jobs::JobsCommand;
use super::migrate::MigrateCommand;
use super::observe::ObserveCommand;
use super::package::PackageCommand;

#[derive(Parser)]
#[command(name = "corvid", version, about = "The Corvid language compiler")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Scaffold a new Corvid project.
    New { name: String },
    /// Type-check a Corvid source file.
    Check { file: PathBuf },
    /// Compile a Corvid source file. Default target is Python (target/py/);
    /// `--target=native` emits target/bin/, and `--target=wasm`
    /// emits target/wasm/ with `.wasm`, JS loader, and TypeScript types.
    Build {
        file: PathBuf,
        /// Output target. `python` (default), `native`, `server`, `wasm`, `cdylib`, or `staticlib`.
        #[arg(long, default_value = "python")]
        target: String,
        /// Path to a compiled `#[tool]` staticlib. When provided,
        /// tool-using native/library builds link `__corvid_tool_<name>`
        /// symbols against this archive.
        #[arg(long, value_name = "PATH")]
        with_tools_lib: Option<PathBuf>,
        /// Emit a companion C header alongside library targets.
        #[arg(long)]
        header: bool,
        /// Emit a companion ABI descriptor alongside cdylib targets.
        #[arg(long)]
        abi_descriptor: bool,
        /// Emit every supported companion artifact for the selected target.
        #[arg(long)]
        all_artifacts: bool,
        /// Sign the cdylib's embedded ABI descriptor and add a parallel
        /// `CORVID_ABI_ATTESTATION` symbol carrying a DSSE envelope. Hosts
        /// can verify the signature against an ed25519 public key with
        /// `corvid receipt verify-abi`. Accepts a path to a 64-char hex or
        /// 32-byte raw seed; falls back to `CORVID_SIGNING_KEY` env var
        /// when unset. Only valid for `cdylib` target.
        #[arg(long, value_name = "KEY_PATH")]
        sign: Option<PathBuf>,
        /// Opaque key identifier embedded in the DSSE envelope's
        /// `keyid` field. Free-form; typically a fingerprint or
        /// human-readable label. Defaults to "build-key" when
        /// `--sign` is used.
        #[arg(long, value_name = "ID")]
        key_id: Option<String>,
    },
    /// Build and run a Corvid source file. Picks the native AOT tier
    /// when the program stays within the current native command-line
    /// boundary; falls back to the interpreter otherwise with a one-line
    /// notice. Override with `--target`.
    Run {
        file: PathBuf,
        /// Execution tier. `auto` (default) tries native first, falls
        /// back to interpreter when a feature isn't native-able yet.
        /// `native` requires native and errors out otherwise. `interp`
        /// / `interpreter` forces the interpreter tier.
        #[arg(long, default_value = "auto")]
        target: String,
        /// Path to a compiled `#[tool]` staticlib. When
        /// provided, tool-using programs compile and run natively; the
        /// linker resolves `__corvid_tool_<name>` symbols against this
        /// lib. Without it, tool-using programs fall back to the
        /// interpreter (auto) or error out (native).
        #[arg(long, value_name = "PATH")]
        with_tools_lib: Option<PathBuf>,
    },
    /// Run verification suites.
    ///
    /// Targets:
    ///   `dimensions`    algebraic-law proptest over every custom dimension
    ///                   declared in corvid.toml
    ///   `spec`          recompile every .cor example in docs/effects-spec/
    ///                   against the current toolchain; with --meta, run the
    ///                   self-verifying verification harness
    ///   `rewrites`      preserved-semantics rewrite fuzzing; failures name
    ///                   the rewrite rule and algebraic law that drifted
    ///   `adversarial`   LLM-driven bypass generation against the effect
    ///                   checker
    ///
    /// Passing a `.cor` file runs its `test` declarations.
    Test {
        /// Source file to test, or a legacy verification target.
        /// Mutually exclusive with `--from-traces`.
        #[arg(conflicts_with = "from_traces")]
        target: Option<String>,
        /// For `spec`: run the meta-verification harness (mutate the
        /// verifier, confirm each counter-example is still caught).
        #[arg(long)]
        meta: bool,
        /// For `spec`: render the executable spec as static HTML with
        /// Run-in-REPL buttons.
        #[arg(long, value_name = "DIR")]
        site_out: Option<PathBuf>,
        /// For `adversarial`: number of bypass programs to generate.
        #[arg(long, default_value = "100")]
        count: u32,
        /// For `adversarial`: model to drive the generator.
        #[arg(long, default_value = "opus")]
        model: String,
        /// For `.cor` test files: rewrite existing snapshots when values change.
        #[arg(long)]
        update_snapshots: bool,
        /// Prod-as-test-suite mode (Phase 21 slice 21-inv-G-cli,
        /// wired live in 21-inv-G-cli-wire). Replay every `.jsonl`
        /// in `<DIR>` against the current code and report any
        /// behavior drift. Requires `--from-traces-source <FILE>`.
        #[arg(long, value_name = "DIR")]
        from_traces: Option<PathBuf>,
        /// For `--from-traces`: path to the Corvid source the
        /// traces were recorded against. Required today; becomes
        /// optional once `SchemaHeader.source_path` is populated
        /// at record time.
        #[arg(long, value_name = "FILE", requires = "from_traces")]
        from_traces_source: Option<PathBuf>,
        /// For `--from-traces`: differential replay against a
        /// different model. Composes with `21-inv-B-adapter`. When
        /// present, every trace's recorded LLM results are compared
        /// against this model's live output; divergences surface in
        /// the regression report.
        #[arg(long, value_name = "ID", requires = "from_traces")]
        replay_model: Option<String>,
        /// For `--from-traces`: only include traces that hit a
        /// `@dangerous` tool. The Corvid approve-before-dangerous
        /// guarantee means traces with an `ApprovalRequest` event
        /// are exactly the dangerous-tool traces; no separate
        /// annotation needed.
        #[arg(long, requires = "from_traces")]
        only_dangerous: bool,
        /// For `--from-traces`: only include traces that exercise
        /// the named prompt.
        #[arg(long, value_name = "NAME", requires = "from_traces")]
        only_prompt: Option<String>,
        /// For `--from-traces`: only include traces that exercise
        /// the named tool.
        #[arg(long, value_name = "NAME", requires = "from_traces")]
        only_tool: Option<String>,
        /// For `--from-traces`: only include traces with at least
        /// one event at or after this RFC3339 timestamp (matches
        /// `corvid routing-report --since`).
        #[arg(long, value_name = "RFC3339", requires = "from_traces")]
        since: Option<String>,
        /// For `--from-traces`: promote mode. Divergences become
        /// interactively-accepted "golden" traces, overwriting the
        /// originals (Jest-snapshot-style). Mutually exclusive with
        /// `--replay-model` (promoting cross-model divergences would
        /// quietly steal your golden's model; re-record instead)
        /// and with `--flake-detect` (promoting a flaky result is
        /// a bug).
        #[arg(
            long,
            requires = "from_traces",
            conflicts_with = "replay_model",
            conflicts_with = "flake_detect"
        )]
        promote: bool,
        /// For `--from-traces`: flake-detection mode. Replay each
        /// trace N times; any trace producing different output
        /// across runs surfaces program-level nondeterminism the
        /// `@deterministic` attribute didn't catch. Since replay
        /// substitutes recorded responses, deterministic programs
        /// must produce identical output every time.
        #[arg(long, value_name = "N", requires = "from_traces")]
        flake_detect: Option<u32>,
    },
    /// Cross-verify effect profiles across checker, interpreter,
    /// native, and replay tiers.
    Verify {
        /// Verify every `.cor` file under this directory recursively.
        #[arg(long, value_name = "DIR", conflicts_with = "shrink")]
        corpus: Option<PathBuf>,
        /// Shrink a divergent program to a smaller reproducer.
        #[arg(long, value_name = "FILE", conflicts_with = "corpus")]
        shrink: Option<PathBuf>,
        /// Emit the structured report as JSON to stderr.
        #[arg(long)]
        json: bool,
    },
    /// Diff the composed effect profile between two revisions.
    /// Reports dimension-value drift per agent and constraints that
    /// newly fire or release because of the change.
    EffectDiff {
        /// Before revision (git ref or file path).
        before: String,
        /// After revision (git ref or file path).
        after: String,
    },
    /// Install a dimension from the Corvid effect registry.
    /// Verifies the dimension's signature, replays its algebraic-law
    /// proofs against the current toolchain, adds it to corvid.toml.
    AddDimension {
        /// Dimension spec in `name@version` form (e.g. `fairness@1.2`).
        spec: String,
        /// Effect registry index URL, local `index.toml` path, or registry directory.
        #[arg(long)]
        registry: Option<String>,
    },
    /// Add a Corvid package dependency and write/update Corvid.lock.
    Add {
        /// Package spec in `@scope/name@version` form.
        spec: String,
        /// Registry index URL or local `index.toml` path.
        #[arg(long)]
        registry: Option<String>,
    },
    /// Remove a Corvid package dependency from corvid.toml and Corvid.lock.
    Remove {
        /// Package name in `@scope/name` form.
        name: String,
    },
    /// Refresh a Corvid package dependency through its registry.
    Update {
        /// Package name in `@scope/name` form, or a full `@scope/name@version` spec.
        spec: String,
        /// Registry index URL or local `index.toml` path. Overrides the manifest registry.
        #[arg(long)]
        registry: Option<String>,
    },
    /// Aggregate routing and dispatch traces into an optimization report.
    RoutingReport {
        /// Only include events at or after this RFC3339 timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only include events at or after this git commit timestamp.
        #[arg(long)]
        since_commit: Option<String>,
        /// Emit the structured report as JSON.
        #[arg(long)]
        json: bool,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Compute the cost / quality Pareto frontier for one prompt.
    ///
    /// Cost comes from `model_selected.cost_estimate` trace events. Quality
    /// comes from explicit eval host events named `corvid.eval.result`, so the
    /// command reports missing quality evidence instead of guessing.
    CostFrontier {
        /// Prompt to analyze.
        prompt: String,
        /// Only include events at or after this RFC3339 timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only include events at or after this git commit timestamp.
        #[arg(long)]
        since_commit: Option<String>,
        /// Emit the structured report as JSON.
        #[arg(long)]
        json: bool,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Open runnable demos for Corvid's shipped inventions.
    Tour {
        /// List available invention demos.
        #[arg(long)]
        list: bool,
        /// Topic to load into the REPL.
        #[arg(long, value_name = "NAME")]
        topic: Option<String>,
    },
    /// Inspect semantic summaries for every Corvid import in a root file.
    ImportSummary {
        /// Root Corvid source file.
        file: PathBuf,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run eval declarations or evaluate model migrations against traces.
    ///
    /// Default mode executes source-level `eval` declarations and writes
    /// a terminal report plus `target/eval/<file>/report.html`.
    ///
    /// `--swap-model` keeps the Phase 20h retrospective model migration mode:
    /// `corvid eval --swap-model <ID> --source <FILE> <TRACE_OR_DIR>...`.
    /// It replays recorded traces against the candidate model and reports
    /// semantic divergence without re-running unchanged tools.
    Eval {
        /// Corvid source file(s), or trace file(s)/directories with `--swap-model`.
        #[arg(value_name = "FILE_OR_TRACE")]
        inputs: Vec<PathBuf>,
        /// Corvid source the traces were recorded against.
        #[arg(long, value_name = "FILE")]
        source: Option<PathBuf>,
        /// Candidate model for retrospective migration analysis.
        #[arg(long, value_name = "ID")]
        swap_model: Option<String>,
        /// Maximum planned source-eval spend in USD.
        #[arg(long, value_name = "USD")]
        max_spend: Option<f64>,
        /// Replay a production trace directory as golden eval evidence.
        #[arg(long, value_name = "DIR")]
        golden_traces: Option<PathBuf>,
        /// Output directory for `corvid eval promote <trace.lineage.jsonl>`.
        #[arg(long, value_name = "DIR")]
        promote_out: Option<PathBuf>,
    },
    /// AI-assisted drift attribution: decompose drift between
    /// two trace runs into the four named dimensions (model /
    /// prompt / retrieval-index / input). Output's `sources`
    /// carry `(trace_id, span_id)` pairs of every event the
    /// analysis consulted — the `Grounded<T>` shape.
    EvalDrift {
        /// Baseline lineage file or directory.
        #[arg(long, value_name = "PATH")]
        baseline: PathBuf,
        /// Candidate lineage file or directory.
        #[arg(long, value_name = "PATH")]
        candidate: PathBuf,
        /// Carry the slice 40K developer-flow flag for parity
        /// with the documented `corvid eval drift --explain`
        /// surface; the helper output is always structured.
        #[arg(long)]
        explain: bool,
    },
    /// AI-assisted eval fixture from a "wrong answer" feedback
    /// record. Reads the feedback JSON, looks up the matching
    /// trace, redacts via the production redaction policy, and
    /// writes a typed eval fixture to disk.
    EvalFromFeedback {
        /// Path to the feedback JSON record (must include
        /// `trace_id`, optionally `feedback_kind`,
        /// `user_correction`).
        #[arg(long, value_name = "FILE")]
        feedback: PathBuf,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH", default_value = "target/trace")]
        trace_dir: PathBuf,
        /// Write the synthesised fixture to this path. Without
        /// `--out`, the helper prints the fixture summary to
        /// stdout and skips file emission.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
    /// Re-execute a recorded trace deterministically.
    ///
    /// Default mode: substitute recorded responses for every
    /// live call and reproduce the original run byte-for-byte.
    ///
    /// With `--model <id>`: differential replay — instead of
    /// using recorded LLM results verbatim, issue each prompt
    /// against `<id>` and render a divergence report listing
    /// the steps whose output differs. Useful for testing a
    /// new model version against a corpus of prod traces
    /// without paying the full re-run cost.
    ///
    /// With `--mutate <STEP> <JSON>`: counterfactual replay —
    /// replay the trace with exactly one recorded response
    /// overridden and render the downstream behavior diff.
    /// `<STEP>` is the 1-based index among substitutable events
    /// (ToolCall / LlmCall / ApprovalRequest) as numbered in
    /// `corvid trace show`.
    ///
    /// `--source <FILE>` points at the Corvid source the trace
    /// was recorded against. Eventually this will be inferred
    /// from `SchemaHeader.source_path` when populated; today
    /// it's required for any actually-execute mode (differential
    /// / plain). Modes that don't actually run (load-validation
    /// only) ignore it.
    Replay {
        /// Path to a JSONL trace file.
        trace: PathBuf,
        /// Path to the Corvid source the trace was recorded
        /// against. Required for modes that execute (default
        /// plain replay, `--model` differential, `--mutate`
        /// counterfactual). Once `SchemaHeader.source_path` is
        /// populated at record time, this flag becomes optional.
        #[arg(long, value_name = "FILE")]
        source: Option<PathBuf>,
        /// Target model for differential replay. When present,
        /// replays against this model and reports divergences
        /// against the recorded results. When absent, runs a
        /// plain reproduction. Mutually exclusive with
        /// `--mutate`.
        #[arg(long, value_name = "ID", conflicts_with = "mutate")]
        model: Option<String>,
        /// Counterfactual replay: replace the response of the
        /// substitutable event at 1-based `STEP` with `JSON`,
        /// then replay and diff. Mutually exclusive with
        /// `--model`.
        #[arg(
            long,
            num_args = 2,
            value_names = ["STEP", "JSON"],
            conflicts_with = "model",
        )]
        mutate: Option<Vec<String>>,
    },
    /// Inspect or verify the embedded Corvid ABI/capability descriptor.
    Abi {
        #[command(subcommand)]
        command: AbiCommand,
    },
    /// Generate Rust or Python host bindings from a Corvid ABI descriptor.
    Bind {
        /// Output language: `rust` or `python`.
        language: String,
        /// Path to a `.corvid-abi.json` descriptor.
        descriptor: PathBuf,
        /// Output directory for the generated crate/package.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
    /// Work with reproducibility-spec bundles.
    Bundle {
        #[command(subcommand)]
        command: BundleCommand,
    },
    /// Check or simulate a Corvid approver source.
    Approver {
        #[command(subcommand)]
        command: ApproverCommand,
    },
    /// Package or replay a portable execution capsule containing
    /// the library, descriptor, trace, and manifest.
    Capsule {
        #[command(subcommand)]
        command: CapsuleCommand,
    },
    /// Inspect recorded traces under `target/trace/` (or a
    /// user-supplied directory).
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },
    /// Inspect Phase 40 lineage observability stores.
    Observe {
        #[command(subcommand)]
        command: ObserveCommand,
    },
    /// Behavior-diff a PR: compile the source at two git
    /// revisions, extract the Corvid ABI descriptor from each,
    /// and render a PR behavior receipt describing every
    /// algebraic change (trust tier, `@dangerous`, `@replayable`,
    /// added / removed agents). Compares the *exported surface*
    /// — `pub extern "c"` agents and their transitive closure —
    /// since that is the AI-safety boundary a host consumes. The
    /// reviewer is itself a Corvid `@deterministic` agent (see
    /// `crates/corvid-cli/src/trace_diff/reviewer.cor`), so
    /// receipts are byte-identical across reruns. With `--traces
    /// <dir>`, each recorded trace is replayed against base and
    /// head; the receipt includes a counterfactual impact
    /// section reporting which traces would have newly diverged
    /// under the PR.
    TraceDiff {
        /// Git revision for the "before" side (typically the PR
        /// base branch tip).
        base_sha: String,
        /// Git revision for the "after" side (typically the PR
        /// head branch tip).
        head_sha: String,
        /// Path within the repo to the single `.cor` source file
        /// to compare. Multi-file sources are a follow-up slice.
        path: PathBuf,
        /// Optional directory of recorded `.jsonl` traces to
        /// replay against both SHAs. When present, the receipt
        /// gains a "Counterfactual Replay Impact" section with
        /// the newly-divergent trace population + an impact
        /// percentage.
        #[arg(long, value_name = "DIR")]
        traces: Option<PathBuf>,
        /// Whether the receipt's top-of-page prose summary is
        /// generated by an LLM. `auto` (default) uses the
        /// narrative when `CORVID_MODEL` + an `ANTHROPIC_API_KEY`
        /// / `OPENAI_API_KEY` is set, silently falls back to
        /// deterministic boilerplate otherwise. `on` hard-fails
        /// when no adapter is available. `off` skips the prompt
        /// entirely so the receipt is byte-deterministic — pick
        /// this for CI and reproducers.
        #[arg(long, value_name = "MODE", default_value = "auto")]
        narrative: String,
        /// Output format for the receipt. `markdown` (human
        /// review), `github-check` (GitHub Actions annotation
        /// commands on stdout), `json` (schema-versioned,
        /// bot-consumable), `in-toto` (SLSA/Sigstore-compatible
        /// Statement v1 with the Corvid receipt as predicate),
        /// `gitlab` (CodeClimate-compatible codequality JSON for
        /// GitLab MR widget via `artifacts.reports.codequality`),
        /// `watch` (local reactive mode comparing base SHA against
        /// the working-tree file and rerendering on change).
        /// `auto` (default) detects the environment: GitHub
        /// Actions → `github-check`, GitLab CI → `gitlab`, piped
        /// stdout → `json`, tty → `markdown`. Non-zero exit on
        /// any regression the default policy flags regardless
        /// of format.
        #[arg(long, value_name = "MODE", default_value = "auto")]
        format: String,
        /// Sign the canonical JSON receipt with the ed25519 key
        /// at the given path and emit a DSSE envelope instead
        /// of the raw `--format` output. The key file is read
        /// as 64 hex chars (32-byte ed25519 seed) or 32 raw
        /// bytes. When omitted, the CLI falls back to the
        /// `CORVID_SIGNING_KEY` env var (also hex or raw).
        /// With neither set, signing is skipped and the
        /// `--format` output prints unchanged.
        #[arg(long, value_name = "KEY_PATH")]
        sign: Option<PathBuf>,
        /// Key ID embedded in the DSSE envelope's
        /// `signatures[0].keyid` field. Free-form identifier
        /// useful for downstream verifiers to pick the right
        /// verifying key. Defaults to `corvid-default`.
        #[arg(long, value_name = "ID")]
        sign_key_id: Option<String>,
        /// Replace the baked trace-diff regression policy with a
        /// user-supplied Corvid policy body. The file must define
        /// `@deterministic agent apply_policy(receipt:
        /// PolicyReceipt) -> Verdict`; the CLI prepends the
        /// typed policy prelude so policy code reasons over
        /// structured safety facts, not raw delta-key strings.
        #[arg(long, value_name = "POLICY_COR")]
        policy: Option<PathBuf>,
        /// Enter stack mode: compose per-commit trace-diff
        /// receipts across a commit range into one algebraic
        /// `StackReceipt` with normal-form (cancelled) + history
        /// (preserved) views and `introduced_at` provenance per
        /// surviving delta. Without a value, the commit range is
        /// derived from the positional `<base>..<head>` (or CI
        /// env vars `GITHUB_BASE_REF` / `CI_MERGE_REQUEST_DIFF_BASE_SHA`
        /// when set). With a value, accepts either a git range
        /// expression (e.g. `main..feature`, `HEAD~5..HEAD`) or
        /// a comma-separated list of SHAs. Currently only emits
        /// `--format=json`; other renderers and `--sign` /
        /// `--traces` integration land in later commits of
        /// `21-inv-H-5-stacked`.
        #[arg(
            long,
            value_name = "SPEC",
            num_args = 0..=1,
            default_missing_value = "",
        )]
        stack: Option<String>,
        /// Force full per-waypoint x per-trace replay in stack
        /// mode, disabling the algebra-directed skip that proves
        /// no-change (trace, commit) pairs don't need replay.
        /// Skip is active by default and is behaviorally
        /// equivalent to full replay; this flag exists for
        /// debugging, audit, and verifying the skip's correctness
        /// on new workloads. Not meaningful without `--stack
        /// --traces`; ignored otherwise.
        #[arg(long)]
        no_replay_skip: bool,
    },
    /// Work with receipts produced by `corvid trace-diff --sign`:
    /// show a receipt from the local cache by its hash, or verify
    /// a DSSE envelope against a supplied verifying key.
    Receipt {
        #[command(subcommand)]
        command: ReceiptCommand,
    },
    /// Publish and verify source packages for Corvid registries.
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },
    /// Emit a self-contained provenance statement for a Corvid cdylib.
    Claim {
        #[command(subcommand)]
        command: Option<ClaimCommand>,
        /// Print the claim explanation. Kept as an explicit flag so
        /// command transcripts read as `corvid claim --explain <cdylib>`.
        #[arg(long)]
        explain: bool,
        /// Path to the cdylib `.so` / `.dll` / `.dylib`.
        cdylib: Option<PathBuf>,
        /// Optional ed25519 verifying key. When supplied, the claim
        /// verifies the embedded ABI attestation and prints the key
        /// fingerprint.
        #[arg(long, value_name = "KEY_PATH")]
        key: Option<PathBuf>,
        /// Optional Corvid source file. When supplied, the claim
        /// independently rebuilds the ABI descriptor and proves it
        /// byte-matches the cdylib descriptor.
        #[arg(long, value_name = "SOURCE")]
        source: Option<PathBuf>,
    },
    /// Start the interactive Corvid REPL.
    Repl,
    /// Check the local environment for required tools.
    Doctor,
    /// Audit a Corvid project surface for launch-relevant risks.
    Audit {
        /// Root Corvid source file.
        file: PathBuf,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Generate deployment artifacts for a Corvid backend app.
    Deploy {
        #[command(subcommand)]
        command: DeployCommand,
    },
    /// Produce signed release-channel artifacts.
    Release {
        /// Release channel: nightly, beta, or stable.
        channel: String,
        /// Explicit version. Nightly requires `-nightly.`, beta requires `-beta.`, stable is plain SemVer.
        version: Option<String>,
        /// Output directory for generated release artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Check or apply source and stdlib migrations.
    Upgrade {
        #[command(subcommand)]
        command: UpgradeCommand,
    },
    /// Inspect and apply checked-in database migrations.
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    /// Enqueue and run durable local background jobs.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    /// Inspect published orchestration-overhead benchmark archives.
    Bench {
        #[command(subcommand)]
        command: BenchCommand,
    },
    /// Inspect Corvid's canonical guarantee table — the public list of
    /// promises the compiler and runtime enforce, the pipeline phase
    /// each lives in, and the test references that prove each one.
    Contract {
        #[command(subcommand)]
        command: ContractCommand,
    },
    /// Inspect, validate, and exercise Corvid's built-in connectors
    /// (Gmail, Slack, GitHub/Linear tasks, Microsoft 365, calendar,
    /// local files). Real-mode operations gate on
    /// `CORVID_PROVIDER_LIVE=1` and the relevant per-provider
    /// credential env vars.
    Connectors {
        #[command(subcommand)]
        command: ConnectorsCommand,
    },
    /// Manage Corvid's auth surface: initialize the session / API
    /// key / approval stores, issue / revoke / rotate API keys.
    /// Stores live in SQLite files under `target/` by default; the
    /// `--auth-state` and `--approvals-state` flags accept a
    /// production-grade location.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Manage the human-in-the-loop approval queue: list, inspect,
    /// approve, deny, expire, comment, delegate, batch-approve, and
    /// export approvals for a tenant. Every transition writes a
    /// trace + audit event the compliance review consumes.
    Approvals {
        #[command(subcommand)]
        command: ApprovalsCommand,
    },
}

#[derive(Subcommand)]
pub enum UpgradeCommand {
    /// Report syntax and stdlib migrations without modifying files.
    Check {
        /// Source file or project directory to scan.
        path: PathBuf,
        /// Emit JSON findings.
        #[arg(long)]
        json: bool,
    },
    /// Apply safe syntax and stdlib migrations.
    Apply {
        /// Source file or project directory to rewrite.
        path: PathBuf,
        /// Emit JSON findings.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum ReceiptCommand {
    /// Resolve a cached receipt by its SHA-256 hash (or a
    /// unique prefix of at least 8 characters) and print the
    /// canonical JSON to stdout.
    Show {
        /// Receipt hash (full 64-char SHA-256, or a unique
        /// prefix of at least 8 characters).
        hash: String,
    },
    /// Verify a DSSE envelope against an ed25519 verifying key.
    /// Prints the inner receipt payload on success; exits
    /// non-zero with a typed error on any verification failure.
    Verify {
        /// Envelope location: either a filesystem path to a
        /// `.envelope.json` file OR a hash-prefix matching a
        /// cached `<hash>.envelope.json`.
        envelope: String,
        /// Path to the ed25519 verifying key (64 hex chars or
        /// 32 raw bytes).
        #[arg(long, value_name = "KEY_PATH")]
        key: PathBuf,
    },
    /// Verify the embedded `CORVID_ABI_ATTESTATION` of a Corvid
    /// cdylib against an ed25519 verifying key. Confirms the
    /// signature is valid AND that the recovered descriptor JSON
    /// matches the `CORVID_ABI_DESCRIPTOR` symbol — tampering with
    /// either side is detected. Exits 0 on verified, 2 on absent
    /// (no attestation symbol — host policy decides), 1 on every
    /// other failure (signature mismatch / descriptor drift /
    /// malformed envelope).
    VerifyAbi {
        /// Path to the cdylib `.so` / `.dll` / `.dylib`.
        cdylib: PathBuf,
        /// Path to the ed25519 verifying key (64 hex chars or
        /// 32 raw bytes).
        #[arg(long, value_name = "KEY_PATH")]
        key: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum BundleCommand {
    Verify {
        path: PathBuf,
        #[arg(long)]
        rebuild: bool,
    },
    Diff {
        old: PathBuf,
        new: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Audit {
        path: PathBuf,
        #[arg(long)]
        question: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Explain {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Report {
        path: PathBuf,
        #[arg(long, default_value = "soc2")]
        format: String,
        #[arg(long)]
        json: bool,
    },
    Query {
        path: PathBuf,
        #[arg(long, value_name = "DELTA_KEY")]
        delta: String,
        #[arg(long, value_name = "NAME")]
        predecessor: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Lineage {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum TraceCommand {
    /// List every JSONL trace under `--trace-dir` (default:
    /// `target/trace/`). One row per trace with run id, schema
    /// version, event count, and timestamp range.
    List {
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Print every event in a trace as formatted JSON, one
    /// event per line.
    Show {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Render the Grounded<T> provenance DAG of a trace as a
    /// Graphviz DOT graph. Pipe into `dot -Tsvg > prov.svg` to
    /// render. Traces without provenance events produce an empty
    /// digraph plus a warning on stderr.
    Dag {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Render a Phase 40 lineage JSONL trace as an indented tree.
    Lineage {
        /// Lineage trace identifier: either a direct file path, or a
        /// run id resolved as `<id>.lineage.jsonl` under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare run id.
        /// Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum AbiCommand {
    Dump {
        library: PathBuf,
    },
    Hash {
        source: PathBuf,
    },
    Verify {
        library: PathBuf,
        #[arg(long, value_name = "HEX")]
        expected_hash: String,
    },
}

#[derive(Subcommand)]
pub enum ApproverCommand {
    Check {
        approver: PathBuf,
        #[arg(long, value_name = "USD")]
        max_budget_usd: Option<f64>,
    },
    Simulate {
        approver: PathBuf,
        site_label: String,
        #[arg(long, value_name = "JSON")]
        args: String,
        #[arg(long, value_name = "USD")]
        max_budget_usd: Option<f64>,
    },
    Card {
        site_label: String,
        #[arg(long, value_name = "JSON")]
        args: String,
        #[arg(long, value_enum, default_value_t = ApproverCardFormat::Text)]
        format: ApproverCardFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ApproverCardFormat {
    Text,
    Json,
    Html,
}

#[derive(Subcommand)]
pub enum CapsuleCommand {
    Create {
        trace: PathBuf,
        cdylib: PathBuf,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    Replay {
        capsule: PathBuf,
    },
}
