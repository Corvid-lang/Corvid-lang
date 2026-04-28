//! The `corvid` CLI.
//!
//! Subcommands:
//!   corvid new <name>         scaffold a new project
//!   corvid check <file>       type-check a source file
//!   corvid build <file>       compile to target/py/<name>.py
//!   corvid build --target=wasm <file> emits browser/edge artifacts
//!   corvid run <file>         build + invoke python on the output
//!   corvid repl               start the interactive REPL
//!   corvid test <file>        run Corvid test declarations in a source file
//!   corvid test <what>        run verification suites (dimensions, spec, rewrites, adversarial)
//!   corvid verify             cross-tier effect-profile verification
//!   corvid effect-diff        diff composed effect profiles between two revisions
//!   corvid add                add a package dependency to Corvid.lock
//!   corvid remove             remove a package dependency from corvid.toml and Corvid.lock
//!   corvid update             refresh a package dependency through its registry
//!   corvid add-dimension      install a dimension from the effect registry
//!   corvid routing-report     aggregate dispatch traces into routing guidance
//!   corvid cost-frontier      compute prompt cost/quality Pareto frontier
//!   corvid tour               open runnable demos for Corvid inventions
//!   corvid import-summary     inspect imported module semantic contracts
//!   corvid eval --swap-model <id> <trace>  retrospective model migration analysis
//!   corvid replay <trace>     re-execute a recorded trace deterministically
//!   corvid replay --model <id> <trace>  differential replay against a different model
//!   corvid abi dump <lib>     inspect the embedded ABI/capability catalog
//!   corvid trace list         list traces under target/trace/
//!   corvid trace show <id>    print a recorded trace as formatted JSON
//!   corvid trace dag <id>     render provenance DAG as Graphviz DOT
//!   corvid claim --explain <lib>  explain the guarantees claimed by a cdylib

mod abi_cmd;
mod approver_cmd;
mod audit_cmd;
mod bench_cmd;
mod bind_cmd;
mod bundle_cmd;
mod capsule_cmd;
mod claim_cmd;
mod contract_cmd;
mod cost_frontier;
mod eval_cmd;
mod receipt_cache;
mod receipt_cmd;
mod replay;
mod routing_report;
mod test_from_traces;
mod tour;
mod trace_cmd;
mod trace_dag;
mod trace_diff;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use corvid_differential_verify::{
    render_corpus_grid, render_report, shrink_program, verify_corpus,
};
use corvid_runtime::queue::{
    DurableQueueRuntime, QueueJob, QueueScheduleManifest, ScheduleMissedPolicy,
};
use cost_frontier::{build_frontier, render_frontier as render_cost_frontier, CostFrontierOptions};
use routing_report::{build_report, render_report as render_routing_report, RoutingReportOptions};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_server_to_disk, build_spec_site, build_target_to_disk,
    build_to_disk, build_wasm_to_disk, compile, compile_with_config, diff_snapshots,
    file_github_issues_for_escapes, inspect_import_semantics, load_corvid_config_for,
    load_corvid_config_with_path_for, load_dotenv_walking, render_adversarial_report,
    render_all_pretty, render_dimension_verification_report, render_effect_diff,
    render_import_semantic_summaries, render_law_check_report, render_spec_report,
    render_spec_site_report, render_test_report, run_adversarial_suite, run_dimension_verification,
    run_law_checks, run_native, run_tests_at_path_with_options, run_with_target, scaffold_new,
    snapshot_revision, test_options, verify_spec_examples, BuildTarget, RunTarget, VerdictKind,
    DEFAULT_SAMPLES,
};

#[derive(Parser)]
#[command(name = "corvid", version, about = "The Corvid language compiler")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
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
        /// Print the claim explanation. Kept as an explicit flag so
        /// command transcripts read as `corvid claim --explain <cdylib>`.
        #[arg(long)]
        explain: bool,
        /// Path to the cdylib `.so` / `.dll` / `.dylib`.
        cdylib: PathBuf,
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
}

#[derive(Subcommand)]
enum BenchCommand {
    /// Compare Corvid against Python or JS/TypeScript using a published archive.
    Compare {
        /// Comparison target: `python`, `js`, or `typescript`.
        target: String,
        /// Benchmark session id under `benches/results/`.
        #[arg(
            long,
            value_name = "SESSION",
            default_value = "2026-04-17-marketable-session"
        )]
        session: String,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ContractCommand {
    /// Print the canonical guarantee table.
    ///
    /// Default output is human-readable: one row per guarantee with
    /// id, kind, class (static / runtime-checked / out-of-scope),
    /// pipeline phase, and a one-line description. `--json` emits the
    /// full structured table including test references and (where
    /// applicable) the explicit `out_of_scope_reason` for non-defenses.
    /// The output is the single source of truth that `docs/core-semantics.md`
    /// is generated from in slice 35-D and that `corvid claim --explain`
    /// reports against in slice 35-I.
    List {
        /// Emit machine-readable JSON instead of the human-readable table.
        #[arg(long)]
        json: bool,
        /// Filter by class. Accepts `static`, `runtime_checked`, or
        /// `out_of_scope`. Repeatable; unspecified shows everything.
        #[arg(long, value_name = "CLASS")]
        class: Option<String>,
        /// Filter by kind (e.g. `approval`, `effect_row`, `grounded`,
        /// `budget`, `confidence`, `replay`, `provenance_trace`,
        /// `abi_descriptor`, `abi_attestation`, `platform`).
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
    },
    /// Regenerate `docs/core-semantics.md` from the canonical
    /// guarantee registry. Writes the rendered markdown to the given
    /// `OUTPUT` path (typically `docs/core-semantics.md`); CI fails on
    /// drift between the committed file and the live render, so this
    /// command is the only sanctioned way to evolve the spec doc when
    /// the registry changes.
    RegenDoc {
        /// Output path, e.g. `docs/core-semantics.md`.
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum ReceiptCommand {
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
enum BundleCommand {
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
enum PackageCommand {
    /// Render the public semantic metadata page for a source package.
    Metadata {
        /// Source `.cor` file to inspect.
        source: PathBuf,
        /// Scoped package name, e.g. `@scope/name`.
        #[arg(long)]
        name: String,
        /// Semantic version to display in install snippets.
        #[arg(long)]
        version: String,
        /// Optional package signature to render on the metadata page.
        #[arg(long)]
        signature: Option<String>,
        /// Emit structured JSON instead of Markdown.
        #[arg(long)]
        json: bool,
    },
    /// Verify a registry index and all referenced source artifacts.
    VerifyRegistry {
        /// Registry index URL, local index.toml, or registry directory.
        registry: String,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify corvid.toml and Corvid.lock agree with package policy.
    VerifyLock {
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Publish a signed source package into a registry directory.
    Publish {
        /// Source `.cor` file to publish.
        source: PathBuf,
        /// Scoped package name, e.g. `@scope/name`.
        #[arg(long)]
        name: String,
        /// Semantic version, e.g. `1.2.3`.
        #[arg(long)]
        version: String,
        /// Registry output directory. `index.toml` is created/updated here.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Public URL prefix where copied package artifacts will be served.
        #[arg(long, value_name = "URL")]
        url_base: String,
        /// 32-byte Ed25519 signing seed as 64 hex chars.
        #[arg(long, value_name = "HEX")]
        key: String,
        /// Key identifier embedded in the package signature.
        #[arg(long, default_value = "corvid-package")]
        key_id: String,
    },
}

#[derive(Subcommand)]
enum TraceCommand {
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
}

#[derive(Subcommand)]
enum AbiCommand {
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
enum ApproverCommand {
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
enum ApproverCardFormat {
    Text,
    Json,
    Html,
}

#[derive(Subcommand)]
enum CapsuleCommand {
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

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::New { name }) => cmd_new(&name),
        Some(Command::Check { file }) => cmd_check(&file),
        Some(Command::Build {
            file,
            target,
            with_tools_lib,
            header,
            abi_descriptor,
            all_artifacts,
            sign,
            key_id,
        }) => cmd_build(
            &file,
            &target,
            with_tools_lib.as_deref(),
            header,
            abi_descriptor,
            all_artifacts,
            sign.as_deref(),
            key_id.as_deref(),
        ),
        Some(Command::Run {
            file,
            target,
            with_tools_lib,
        }) => cmd_run(&file, &target, with_tools_lib.as_deref()),
        Some(Command::Test {
            target,
            meta,
            site_out,
            count,
            model,
            update_snapshots,
            from_traces,
            from_traces_source,
            replay_model,
            only_dangerous,
            only_prompt,
            only_tool,
            since,
            promote,
            flake_detect,
        }) => {
            if let Some(dir) = from_traces {
                test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
                    trace_dir: &dir,
                    source: from_traces_source.as_deref(),
                    replay_model: replay_model.as_deref(),
                    only_dangerous,
                    only_prompt: only_prompt.as_deref(),
                    only_tool: only_tool.as_deref(),
                    since: since.as_deref(),
                    promote,
                    flake_detect,
                })
            } else {
                cmd_test(
                    target.as_deref(),
                    meta,
                    site_out.as_deref(),
                    count,
                    &model,
                    update_snapshots,
                )
            }
        }
        Some(Command::Verify {
            corpus,
            shrink,
            json,
        }) => cmd_verify(corpus.as_deref(), shrink.as_deref(), json),
        Some(Command::EffectDiff { before, after }) => cmd_effect_diff(&before, &after),
        Some(Command::AddDimension { spec, registry }) => {
            cmd_add_dimension(&spec, registry.as_deref())
        }
        Some(Command::Add { spec, registry }) => cmd_add_package(&spec, registry.as_deref()),
        Some(Command::Remove { name }) => cmd_remove_package(&name),
        Some(Command::Update { spec, registry }) => cmd_update_package(&spec, registry.as_deref()),
        Some(Command::RoutingReport {
            since,
            since_commit,
            json,
            trace_dir,
        }) => cmd_routing_report(
            trace_dir.as_deref(),
            since.as_deref(),
            since_commit.as_deref(),
            json,
        ),
        Some(Command::CostFrontier {
            prompt,
            since,
            since_commit,
            json,
            trace_dir,
        }) => cmd_cost_frontier(
            &prompt,
            trace_dir.as_deref(),
            since.as_deref(),
            since_commit.as_deref(),
            json,
        ),
        Some(Command::Tour { list, topic }) => tour::cmd_tour(list, topic.as_deref()),
        Some(Command::ImportSummary { file, json }) => cmd_import_summary(&file, json),
        Some(Command::Eval {
            inputs,
            source,
            swap_model,
            max_spend,
            golden_traces,
        }) => eval_cmd::run_eval(
            &inputs,
            source.as_deref(),
            swap_model.as_deref(),
            max_spend,
            golden_traces.as_deref(),
        ),
        Some(Command::Replay {
            trace,
            source,
            model,
            mutate,
        }) => replay::run_replay(
            &trace,
            source.as_deref(),
            model.as_deref(),
            mutate.as_deref(),
        ),
        Some(Command::Abi { command }) => match command {
            AbiCommand::Dump { library } => abi_cmd::run_dump(&library),
            AbiCommand::Hash { source } => abi_cmd::run_hash(&source),
            AbiCommand::Verify {
                library,
                expected_hash,
            } => abi_cmd::run_verify(&library, &expected_hash),
        },
        Some(Command::Bind {
            language,
            descriptor,
            out,
        }) => bind_cmd::run_bind(&language, &descriptor, &out),
        Some(Command::Bundle { command }) => match command {
            BundleCommand::Verify { path, rebuild } => bundle_cmd::run_verify(&path, rebuild),
            BundleCommand::Diff { old, new, json } => bundle_cmd::run_diff(&old, &new, json),
            BundleCommand::Audit {
                path,
                question,
                json,
            } => bundle_cmd::run_audit(&path, question.as_deref(), json),
            BundleCommand::Explain { path, json } => bundle_cmd::run_explain(&path, json),
            BundleCommand::Report { path, format, json } => {
                bundle_cmd::run_report(&path, &format, json)
            }
            BundleCommand::Query {
                path,
                delta,
                predecessor,
                json,
            } => bundle_cmd::run_query(&path, &delta, predecessor.as_deref(), json),
            BundleCommand::Lineage { path, json } => bundle_cmd::run_lineage(&path, json),
        },
        Some(Command::Approver { command }) => match command {
            ApproverCommand::Check {
                approver,
                max_budget_usd,
            } => approver_cmd::run_check(&approver, max_budget_usd),
            ApproverCommand::Simulate {
                approver,
                site_label,
                args,
                max_budget_usd,
            } => approver_cmd::run_simulate(&approver, &site_label, &args, max_budget_usd),
            ApproverCommand::Card {
                site_label,
                args,
                format,
            } => approver_cmd::run_card(&site_label, &args, format),
        },
        Some(Command::Capsule { command }) => match command {
            CapsuleCommand::Create { trace, cdylib, out } => {
                capsule_cmd::run_create(&trace, &cdylib, out.as_deref())
            }
            CapsuleCommand::Replay { capsule } => capsule_cmd::run_replay(&capsule),
        },
        Some(Command::Trace { command }) => match command {
            TraceCommand::List { trace_dir } => trace_cmd::run_list(trace_dir.as_deref()),
            TraceCommand::Show {
                id_or_path,
                trace_dir,
            } => trace_cmd::run_show(&id_or_path, trace_dir.as_deref()),
            TraceCommand::Dag {
                id_or_path,
                trace_dir,
            } => trace_dag::run_dag(&id_or_path, trace_dir.as_deref()),
        },
        Some(Command::TraceDiff {
            base_sha,
            head_sha,
            path,
            traces,
            narrative,
            format,
            sign,
            sign_key_id,
            policy,
            stack,
            no_replay_skip,
        }) => {
            let parsed = narrative
                .parse::<trace_diff::NarrativeMode>()
                .map_err(anyhow::Error::msg)
                .and_then(|narrative_mode| {
                    trace_diff::OutputFormat::parse(&format)
                        .map_err(anyhow::Error::msg)
                        .map(|format| (narrative_mode, format))
                })
                .and_then(|(narrative_mode, format)| {
                    stack
                        .as_deref()
                        .map(trace_diff::parse_stack_spec)
                        .transpose()
                        .map_err(anyhow::Error::msg)
                        .map(|stack_spec| (narrative_mode, format, stack_spec))
                });
            match parsed {
                Ok((narrative_mode, format, stack_spec)) => {
                    trace_diff::run_trace_diff(trace_diff::TraceDiffArgs {
                        base_sha: &base_sha,
                        head_sha: &head_sha,
                        source_path: &path,
                        trace_dir: traces.as_deref(),
                        narrative_mode,
                        format,
                        sign_key_path: sign.as_deref(),
                        sign_key_id: sign_key_id.as_deref(),
                        policy_path: policy.as_deref(),
                        stack_spec,
                        no_replay_skip,
                    })
                }
                Err(e) => Err(e),
            }
        }
        Some(Command::Receipt { command }) => match command {
            ReceiptCommand::Show { hash } => receipt_cmd::run_show(&hash),
            ReceiptCommand::Verify { envelope, key } => receipt_cmd::run_verify(&envelope, &key),
            ReceiptCommand::VerifyAbi { cdylib, key } => receipt_cmd::run_verify_abi(&cdylib, &key),
        },
        Some(Command::Package { command }) => match command {
            PackageCommand::Metadata {
                source,
                name,
                version,
                signature,
                json,
            } => cmd_package_metadata(&source, &name, &version, signature.as_deref(), json),
            PackageCommand::VerifyRegistry { registry, json } => {
                cmd_package_verify_registry(&registry, json)
            }
            PackageCommand::VerifyLock { json } => cmd_package_verify_lock(json),
            PackageCommand::Publish {
                source,
                name,
                version,
                out,
                url_base,
                key,
                key_id,
            } => cmd_package_publish(&source, &name, &version, &out, &url_base, &key, &key_id),
        },
        Some(Command::Claim {
            explain,
            cdylib,
            key,
            source,
        }) => claim_cmd::run_claim_explain(&cdylib, explain, key.as_deref(), source.as_deref()),
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor_v2(),
        Some(Command::Audit { file, json }) => audit_cmd::run_audit(&file, json),
        Some(Command::Migrate { command }) => match command {
            MigrateCommand::Status {
                dir,
                state,
                database,
                dry_run,
            } => cmd_migrate("status", &dir, &state, &database, dry_run),
            MigrateCommand::Up {
                dir,
                state,
                database,
                dry_run,
            } => cmd_migrate("up", &dir, &state, &database, dry_run),
            MigrateCommand::Down {
                dir,
                down_dir,
                state,
                database,
                dry_run,
            } => cmd_migrate_down(&dir, &down_dir, &state, &database, dry_run),
        },
        Some(Command::Jobs { command }) => match command {
            JobsCommand::Enqueue {
                state,
                task,
                payload,
                input_schema,
                max_retries,
                budget_usd,
                effect_summary,
                replay_key,
                delay_ms,
            } => cmd_jobs_enqueue(
                &state,
                &task,
                &payload,
                input_schema,
                max_retries,
                budget_usd,
                effect_summary,
                replay_key,
                delay_ms,
            ),
            JobsCommand::RunOne {
                state,
                output_kind,
                output_fingerprint,
                fail_kind,
                fail_fingerprint,
                retry_base_ms,
            } => cmd_jobs_run_one(
                &state,
                output_kind,
                output_fingerprint,
                fail_kind,
                fail_fingerprint,
                retry_base_ms,
            ),
            JobsCommand::Schedule { command } => match command {
                JobsScheduleCommand::Add {
                    state,
                    id,
                    cron,
                    zone,
                    task,
                    payload,
                    max_retries,
                    budget_usd,
                    effect_summary,
                    replay_key_prefix,
                    missed_policy,
                } => cmd_jobs_schedule_add(
                    &state,
                    &id,
                    &cron,
                    &zone,
                    &task,
                    &payload,
                    max_retries,
                    budget_usd,
                    effect_summary,
                    replay_key_prefix,
                    missed_policy,
                ),
                JobsScheduleCommand::List { state } => cmd_jobs_schedule_list(&state),
                JobsScheduleCommand::Recover {
                    state,
                    max_missed_per_schedule,
                } => cmd_jobs_schedule_recover(&state, max_missed_per_schedule),
            },
            JobsCommand::Dlq { state } => cmd_jobs_dlq(&state),
        },
        Some(Command::Bench { command }) => match command {
            BenchCommand::Compare {
                target,
                session,
                json,
            } => bench_cmd::run_compare(&target, &session, json),
        },
        Some(Command::Contract { command }) => match command {
            ContractCommand::List { json, class, kind } => {
                contract_cmd::run_list(json, class.as_deref(), kind.as_deref())
            }
            ContractCommand::RegenDoc { output } => contract_cmd::run_regen_doc(&output),
        },
        None => {
            println!("corvid — the AI-native language compiler");
            println!("Run `corvid --help` for usage.");
            Ok(0)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

// ------------------------------------------------------------
// Commands
// ------------------------------------------------------------

fn cmd_new(name: &str) -> Result<u8> {
    let root = scaffold_new(name).context("failed to scaffold project")?;
    println!("created new Corvid project at `{}`", root.display());
    println!("\nNext steps:");
    println!("  cd {name}");
    println!("  pip install corvid-runtime");
    println!("  corvid run src/main.cor");
    Ok(0)
}

fn cmd_migrate(
    action: &str,
    dir: &Path,
    state: &Path,
    database: &Path,
    dry_run: bool,
) -> Result<u8> {
    let migrations = scan_migration_files(dir)?;
    let mut migration_state = load_migration_state(state)?;
    let drift = detect_migration_drift(&migrations, &migration_state);
    println!("corvid migrate {action}");
    println!("migrations: {}", dir.display());
    println!("state: {}", state.display());
    println!("database: {}", database.display());
    println!("dry_run: {dry_run}");
    let applied_count = migrations
        .iter()
        .filter(|migration| {
            migration_state
                .migrations
                .iter()
                .any(|applied| applied.name == migration.name && applied.sha256 == migration.sha256)
        })
        .count();
    let pending_count = migrations.len().saturating_sub(applied_count);
    println!("applied_count: {applied_count}");
    println!("pending_count: {pending_count}");
    println!("drift_count: {}", drift.len());
    if migrations.is_empty() {
        println!("migrations_found: 0");
    } else {
        println!("migrations_found: {}", migrations.len());
        for migration in &migrations {
            let applied = migration_state.migrations.iter().any(|applied| {
                applied.name == migration.name && applied.sha256 == migration.sha256
            });
            println!(
                "migration: {} sha256:{} status:{}",
                migration.name,
                migration.sha256,
                if applied { "applied" } else { "pending" }
            );
        }
    }
    for item in &drift {
        println!("drift: {} {}", item.kind, item.message);
    }
    if !drift.is_empty() {
        println!("drift_found: {}", drift.len());
        println!("state_updated: false");
        return Ok(1);
    }
    if action == "up" && !dry_run {
        apply_pending_sql_migrations(database, &migrations, &mut migration_state)?;
        save_migration_state(state, &migration_state)?;
        println!("state_updated: true");
    } else {
        println!("state_updated: false");
    }
    println!(
        "mutation_intent: {}",
        if action == "up" && !dry_run && pending_count > 0 {
            "apply_pending"
        } else if action == "down" && !dry_run {
            "rollback_latest"
        } else {
            "none"
        }
    );
    Ok(0)
}

fn cmd_migrate_down(
    dir: &Path,
    down_dir: &Path,
    state: &Path,
    database: &Path,
    dry_run: bool,
) -> Result<u8> {
    let migrations = scan_migration_files(dir)?;
    let mut migration_state = load_migration_state(state)?;
    let drift = detect_migration_drift(&migrations, &migration_state);
    println!("corvid migrate down");
    println!("migrations: {}", dir.display());
    println!("down_migrations: {}", down_dir.display());
    println!("state: {}", state.display());
    println!("database: {}", database.display());
    println!("dry_run: {dry_run}");
    println!("applied_count: {}", migration_state.migrations.len());
    println!("drift_count: {}", drift.len());
    for item in &drift {
        println!("drift: {} {}", item.kind, item.message);
    }
    if !drift.is_empty() {
        println!("drift_found: {}", drift.len());
        println!("state_updated: false");
        return Ok(1);
    }
    let Some(latest) = migration_state.migrations.last().cloned() else {
        println!("rollback: none");
        println!("state_updated: false");
        println!("mutation_intent: none");
        return Ok(0);
    };
    let rollback = rollback_migration_path(down_dir, &latest.name);
    println!("rollback: {}", latest.name);
    println!("rollback_sql: {}", rollback.display());
    if !rollback.exists() {
        println!("state_updated: false");
        return Err(anyhow::anyhow!(
            "missing rollback SQL `{}` for `{}`",
            rollback.display(),
            latest.name
        ));
    }
    if dry_run {
        println!("state_updated: false");
        println!("mutation_intent: rollback_latest");
        return Ok(0);
    }
    execute_rollback_sql(database, &rollback, &latest.name)?;
    migration_state.migrations.pop();
    save_migration_state(state, &migration_state)?;
    println!("state_updated: true");
    println!("mutation_intent: rollback_latest");
    Ok(0)
}

fn cmd_jobs_enqueue(
    state: &Path,
    task: &str,
    payload: &str,
    input_schema: Option<String>,
    max_retries: u64,
    budget_usd: f64,
    effect_summary: Option<String>,
    replay_key: Option<String>,
    delay_ms: u64,
) -> Result<u8> {
    if let Some(parent) = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create jobs state dir `{}`", parent.display()))?;
    }
    let queue = DurableQueueRuntime::open(state)?;
    let payload = serde_json::from_str(payload).context("jobs payload must be valid JSON")?;
    let next_run_ms = if delay_ms == 0 {
        None
    } else {
        Some(corvid_runtime::tracing::now_ms().saturating_add(delay_ms))
    };
    let job = queue.enqueue_typed_at(
        task,
        payload,
        input_schema,
        max_retries,
        budget_usd,
        effect_summary,
        replay_key,
        next_run_ms,
    )?;
    println!("corvid jobs enqueue");
    println!("state: {}", state.display());
    print_job_summary(&job);
    Ok(0)
}

fn cmd_jobs_run_one(
    state: &Path,
    output_kind: Option<String>,
    output_fingerprint: Option<String>,
    fail_kind: Option<String>,
    fail_fingerprint: Option<String>,
    retry_base_ms: u64,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    println!("corvid jobs run-one");
    println!("state: {}", state.display());
    let result = if let Some(kind) = fail_kind {
        queue.run_one_failed(
            kind,
            fail_fingerprint.unwrap_or_else(|| "sha256:redacted-failure".to_string()),
            retry_base_ms,
        )?
    } else {
        queue.run_one_with_output(output_kind, output_fingerprint)?
    };
    match result {
        Some(job) => {
            print_job_summary(&job);
            Ok(0)
        }
        None => {
            println!("job: none");
            Ok(0)
        }
    }
}

fn cmd_jobs_dlq(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let jobs = queue.dead_lettered()?;
    println!("corvid jobs dlq");
    println!("state: {}", state.display());
    println!("dead_lettered_count: {}", jobs.len());
    for job in jobs {
        println!(
            "dead_lettered: {} task:{} attempts:{} failure_kind:{} failure_fingerprint:{} replay_key:{}",
            job.id,
            job.task,
            job.attempts,
            job.failure_kind.as_deref().unwrap_or(""),
            job.failure_fingerprint.as_deref().unwrap_or(""),
            job.replay_key.as_deref().unwrap_or("")
        );
    }
    Ok(0)
}

fn cmd_jobs_schedule_add(
    state: &Path,
    id: &str,
    cron: &str,
    zone: &str,
    task: &str,
    payload: &str,
    max_retries: u64,
    budget_usd: f64,
    effect_summary: Option<String>,
    replay_key_prefix: Option<String>,
    missed_policy: SchedulePolicyArg,
) -> Result<u8> {
    if let Some(parent) = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create jobs state dir `{}`", parent.display()))?;
    }
    let queue = DurableQueueRuntime::open(state)?;
    let payload = serde_json::from_str(payload).context("schedule payload must be valid JSON")?;
    let now = corvid_runtime::tracing::now_ms();
    let schedule = queue.upsert_schedule(QueueScheduleManifest {
        id: id.to_string(),
        cron: cron.to_string(),
        zone: zone.to_string(),
        task: task.to_string(),
        payload,
        max_retries,
        budget_usd,
        effect_summary,
        replay_key_prefix,
        missed_policy: missed_policy.into(),
        last_checked_ms: now,
        last_fire_ms: None,
        created_ms: now,
        updated_ms: now,
    })?;
    println!("corvid jobs schedule add");
    println!("state: {}", state.display());
    print_schedule_summary(&schedule);
    Ok(0)
}

fn cmd_jobs_schedule_list(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let schedules = queue.list_schedules()?;
    println!("corvid jobs schedule list");
    println!("state: {}", state.display());
    println!("schedule_count: {}", schedules.len());
    for schedule in schedules {
        print_schedule_summary(&schedule);
    }
    Ok(0)
}

fn cmd_jobs_schedule_recover(state: &Path, max_missed_per_schedule: usize) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let report = queue.recover_schedules(max_missed_per_schedule)?;
    println!("corvid jobs schedule recover");
    println!("state: {}", state.display());
    println!("scanned: {}", report.scanned);
    println!("enqueued: {}", report.enqueued);
    println!("skipped: {}", report.skipped);
    for recovery in report.recoveries {
        println!(
            "recovery: schedule:{} task:{} fire_ms:{} action:{} job:{} policy:{}",
            recovery.schedule_id,
            recovery.task,
            recovery.fire_ms,
            recovery.action,
            recovery.job_id.as_deref().unwrap_or(""),
            recovery.policy.as_str()
        );
    }
    Ok(0)
}

fn print_schedule_summary(schedule: &QueueScheduleManifest) {
    println!("schedule: {}", schedule.id);
    println!("cron: {}", schedule.cron);
    println!("zone: {}", schedule.zone);
    println!("task: {}", schedule.task);
    println!("missed_policy: {}", schedule.missed_policy.as_str());
    println!("last_checked_ms: {}", schedule.last_checked_ms);
    println!(
        "last_fire_ms: {}",
        schedule
            .last_fire_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("max_retries: {}", schedule.max_retries);
    println!("budget_usd: {:.4}", schedule.budget_usd);
    println!(
        "effect_summary: {}",
        schedule.effect_summary.as_deref().unwrap_or("")
    );
    println!(
        "replay_key_prefix: {}",
        schedule.replay_key_prefix.as_deref().unwrap_or("")
    );
}

fn print_job_summary(job: &QueueJob) {
    println!("job: {}", job.id);
    println!("task: {}", job.task);
    println!(
        "input_schema: {}",
        job.input_schema.as_deref().unwrap_or("")
    );
    println!("status: {}", job.status.as_str());
    println!("attempts: {}", job.attempts);
    println!("max_retries: {}", job.max_retries);
    println!("budget_usd: {:.4}", job.budget_usd);
    println!(
        "effect_summary: {}",
        job.effect_summary.as_deref().unwrap_or("")
    );
    println!("replay_key: {}", job.replay_key.as_deref().unwrap_or(""));
    println!("output_kind: {}", job.output_kind.as_deref().unwrap_or(""));
    println!(
        "output_fingerprint: {}",
        job.output_fingerprint.as_deref().unwrap_or("")
    );
    println!(
        "failure_kind: {}",
        job.failure_kind.as_deref().unwrap_or("")
    );
    println!(
        "failure_fingerprint: {}",
        job.failure_fingerprint.as_deref().unwrap_or("")
    );
    println!(
        "next_run_ms: {}",
        job.next_run_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
}

struct MigrationFile {
    name: String,
    sha256: String,
    path: PathBuf,
}

struct MigrationDrift {
    kind: &'static str,
    message: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MigrationState {
    migrations: Vec<AppliedMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppliedMigration {
    name: String,
    sha256: String,
    applied_at: u64,
}

fn scan_migration_files(dir: &Path) -> Result<Vec<MigrationFile>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("cannot read migrations directory `{}`", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("cannot read entry under `{}`", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
            continue;
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("cannot read migration `{}`", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<invalid>")
            .to_string();
        let sha256 = hex::encode(Sha256::digest(&bytes));
        files.push(MigrationFile { name, sha256, path });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

fn detect_migration_drift(
    migrations: &[MigrationFile],
    state: &MigrationState,
) -> Vec<MigrationDrift> {
    let mut drift = Vec::new();
    let mut seen_versions = std::collections::HashMap::<String, String>::new();
    for migration in migrations {
        let version = migration_version(&migration.name);
        if let Some(previous) = seen_versions.insert(version.clone(), migration.name.clone()) {
            drift.push(MigrationDrift {
                kind: "duplicate",
                message: format!(
                    "version `{version}` appears in `{previous}` and `{}`",
                    migration.name
                ),
            });
        }
    }

    for applied in &state.migrations {
        match migrations
            .iter()
            .find(|migration| migration.name == applied.name)
        {
            Some(current) if current.sha256 != applied.sha256 => drift.push(MigrationDrift {
                kind: "changed",
                message: format!(
                    "`{}` expected sha256:{}, actual sha256:{}",
                    applied.name, applied.sha256, current.sha256
                ),
            }),
            Some(_) => {}
            None => drift.push(MigrationDrift {
                kind: "missing",
                message: format!("applied migration `{}` is missing from disk", applied.name),
            }),
        }
    }

    let file_order = migrations
        .iter()
        .map(|migration| migration.name.as_str())
        .collect::<Vec<_>>();
    let mut last_index = None;
    for applied in &state.migrations {
        if let Some(index) = file_order.iter().position(|name| *name == applied.name) {
            if last_index.is_some_and(|last| index < last) {
                drift.push(MigrationDrift {
                    kind: "out_of_order",
                    message: format!(
                        "applied migration `{}` is earlier than a previously applied migration",
                        applied.name
                    ),
                });
            }
            last_index = Some(index);
        }
    }

    drift
}

fn migration_version(name: &str) -> String {
    name.split_once('_')
        .map(|(version, _)| version)
        .or_else(|| name.split_once('.').map(|(version, _)| version))
        .unwrap_or(name)
        .to_string()
}

fn load_migration_state(path: &Path) -> Result<MigrationState> {
    if !path.exists() {
        return Ok(MigrationState::default());
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("cannot read migration state `{}`", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse migration state `{}`", path.display()))
}

fn save_migration_state(path: &Path, state: &MigrationState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create migration state dir `{}`", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(state).context("cannot serialize migration state")?;
    std::fs::write(path, json)
        .with_context(|| format!("cannot write migration state `{}`", path.display()))
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn cmd_check(file: &Path) -> Result<u8> {
    let source = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read `{}`", file.display()))?;
    let config = load_corvid_config_for(file);
    let result = compile_with_config(&source, config.as_ref());
    if result.ok() {
        println!("ok: {} — no errors", file.display());
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&result.diagnostics, file, &source));
        Ok(1)
    }
}

fn cmd_build(
    file: &Path,
    target: &str,
    tools_lib: Option<&Path>,
    header: bool,
    abi_descriptor: bool,
    all_artifacts: bool,
    sign_key_path: Option<&Path>,
    key_id: Option<&str>,
) -> Result<u8> {
    let header = header || all_artifacts;
    let abi_descriptor = abi_descriptor || all_artifacts;
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    if sign_key_path.is_some() && target != "cdylib" {
        anyhow::bail!(
            "`--sign` is only valid for `--target=cdylib` — descriptor attestations are bound to the embedded cdylib descriptor symbol"
        );
    }
    let extra_libs_owned: Vec<&Path> = tools_lib.iter().copied().collect();
    match target {
        "python" | "py" => {
            if tools_lib.is_some() {
                anyhow::bail!("`--with-tools-lib` is only valid for `native`, `cdylib`, and `staticlib` targets");
            }
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_to_disk(file)
                .with_context(|| format!("failed to build `{}`", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "native" => {
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_native_to_disk(file, &extra_libs_owned)
                .with_context(|| format!("failed to build `{}` (native)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "server" => {
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_server_to_disk(file, &extra_libs_owned)
                .with_context(|| format!("failed to build `{}` (server)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                if let Some(handler) = out.handler_path {
                    println!("handler: {}", handler.display());
                }
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "wasm" => {
            if tools_lib.is_some() {
                anyhow::bail!(
                    "`--with-tools-lib` is not valid for `wasm` until the Phase 23 host-capability ABI lands"
                );
            }
            if header || abi_descriptor {
                anyhow::bail!(
                    "`--header`, `--abi-descriptor`, and `--all-artifacts` are only valid for library targets"
                );
            }
            let out = build_wasm_to_disk(file)
                .with_context(|| format!("failed to build `{}` (wasm)", file.display()))?;
            if let Some(path) = out.wasm_path {
                println!("built: {} -> {}", file.display(), path.display());
                if let Some(js) = out.js_loader_path {
                    println!("loader: {}", js.display());
                }
                if let Some(types) = out.ts_types_path {
                    println!("types: {}", types.display());
                }
                if let Some(manifest) = out.manifest_path {
                    println!("manifest: {}", manifest.display());
                }
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "cdylib" => cmd_build_library(
            file,
            BuildTarget::Cdylib,
            &extra_libs_owned,
            header,
            abi_descriptor,
            sign_key_path,
            key_id,
        ),
        "staticlib" => {
            if abi_descriptor {
                anyhow::bail!(
                    "`--abi-descriptor` and `--all-artifacts` are only valid for `cdylib`"
                );
            }
            cmd_build_library(
                file,
                BuildTarget::Staticlib,
                &extra_libs_owned,
                header,
                false,
                None,
                None,
            )
        }
        other => {
            anyhow::bail!(
                "unknown target `{other}`; valid: `python` (default), `native`, `server`, `wasm`, `cdylib`, `staticlib`"
            )
        }
    }
}

fn cmd_build_library(
    file: &Path,
    target: BuildTarget,
    tools_libs: &[&Path],
    header: bool,
    abi_descriptor: bool,
    sign_key_path: Option<&Path>,
    key_id: Option<&str>,
) -> Result<u8> {
    // Resolve the signing key once at flag-parse time. The driver
    // stays string-typed; key parsing belongs at the CLI boundary
    // so failure modes surface with `--sign`'s context.
    let signing = match sign_key_path {
        Some(path) => {
            let key =
                corvid_abi::load_signing_key(&corvid_abi::KeySource::Path(path.to_path_buf()))
                    .with_context(|| format!("loading --sign key from `{}`", path.display()))?;
            Some(corvid_driver::SigningRequest {
                key,
                key_id: key_id.unwrap_or("build-key").to_string(),
            })
        }
        None => match std::env::var("CORVID_SIGNING_KEY") {
            Ok(value) if !value.is_empty() => {
                let key = corvid_abi::load_signing_key(&corvid_abi::KeySource::Env(value))
                    .context("loading signing key from CORVID_SIGNING_KEY env var")?;
                Some(corvid_driver::SigningRequest {
                    key,
                    key_id: key_id.unwrap_or("build-key").to_string(),
                })
            }
            _ => None,
        },
    };
    let out = build_target_to_disk(file, target, header, abi_descriptor, tools_libs, signing)
        .with_context(|| {
            format!(
                "failed to build `{}` ({})",
                file.display(),
                match target {
                    BuildTarget::Native => "native",
                    BuildTarget::Cdylib => "cdylib",
                    BuildTarget::Staticlib => "staticlib",
                }
            )
        })?;
    if let Some(path) = out.output_path {
        println!("built: {} -> {}", file.display(), path.display());
        if let Some(header_path) = out.header_path {
            println!("header: {}", header_path.display());
        }
        if let Some(abi_descriptor_path) = out.abi_descriptor_path {
            println!("abi descriptor: {}", abi_descriptor_path.display());
        }
        if out.signed {
            println!("attestation: signed (CORVID_ABI_ATTESTATION embedded)");
        }
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
        Ok(1)
    }
}

fn cmd_run(file: &Path, target: &str, tools_lib: Option<&Path>) -> Result<u8> {
    let rt = match target {
        "auto" => RunTarget::Auto,
        "native" => RunTarget::Native,
        "interp" | "interpreter" => RunTarget::Interpreter,
        other => anyhow::bail!(
            "unknown target `{other}`; valid: `auto` (default), `native`, `interpreter`"
        ),
    };
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    // Auto: native AOT tier when the IR is tool-free and uses only
    // supported command-line boundary types, or when tool-using code
    // has a companion tools staticlib provided.
    // Interpreter otherwise (with a stderr notice). Native-required
    // and interpreter-forced are the explicit overrides. See
    // `RunTarget` docs in corvid-driver for the exact semantics.
    run_with_target(file, rt, tools_lib)
        .with_context(|| format!("failed to run `{}`", file.display()))
}

fn cmd_repl() -> Result<u8> {
    corvid_repl::Repl::run_stdio().context("failed to run `corvid repl`")?;
    Ok(0)
}

fn cmd_verify(corpus: Option<&Path>, shrink: Option<&Path>, json: bool) -> Result<u8> {
    match (corpus, shrink) {
        (Some(dir), None) => {
            let reports = verify_corpus(dir)?;
            let divergent: Vec<_> = reports
                .iter()
                .filter(|report| !report.divergences.is_empty())
                .collect();
            println!("{}", render_corpus_grid(&reports));
            if !divergent.is_empty() {
                println!();
                for (index, report) in divergent.iter().enumerate() {
                    if index > 0 {
                        println!();
                    }
                    println!("{}", render_report(report));
                }
            }
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&reports)?);
            }
            Ok(if divergent.is_empty() { 0 } else { 1 })
        }
        (None, Some(file)) => {
            let result = shrink_program(file)?;
            println!(
                "shrunk reproducer: {} -> {} (removed {} line(s))",
                result.original.display(),
                result.output.display(),
                result.removed_lines
            );
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&result)?);
            }
            Ok(0)
        }
        (None, None) => {
            anyhow::bail!("use `corvid verify --corpus <dir>` or `corvid verify --shrink <file>`")
        }
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

// ------------------------------------------------------------
// Verification suites — effect-system spec, custom dimensions,
// adversarial bypass generation
// ------------------------------------------------------------

fn cmd_test(
    target: Option<&str>,
    meta: bool,
    site_out: Option<&Path>,
    count: u32,
    model: &str,
    update_snapshots: bool,
) -> Result<u8> {
    match target {
        None => {
            eprintln!("usage: `corvid test <file.cor>`");
            eprintln!("Legacy verification targets are still available:");
            eprintln!("  `corvid test dimensions`, `corvid test spec`,");
            eprintln!(
                "  `corvid test spec --meta`, `corvid test rewrites`, `corvid test adversarial --count <N>`."
            );
            Ok(1)
        }
        Some("dimensions") => cmd_test_dimensions(),
        Some("spec") if site_out.is_some() => cmd_test_spec_site(site_out.unwrap()),
        Some("spec") if meta => cmd_test_spec_meta(),
        Some("spec") => cmd_test_spec(),
        Some("rewrites") => cmd_test_rewrites(),
        Some("adversarial") => cmd_test_adversarial(count, model),
        Some(other) if other.ends_with(".cor") || Path::new(other).exists() => {
            cmd_test_file(Path::new(other), update_snapshots)
        }
        Some(other) => {
            anyhow::bail!(
                "unknown test target `{other}`; pass a `.cor` file or one of: `dimensions`, `spec`, `spec --meta`, `rewrites`, `adversarial`"
            )
        }
    }
}

fn cmd_test_file(path: &Path, update_snapshots: bool) -> Result<u8> {
    let dotenv_start = path.parent().unwrap_or_else(|| Path::new("."));
    load_dotenv_walking(dotenv_start);
    let runtime = corvid_driver::Runtime::builder().build();
    let source = std::fs::read_to_string(path).ok();
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async test runtime")?;
    let report = tokio
        .block_on(run_tests_at_path_with_options(
            path,
            &runtime,
            test_options(path, update_snapshots),
        ))
        .map_err(anyhow::Error::new)?;
    print!("{}", render_test_report(&report, source.as_deref()));
    Ok(report.exit_code())
}

fn cmd_cost_frontier(
    prompt: &str,
    trace_dir: Option<&Path>,
    since: Option<&str>,
    since_commit: Option<&str>,
    json: bool,
) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_frontier(CostFrontierOptions {
        prompt,
        trace_dir,
        since,
        since_commit,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_cost_frontier(&report));
    }
    Ok(if report.has_quality_evidence { 0 } else { 1 })
}

fn cmd_import_summary(file: &Path, json: bool) -> Result<u8> {
    let summaries = inspect_import_semantics(file)?;
    if json {
        let payload = summaries
            .iter()
            .map(|summary| {
                serde_json::json!({
                    "import": summary.import,
                    "path": summary.path.display().to_string(),
                    "content_hash": summary.content_hash,
                    "summary": summary.summary,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print!("{}", render_import_semantic_summaries(&summaries));
    }
    Ok(0)
}

fn cmd_test_dimensions() -> Result<u8> {
    println!("corvid test dimensions — archetype law-check suite");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config_with_path = load_corvid_config_with_path_for(&cwd.join("anywhere.cor"));
    let config = config_with_path.as_ref().map(|(_, config)| config);
    match config {
        Some(cfg) => {
            let n = cfg.effect_system.dimensions.len();
            println!(
                "reading corvid.toml — {n} custom dimension{}",
                if n == 1 { "" } else { "s" }
            );
        }
        None => println!("no corvid.toml found — running law checks on built-ins only"),
    }
    println!("running {DEFAULT_SAMPLES} cases per law…");
    let config_dir = config_with_path
        .as_ref()
        .and_then(|(path, _)| path.parent());
    let report = run_dimension_verification(config, config_dir, DEFAULT_SAMPLES);
    print!("{}", render_dimension_verification_report(&report));
    let law_failures = report
        .laws
        .iter()
        .filter(|r| matches!(r.verdict, corvid_driver::LawVerdict::CounterExample { .. }))
        .count();
    let proof_failures = report.proofs.iter().filter(|p| p.failed()).count();
    Ok(if law_failures == 0 && proof_failures == 0 {
        0
    } else {
        1
    })
}

fn cmd_test_spec() -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec` from the repository root"
        );
    }
    println!(
        "corvid test spec — verify every fenced corvid block in {}\n",
        spec_dir.display()
    );
    let verdicts = verify_spec_examples(&spec_dir)
        .with_context(|| format!("failed to verify `{}`", spec_dir.display()))?;
    print!("{}", render_spec_report(&verdicts));
    let failed = verdicts
        .iter()
        .filter(|v| matches!(v.kind, VerdictKind::Fail { .. }))
        .count();
    Ok(if failed == 0 { 0 } else { 1 })
}

fn cmd_test_spec_site(out_dir: &Path) -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec --site-out <DIR>` from the repository root"
        );
    }
    println!(
        "corvid test spec --site-out {} — render executable spec site\n",
        out_dir.display()
    );
    let report = build_spec_site(&spec_dir, out_dir)
        .with_context(|| format!("failed to render spec site to `{}`", out_dir.display()))?;
    print!("{}", render_spec_site_report(&report));
    Ok(0)
}

fn cmd_test_spec_meta() -> Result<u8> {
    println!("corvid test spec --meta — self-verifying verification\n");
    let corpus_dir = PathBuf::from("docs/effects-spec/counterexamples/composition");
    if !corpus_dir.exists() {
        anyhow::bail!(
            "counter-example corpus not found at `{}`; run from the repository root",
            corpus_dir.display()
        );
    }
    let verdicts = corvid_driver::verify_counterexample_corpus(&corpus_dir)
        .context("failed to run meta-verification harness")?;
    print!("{}", corvid_driver::render_meta_report(&verdicts));
    let failed = verdicts
        .iter()
        .filter(|v| !matches!(v.kind, corvid_driver::MetaKind::Distinguishes))
        .count();
    Ok(if failed == 0 { 0 } else { 1 })
}

fn cmd_test_rewrites() -> Result<u8> {
    println!("corvid test rewrites — preserved-semantics effect-profile fuzzing\n");
    println!("Each rewrite row names the semantic law it is obligated to preserve.");
    println!(
        "If a profile drifts, the failure report names the rewrite rule, law, line, and shrunk reproducer.\n"
    );
    let matrix = corvid_differential_verify::fuzz::build_coverage_matrix()
        .context("preserved-semantics rewrite verification failed")?;
    print!(
        "{}",
        corvid_differential_verify::fuzz::render_coverage_matrix(&matrix)
    );
    println!();
    Ok(0)
}

fn cmd_test_adversarial(count: u32, model: &str) -> Result<u8> {
    println!("corvid test adversarial --count {count} --model {model}\n");
    let mut report = run_adversarial_suite(count, model);
    file_github_issues_for_escapes(&mut report)?;
    print!("{}", render_adversarial_report(&report));
    Ok(if report.escaped_count == 0 { 0 } else { 1 })
}

// ------------------------------------------------------------
// Effect-diff tool
// ------------------------------------------------------------

fn cmd_effect_diff(before: &str, after: &str) -> Result<u8> {
    let before_path = PathBuf::from(before);
    let after_path = PathBuf::from(after);
    println!(
        "corvid effect-diff {} -> {}\n",
        before_path.display(),
        after_path.display(),
    );
    let before_snap = snapshot_revision(&before_path)
        .with_context(|| format!("failed to snapshot `{}`", before_path.display()))?;
    let after_snap = snapshot_revision(&after_path)
        .with_context(|| format!("failed to snapshot `{}`", after_path.display()))?;
    let diff = diff_snapshots(&before_snap, &after_snap);
    print!("{}", render_effect_diff(&diff));
    // Exit 1 when the diff is non-empty so CI can gate on
    // "unexpected effect-shape drift" if the user wants it.
    let any_change = !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
    Ok(if any_change { 1 } else { 0 })
}

// ------------------------------------------------------------
// Dimension registry client
// ------------------------------------------------------------

fn cmd_add_dimension(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add-dimension {spec}\n");
    let outcome = corvid_driver::install_dimension_with_registry(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::AddDimensionOutcome::Added { name, target } => {
            println!("installed `{name}` into {}", target.display());
            println!("run `corvid test dimensions` to re-verify every dimension.");
            Ok(0)
        }
        corvid_driver::AddDimensionOutcome::Rejected { reason } => {
            eprintln!("rejected: {reason}");
            Ok(1)
        }
    }
}

fn cmd_add_package(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add {spec}\n");
    let outcome = corvid_driver::add_package(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::AddPackageOutcome::Added {
            uri,
            version,
            lockfile,
            exports,
        } => {
            println!(
                "added `{uri}` ({version}) to {} with {exports} exported contract item{}",
                lockfile.display(),
                if exports == 1 { "" } else { "s" }
            );
            Ok(0)
        }
        corvid_driver::AddPackageOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
    }
}

fn cmd_remove_package(name: &str) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid remove {name}\n");
    let outcome = corvid_driver::remove_package(name, &project_dir)?;
    match outcome {
        corvid_driver::PackageMutationOutcome::Removed {
            name,
            manifest_updated,
            lock_entries_removed,
            lockfile,
        } => {
            println!(
                "removed `{name}` (manifest: {}, lock entries: {})",
                if manifest_updated {
                    "updated"
                } else {
                    "unchanged"
                },
                lock_entries_removed
            );
            println!("lockfile: {}", lockfile.display());
            Ok(0)
        }
        corvid_driver::PackageMutationOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
        corvid_driver::PackageMutationOutcome::Updated { .. } => unreachable!(),
    }
}

fn cmd_update_package(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid update {spec}\n");
    let outcome = corvid_driver::update_package(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::PackageMutationOutcome::Updated {
            uri,
            version,
            lockfile,
            exports,
        } => {
            println!(
                "updated `{uri}` ({version}) in {} with {exports} exported contract item{}",
                lockfile.display(),
                if exports == 1 { "" } else { "s" }
            );
            Ok(0)
        }
        corvid_driver::PackageMutationOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
        corvid_driver::PackageMutationOutcome::Removed { .. } => unreachable!(),
    }
}

fn cmd_package_publish(
    source: &Path,
    name: &str,
    version: &str,
    out: &Path,
    url_base: &str,
    key: &str,
    key_id: &str,
) -> Result<u8> {
    let outcome = corvid_driver::publish_package(corvid_driver::PublishPackageOptions {
        source,
        name,
        version,
        out_dir: out,
        url_base,
        signing_seed_hex: key,
        key_id,
    })?;
    println!(
        "published `{}` to {}\nartifact: {}\nsha256: {}",
        outcome.uri,
        outcome.index.display(),
        outcome.artifact.display(),
        outcome.sha256
    );
    Ok(0)
}

fn cmd_package_metadata(
    source: &Path,
    name: &str,
    version: &str,
    signature: Option<&str>,
    json: bool,
) -> Result<u8> {
    let metadata = corvid_driver::package_metadata_from_source(source, name, version, signature)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        print!(
            "{}",
            corvid_driver::render_package_metadata_markdown(&metadata)
        );
    }
    Ok(0)
}

fn cmd_package_verify_registry(registry: &str, json: bool) -> Result<u8> {
    let report = corvid_driver::verify_registry_contract(registry)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("corvid package verify-registry {registry}\n");
        println!("checked package entries: {}", report.checked);
        if report.failures.is_empty() {
            println!("registry contract: ok");
        } else {
            println!("registry contract: failed");
            for failure in &report.failures {
                println!(
                    "- {}@{}: {}",
                    failure.package, failure.version, failure.reason
                );
            }
        }
    }
    Ok(if report.is_clean() { 0 } else { 1 })
}

fn cmd_package_verify_lock(json: bool) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let report = corvid_driver::verify_package_lock(&project_dir)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", corvid_driver::render_package_conflict_report(&report));
    }
    Ok(if report.is_clean() { 0 } else { 1 })
}

fn cmd_routing_report(
    trace_dir: Option<&Path>,
    since: Option<&str>,
    since_commit: Option<&str>,
    json: bool,
) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_report(RoutingReportOptions {
        trace_dir,
        since,
        since_commit,
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_routing_report(&report));
    }
    Ok(if report.healthy { 0 } else { 1 })
}

// ------------------------------------------------------------
// corvid doctor
// ------------------------------------------------------------

#[allow(dead_code)]
fn cmd_doctor() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor — checking local environment...\n");

    // Try loading .env first so the rest of the checks see what programs would.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  ✓ .env loaded from {}", p.display()),
        None => println!("  · no .env file found from cwd upward (optional)"),
    }

    // CORVID_MODEL
    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  ✓ CORVID_MODEL = {v}"),
        None => println!(
            "  · CORVID_MODEL not set. Set one (e.g. `export CORVID_MODEL=gpt-4o-mini` or\n    `claude-opus-4-6`) or put `default_model = \"...\"` in corvid.toml under [llm]."
        ),
    }

    // Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  ✓ ANTHROPIC_API_KEY set (Claude models available)");
    } else {
        println!("  · ANTHROPIC_API_KEY not set — Claude calls will error at the prompt site");
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        println!("  ✓ OPENAI_API_KEY set (GPT / o-series models available)");
    } else {
        println!("  · OPENAI_API_KEY not set — OpenAI calls will error at the prompt site");
    }

    // Cross-check: model prefix vs. available keys.
    if let Some(m) = &model {
        if m.starts_with("claude-") && std::env::var("ANTHROPIC_API_KEY").is_err() {
            println!("  ✗ CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set");
        }
        let openai_prefixes = ["gpt-", "o1-", "o3-", "o4-"];
        if openai_prefixes.iter().any(|p| m.starts_with(p))
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            println!("  ✗ CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set");
        }
    }

    // Python (legacy `--target=python` users only)
    let has_python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_python {
        println!("  · python3 detected (legacy `--target=python` available)");
    } else {
        println!("  · python3 not detected (only needed for `--target=python`)");
    }

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the");
    println!("matching API key and prompt-only programs run end-to-end.");
    Ok(0)
}

fn cmd_doctor_v2() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor - checking local environment...\n");
    let mut ok = true;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  OK .env loaded from {}", p.display()),
        None => println!("  .. no .env file found from cwd upward (optional)"),
    }

    if command_succeeds("cargo", &["--version"]) {
        println!("  OK cargo detected");
    } else {
        ok = false;
        println!("  XX cargo not found in PATH");
    }

    if command_succeeds("rustc", &["--version"]) {
        println!("  OK rustc detected");
    } else {
        ok = false;
        println!("  XX rustc not found in PATH");
    }

    if command_output("rustup", &["target", "list", "--installed"])
        .map(|stdout| stdout.contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
    {
        println!("  OK wasm32-unknown-unknown target installed");
    } else {
        println!("  .. wasm32-unknown-unknown target missing (`rustup target add wasm32-unknown-unknown`)");
    }

    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  OK CORVID_MODEL = {v}"),
        None => println!("  .. CORVID_MODEL not set"),
    }

    let anthropic = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let openai = std::env::var("OPENAI_API_KEY").is_ok();
    println!(
        "  {} ANTHROPIC_API_KEY {}",
        if anthropic { "OK" } else { ".." },
        if anthropic { "set" } else { "not set" }
    );
    println!(
        "  {} OPENAI_API_KEY {}",
        if openai { "OK" } else { ".." },
        if openai { "set" } else { "not set" }
    );

    if let Some(m) = &model {
        if m.starts_with("claude-") && !anthropic {
            ok = false;
            println!("  XX CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set");
        }
        if ["gpt-", "o1-", "o3-", "o4-"]
            .iter()
            .any(|prefix| m.starts_with(prefix))
            && !openai
        {
            ok = false;
            println!("  XX CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set");
        }
    }

    println!(
        "  {} ollama {}",
        if command_succeeds("ollama", &["--version"]) {
            "OK"
        } else {
            ".."
        },
        if command_succeeds("ollama", &["--version"]) {
            "detected"
        } else {
            "not detected (only needed for local-model demos)"
        }
    );

    let trace_dir = cwd.join("target").join("trace");
    if trace_dir.exists() {
        println!("  OK replay storage present at {}", trace_dir.display());
    } else {
        println!(
            "  .. replay storage not initialized yet (expected at {})",
            trace_dir.display()
        );
    }

    match std::env::var("CORVID_APPROVER").ok() {
        Some(value) => println!("  OK CORVID_APPROVER = {value}"),
        None => println!("  .. CORVID_APPROVER not set"),
    }

    if !check_u16_env("CORVID_PORT", "backend listen port") {
        ok = false;
    }
    if !check_u64_env("CORVID_HANDLER_TIMEOUT_MS", "backend handler timeout") {
        ok = false;
    }
    if !check_positive_u64_env(
        "CORVID_MAX_REQUESTS",
        "backend graceful drain request limit",
    ) {
        ok = false;
    }
    if !check_hex_key_env("CORVID_TOKEN_KEY", 64, "connector token encryption key") {
        ok = false;
    }

    match find_upward(&cwd, "Corvid.lock") {
        Some(path) => println!("  OK registry lockfile found at {}", path.display()),
        None => println!("  .. no Corvid.lock found from cwd upward"),
    }

    let has_python =
        command_succeeds("python3", &["--version"]) || command_succeeds("python", &["--version"]);
    println!(
        "  .. {}",
        if has_python {
            "python detected (legacy --target=python available)"
        } else {
            "python not detected (only needed for --target=python)"
        }
    );

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the matching API key and prompt-only programs run end-to-end.");
    Ok(if ok { 0 } else { 1 })
}

fn command_succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                None
            }
        })
}

#[derive(Subcommand)]
enum MigrateCommand {
    /// Report applied, pending, and drifted migrations.
    Status {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file migrations are checked against.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Show what would be checked without writing state.
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply pending migrations in order.
    Up {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file migrations are executed against.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Report pending migrations without applying them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Roll back the latest migration when a down migration exists.
    Down {
        /// Directory containing ordered `.sql` migration files.
        #[arg(long, value_name = "DIR", default_value = "migrations")]
        dir: PathBuf,
        /// Directory containing reviewed rollback SQL files named `<migration>.down.sql`.
        #[arg(long, value_name = "DIR", default_value = "migrations/down")]
        down_dir: PathBuf,
        /// State file used to record applied migration checksums.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "target/corvid-migrations.json"
        )]
        state: PathBuf,
        /// SQLite database file associated with migration state.
        #[arg(long, value_name = "PATH", default_value = "target/corvid.sqlite")]
        database: PathBuf,
        /// Report the rollback candidate without mutating state.
        #[arg(long)]
        dry_run: bool,
    },
}

fn apply_pending_sql_migrations(
    database: &Path,
    migrations: &[MigrationFile],
    state: &mut MigrationState,
) -> Result<()> {
    if let Some(parent) = database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create database dir `{}`", parent.display()))?;
    }
    let mut conn = Connection::open(database)
        .with_context(|| format!("cannot open migration database `{}`", database.display()))?;
    for migration in migrations {
        let applied = state
            .migrations
            .iter()
            .any(|applied| applied.name == migration.name && applied.sha256 == migration.sha256);
        if applied {
            continue;
        }
        let sql = std::fs::read_to_string(&migration.path)
            .with_context(|| format!("cannot read migration SQL `{}`", migration.path.display()))?;
        let tx = conn
            .transaction()
            .with_context(|| format!("cannot start transaction for `{}`", migration.name))?;
        tx.execute_batch(&sql)
            .with_context(|| format!("cannot execute migration `{}`", migration.name))?;
        tx.commit()
            .with_context(|| format!("cannot commit migration `{}`", migration.name))?;
        state.migrations.push(AppliedMigration {
            name: migration.name.clone(),
            sha256: migration.sha256.clone(),
            applied_at: now_unix_seconds(),
        });
    }
    Ok(())
}

fn rollback_migration_path(down_dir: &Path, applied_name: &str) -> PathBuf {
    down_dir.join(format!("{applied_name}.down.sql"))
}

fn execute_rollback_sql(database: &Path, rollback: &Path, applied_name: &str) -> Result<()> {
    let sql = std::fs::read_to_string(rollback)
        .with_context(|| format!("cannot read rollback SQL `{}`", rollback.display()))?;
    let mut conn = Connection::open(database)
        .with_context(|| format!("cannot open migration database `{}`", database.display()))?;
    let tx = conn
        .transaction()
        .with_context(|| format!("cannot start rollback transaction for `{applied_name}`"))?;
    tx.execute_batch(&sql)
        .with_context(|| format!("cannot execute rollback for `{applied_name}`"))?;
    tx.commit()
        .with_context(|| format!("cannot commit rollback for `{applied_name}`"))?;
    Ok(())
}

#[derive(Subcommand)]
enum JobsCommand {
    /// Persist a new local background job.
    Enqueue {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job kind or task name.
        #[arg(long)]
        task: String,
        /// Redacted JSON input payload for the job.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Typed input schema name carried with the persisted job.
        #[arg(long)]
        input_schema: Option<String>,
        /// Maximum retry count available to later retry policies.
        #[arg(long, default_value = "3")]
        max_retries: u64,
        /// Budget carried with the job metadata.
        #[arg(long, default_value = "0")]
        budget_usd: f64,
        /// Human-readable effect summary, for audit output.
        #[arg(long)]
        effect_summary: Option<String>,
        /// Replay key linking the job to trace/replay metadata.
        #[arg(long)]
        replay_key: Option<String>,
        /// Persist the job for a future run after this many milliseconds.
        #[arg(long, default_value = "0")]
        delay_ms: u64,
    },
    /// Execute the first pending local job and persist the result metadata.
    RunOne {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Typed output kind recorded after the job completes.
        #[arg(long)]
        output_kind: Option<String>,
        /// Redacted output fingerprint recorded after the job completes.
        #[arg(long)]
        output_fingerprint: Option<String>,
        /// Record this run as a failed attempt with the given redacted kind.
        #[arg(long)]
        fail_kind: Option<String>,
        /// Redacted failure fingerprint for failed attempts.
        #[arg(long)]
        fail_fingerprint: Option<String>,
        /// Base backoff in milliseconds for failed attempts.
        #[arg(long, default_value = "1000")]
        retry_base_ms: u64,
    },
    /// Manage durable cron schedules and restart recovery.
    Schedule {
        #[command(subcommand)]
        command: JobsScheduleCommand,
    },
    /// Inspect terminally failed local jobs.
    Dlq {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SchedulePolicyArg {
    SkipMissed,
    FireOnceOnRecovery,
    EnqueueAllBounded,
}

impl From<SchedulePolicyArg> for ScheduleMissedPolicy {
    fn from(value: SchedulePolicyArg) -> Self {
        match value {
            SchedulePolicyArg::SkipMissed => ScheduleMissedPolicy::SkipMissed,
            SchedulePolicyArg::FireOnceOnRecovery => ScheduleMissedPolicy::FireOnceOnRecovery,
            SchedulePolicyArg::EnqueueAllBounded => ScheduleMissedPolicy::EnqueueAllBounded,
        }
    }
}

#[derive(Subcommand)]
enum JobsScheduleCommand {
    /// Add or update a durable cron schedule.
    Add {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Stable schedule id.
        #[arg(long)]
        id: String,
        /// Cron expression. Five-field expressions are accepted and normalized to second=0.
        #[arg(long)]
        cron: String,
        /// IANA timezone, such as UTC or America/New_York.
        #[arg(long, default_value = "UTC")]
        zone: String,
        /// Job kind or task name to enqueue when the schedule fires.
        #[arg(long)]
        task: String,
        /// Redacted JSON payload embedded into each recovered job.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Maximum retry count for jobs created by this schedule.
        #[arg(long, default_value = "3")]
        max_retries: u64,
        /// Budget carried into jobs created by this schedule.
        #[arg(long, default_value = "0")]
        budget_usd: f64,
        /// Human-readable effect summary for audit and operations output.
        #[arg(long)]
        effect_summary: Option<String>,
        /// Prefix used to create deterministic replay keys per scheduled fire.
        #[arg(long)]
        replay_key_prefix: Option<String>,
        /// Missed-fire policy applied after restart.
        #[arg(long, value_enum, default_value = "fire-once-on-recovery")]
        missed_policy: SchedulePolicyArg,
    },
    /// List durable cron schedules.
    List {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
    /// Recover missed schedule fires after restart.
    Recover {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Maximum missed fires to inspect per schedule.
        #[arg(long, default_value = "16")]
        max_missed_per_schedule: usize,
    },
}

fn check_u16_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) if value.parse::<u16>().is_ok() => {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_u64_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) if value.parse::<u64>().is_ok() => {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_positive_u64_env(name: &str, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.parse::<u64>() {
            Ok(parsed) if parsed > 0 => {
                println!("  OK {name} valid ({label})");
                true
            }
            _ => {
                println!("  XX {name} invalid ({label}); value redacted");
                false
            }
        },
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn check_hex_key_env(name: &str, expected_len: usize, label: &str) -> bool {
    match std::env::var(name) {
        Ok(value)
            if value.len() == expected_len && value.chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            println!("  OK {name} valid ({label})");
            true
        }
        Ok(_) => {
            println!("  XX {name} invalid ({label}); value redacted");
            false
        }
        Err(_) => {
            println!("  .. {name} not set ({label})");
            true
        }
    }
}

fn find_upward(start: &Path, name: &str) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}
