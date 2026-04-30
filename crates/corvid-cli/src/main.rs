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
mod approvals_cmd;
mod auth_cmd;
mod build_cmd;
mod cli;
mod commands;
mod doctor_cmd;
mod format;
mod migrate_cmd;
mod package_cmd;
mod run_cmd;
mod verify_cmd;

use build_cmd::cmd_build;
use cli::jobs::*;
use cli::migrate::*;
use cli::observe::*;
use cli::package::*;
use commands::eval::*;
use commands::jobs::*;
use commands::misc::*;
use commands::test::*;
use doctor_cmd::cmd_doctor_v2;
use migrate_cmd::{cmd_migrate, cmd_migrate_down};
use package_cmd::{
    cmd_add_package, cmd_package_metadata, cmd_package_publish, cmd_package_verify_lock,
    cmd_package_verify_registry, cmd_remove_package, cmd_update_package,
};
use run_cmd::cmd_run;
use verify_cmd::cmd_verify;
use format::{
    approval_summary_value, approvals_inspect_summary, approvals_queue_summary,
    audit_event_value,
};
mod bench_cmd;
mod bind_cmd;
mod bundle_cmd;
mod capsule_cmd;
mod claim_cmd;
mod connectors_cmd;
mod contract_cmd;
mod cost_frontier;
mod deploy_cmd;
mod eval_cmd;
mod lineage_cmd;
mod observe_cmd;
mod observe_helpers_cmd;
mod receipt_cache;
mod receipt_cmd;
mod release_cmd;
mod replay;
mod routing_report;
mod test_from_traces;
mod tour;
mod trace_cmd;
mod trace_dag;
mod trace_diff;
mod upgrade_cmd;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

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
enum ConnectorsCommand {
    /// List shipped connectors with their modes, scopes, and rate limits.
    List {
        /// Emit machine-readable JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },
    /// Validate every shipped connector manifest. Pass `--live` to
    /// detect contract drift against the real provider; that path
    /// requires `CORVID_PROVIDER_LIVE=1` and lands end-to-end in
    /// slice 41M.
    Check {
        /// Detect manifest-vs-provider drift via real HTTP calls.
        #[arg(long)]
        live: bool,
        /// Emit machine-readable JSON instead of a human report.
        #[arg(long)]
        json: bool,
    },
    /// Drive a connector operation against the chosen mode.
    Run {
        /// Connector name (gmail | slack | tasks | ms365 | calendar | files).
        #[arg(long)]
        connector: String,
        /// Operation name as defined in the connector manifest's
        /// replay rules (e.g. `search`, `read_metadata`, `draft`,
        /// `send`, `github_search`, `github_write`, `channel_read`).
        #[arg(long)]
        operation: String,
        /// Scope id from the connector's manifest (e.g.
        /// `gmail.read_metadata`).
        #[arg(long)]
        scope: String,
        /// Execution mode: `mock` (default), `replay`, `real`. Real
        /// requires `CORVID_PROVIDER_LIVE=1` and per-provider
        /// credentials.
        #[arg(long, default_value = "mock")]
        mode: String,
        /// JSON payload to forward to the operation (file path).
        #[arg(long, value_name = "FILE")]
        payload: Option<PathBuf>,
        /// JSON file with the canned mock response (mock/replay only).
        #[arg(long, value_name = "FILE")]
        mock: Option<PathBuf>,
        /// Approval id (required for write scopes).
        #[arg(long, default_value = "")]
        approval_id: String,
        /// Replay key (deterministic per logical operation).
        #[arg(long, default_value = "cli-run")]
        replay_key: String,
        /// Tenant id for the call.
        #[arg(long, default_value = "tenant-cli")]
        tenant_id: String,
        /// Actor id for the call.
        #[arg(long, default_value = "actor-cli")]
        actor_id: String,
        /// Token id (the encrypted-token reference; not the bearer).
        #[arg(long, default_value = "token-cli")]
        token_id: String,
        /// `now_ms` for rate-limit accounting (defaults to system time).
        #[arg(long)]
        now_ms: Option<u64>,
    },
    /// OAuth2 token lifecycle commands.
    Oauth {
        #[command(subcommand)]
        command: ConnectorsOauthCommand,
    },
    /// Verify an inbound webhook payload's HMAC-SHA256 signature
    /// against a manifest-declared secret stored in an env var.
    /// Exits 0 on a valid signature, 1 on mismatch. Pass
    /// `--provider github|slack|linear` to use the per-provider
    /// header conventions from
    /// `corvid-connector-runtime::webhook_verify` (Slack includes
    /// timestamp replay protection); without `--provider`, the
    /// generic HMAC-SHA256 verifier consumes the `--signature`
    /// value directly.
    VerifyWebhook {
        /// Provider's signature header value (e.g. `sha256=...`).
        /// Required for the generic mode; ignored when
        /// `--provider` is set (the per-provider verifier reads
        /// the `--header` entries instead).
        #[arg(long, default_value = "")]
        signature: String,
        /// Env-var name holding the shared HMAC secret.
        #[arg(long)]
        secret_env: String,
        /// File containing the raw webhook body bytes.
        #[arg(long, value_name = "FILE")]
        body_file: PathBuf,
        /// Provider preset: `github`, `slack`, or `linear`. Selects
        /// the per-provider header conventions and (Slack) the
        /// timestamp replay-protection window.
        #[arg(long)]
        provider: Option<String>,
        /// `Header-Name=value` pair (repeatable) to feed into the
        /// per-provider verifier. Required for `--provider` modes
        /// (e.g. `--header X-Hub-Signature-256=sha256=...` for
        /// github, plus `X-Slack-Signature` and
        /// `X-Slack-Request-Timestamp` for slack).
        #[arg(long = "header", value_name = "NAME=VALUE")]
        headers: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ConnectorsOauthCommand {
    /// Initiate an OAuth2 PKCE authorization flow. Generates a
    /// state + code verifier + code challenge and prints the
    /// provider's authorization URL the user should open.
    Init {
        /// Provider: `gmail`, `slack`, or `ms365`.
        provider: String,
        /// OAuth2 client id registered with the provider.
        #[arg(long)]
        client_id: String,
        /// Redirect URI registered with the provider. Defaults to
        /// `http://localhost:8765/oauth/callback`.
        #[arg(long)]
        redirect_uri: Option<String>,
        /// OAuth2 scopes (repeatable). Defaults to provider-shipped
        /// minimums (gmail: readonly + compose + send, slack:
        /// channels:history + chat:write, ms365: Mail.Read + Mail.Send + Calendars.Read).
        #[arg(long)]
        scope: Vec<String>,
    },
    /// Force-rotate an OAuth2 token by exercising the refresh
    /// endpoint with the supplied `(access, refresh)` pair. Prints
    /// the new pair so the operator can persist it. The production
    /// path consults the encrypted token store; this CLI surface
    /// is dev-friendly.
    Rotate {
        /// Provider: `gmail` or `slack`.
        provider: String,
        /// Token id to associate with the rotated tokens.
        #[arg(long)]
        token_id: String,
        /// Current access token (the soon-to-be-stale one).
        #[arg(long)]
        access_token: String,
        /// Current refresh token.
        #[arg(long)]
        refresh_token: String,
        /// OAuth2 client id.
        #[arg(long)]
        client_id: String,
        /// OAuth2 client secret.
        #[arg(long)]
        client_secret: String,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Initialise both the auth store (sessions / API keys /
    /// OAuth state) and the approval queue store. Idempotent —
    /// safe to re-run on every deploy.
    Migrate {
        #[arg(long, value_name = "PATH", default_value = "target/auth.db")]
        auth_state: PathBuf,
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
    },
    /// Manage API keys for service actors.
    Keys {
        #[command(subcommand)]
        command: AuthKeysCommand,
    },
}

#[derive(Subcommand)]
enum AuthKeysCommand {
    /// Issue a new API key. The raw key is supplied by the caller
    /// (typically generated by an HMAC-SHA256 of a host RNG seed
    /// in the deploy pipeline) and stored only as an Argon2id
    /// hash. The raw value is echoed back in the output once.
    Issue {
        #[arg(long, value_name = "PATH", default_value = "target/auth.db")]
        auth_state: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        service_actor: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        raw_key: String,
        #[arg(long, default_value = "scope:default")]
        scope_fingerprint: String,
        #[arg(long, default_value = "Service")]
        display_name: String,
        /// Expiry timestamp in milliseconds since epoch. Defaults
        /// to never (u64::MAX).
        #[arg(long)]
        expires_at_ms: Option<u64>,
    },
    /// Revoke a previously-issued API key. Idempotent.
    Revoke {
        #[arg(long, value_name = "PATH", default_value = "target/auth.db")]
        auth_state: PathBuf,
        #[arg(long)]
        key_id: String,
    },
    /// Rotate an API key: revoke the old key, create a new key
    /// with the same scope fingerprint for the same service actor.
    Rotate {
        #[arg(long, value_name = "PATH", default_value = "target/auth.db")]
        auth_state: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        service_actor: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        new_key_id: String,
        #[arg(long)]
        new_raw_key: String,
        #[arg(long)]
        expires_at_ms: Option<u64>,
    },
}

#[derive(Subcommand)]
enum ApprovalsCommand {
    /// List approvals for a tenant, optionally filtered by status.
    Queue {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        /// Filter by status: `pending`, `approved`, `denied`, `expired`.
        #[arg(long)]
        status: Option<String>,
    },
    /// Inspect a single approval — record + every audit event.
    Inspect {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        approval_id: String,
    },
    /// Approve a pending approval.
    Approve {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        reason: Option<String>,
        approval_id: String,
    },
    /// Deny a pending approval.
    Deny {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        reason: Option<String>,
        approval_id: String,
    },
    /// Expire a pending approval whose contract expiry has passed.
    Expire {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: Option<String>,
        approval_id: String,
    },
    /// Add a comment to an approval — does not change status.
    Comment {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        comment: String,
        approval_id: String,
    },
    /// Delegate a pending approval to another actor.
    Delegate {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        role: String,
        #[arg(long = "to")]
        delegate_to: String,
        #[arg(long)]
        reason: Option<String>,
        approval_id: String,
    },
    /// Approve multiple pending approvals in one invocation.
    /// Per-approval failures (wrong role, wrong tenant, already
    /// resolved) are reported individually rather than aborting
    /// the whole batch.
    Batch {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        reason: Option<String>,
        /// Approval ids (repeatable).
        #[arg(long = "id", value_name = "ID")]
        ids: Vec<String>,
    },
    /// Export every approval (with full audit trail) for a tenant
    /// since the supplied timestamp. The output is the auditable
    /// transcript a compliance review consumes.
    Export {
        #[arg(long, value_name = "PATH", default_value = "target/approvals.db")]
        approvals_state: PathBuf,
        #[arg(long)]
        tenant: String,
        /// Lower bound timestamp in ms since epoch.
        #[arg(long)]
        since_ms: Option<u64>,
        /// Output file. If omitted, prints to stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ClaimCommand {
    /// Audit launch-facing claims for runnable evidence or explicit non-scope status.
    Audit {
        /// Claim inventory markdown table.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "docs/launch-claim-audit.md"
        )]
        inventory: PathBuf,
        /// Emit JSON report.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DeployCommand {
    /// Emit a deploy package containing Dockerfile and OCI metadata.
    Package {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Docker Compose deployment artifacts.
    Compose {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Fly.io and Render-style single-service deployment artifacts.
    Paas {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit Kubernetes manifests.
    K8s {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Emit systemd service, sysusers, and tmpfiles artifacts.
    Systemd {
        /// App directory, e.g. examples/backend/personal_executive_agent.
        app: PathBuf,
        /// Output directory for generated artifacts.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum UpgradeCommand {
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
    match std::thread::Builder::new()
        .name("corvid-cli".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(main_impl)
    {
        Ok(handle) => handle.join().unwrap_or_else(|_| {
            eprintln!("error: corvid CLI worker thread panicked");
            ExitCode::from(101)
        }),
        Err(err) => {
            eprintln!("error: failed to start corvid CLI worker thread: {err}");
            ExitCode::from(2)
        }
    }
}

fn main_impl() -> ExitCode {
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
            promote_out,
        }) => eval_cmd::run_eval(
            &inputs,
            source.as_deref(),
            swap_model.as_deref(),
            max_spend,
            golden_traces.as_deref(),
            promote_out.as_deref(),
        ),
        Some(Command::EvalDrift {
            baseline,
            candidate,
            explain,
        }) => cmd_eval_drift(baseline, candidate, explain),
        Some(Command::EvalFromFeedback {
            feedback,
            trace_dir,
            out,
        }) => cmd_eval_from_feedback(feedback, trace_dir, out),
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
            TraceCommand::Lineage {
                id_or_path,
                trace_dir,
            } => lineage_cmd::run_lineage(&id_or_path, trace_dir.as_deref()),
        },
        Some(Command::Observe { command }) => match command {
            ObserveCommand::List { trace_dir } => observe_cmd::run_list(trace_dir.as_deref()),
            ObserveCommand::Show {
                id_or_path,
                trace_dir,
            } => observe_cmd::run_show(&id_or_path, trace_dir.as_deref()),
            ObserveCommand::Drift {
                baseline,
                candidate,
                json,
            } => observe_cmd::run_drift(&baseline, &candidate, json),
            ObserveCommand::Explain {
                trace_id,
                trace_dir,
            } => cmd_observe_explain(trace_id, trace_dir),
            ObserveCommand::CostOptimise {
                agent,
                trace_dir,
                top_n,
            } => cmd_observe_cost_optimise(agent, trace_dir, top_n),
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
            command,
            explain,
            cdylib,
            key,
            source,
        }) => match command {
            Some(ClaimCommand::Audit { inventory, json }) => {
                claim_cmd::run_claim_audit(&inventory, json)
            }
            None => {
                if let Some(cdylib) = cdylib {
                    claim_cmd::run_claim_explain(&cdylib, explain, key.as_deref(), source.as_deref())
                } else {
                    Err(anyhow::anyhow!(
                        "`corvid claim --explain` requires a cdylib path"
                    ))
                }
            }
        },
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor_v2(),
        Some(Command::Audit { file, json }) => audit_cmd::run_audit(&file, json),
        Some(Command::Deploy { command }) => match command {
            DeployCommand::Package { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("deploy-package"));
                deploy_cmd::run_package(&app, &out).map(|_| 0)
            }
            DeployCommand::Compose { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("compose"));
                deploy_cmd::run_compose(&app, &out).map(|_| 0)
            }
            DeployCommand::Paas { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("paas"));
                deploy_cmd::run_paas(&app, &out).map(|_| 0)
            }
            DeployCommand::K8s { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("k8s"));
                deploy_cmd::run_k8s(&app, &out).map(|_| 0)
            }
            DeployCommand::Systemd { app, out } => {
                let out = out.unwrap_or_else(|| app.join("target").join("systemd"));
                deploy_cmd::run_systemd(&app, &out).map(|_| 0)
            }
        },
        Some(Command::Release {
            channel,
            version,
            out,
        }) => {
            let out = out.unwrap_or_else(|| {
                PathBuf::from("target")
                    .join("release")
                    .join(channel.as_str())
            });
            release_cmd::run_release(&channel, version.as_deref(), &out).map(|_| 0)
        }
        Some(Command::Upgrade { command }) => match command {
            UpgradeCommand::Check { path, json } => upgrade_cmd::run_check(&path, json),
            UpgradeCommand::Apply { path, json } => upgrade_cmd::run_apply(&path, json),
        },
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
                idempotency_key,
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
                idempotency_key,
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
            JobsCommand::Run {
                state,
                workers,
                lease_ttl_ms,
                idle_poll_ms,
                max_runtime_ms,
            } => cmd_jobs_run(
                &state,
                workers,
                lease_ttl_ms,
                idle_poll_ms,
                max_runtime_ms,
            ),
            JobsCommand::Inspect { state, job } => cmd_jobs_inspect(&state, &job),
            JobsCommand::Retry { state, job } => cmd_jobs_retry(&state, &job),
            JobsCommand::Cancel { state, job } => cmd_jobs_cancel(&state, &job),
            JobsCommand::Pause { state, reason } => cmd_jobs_pause(&state, reason.as_deref()),
            JobsCommand::Resume { state } => cmd_jobs_resume(&state),
            JobsCommand::Drain { state, reason } => cmd_jobs_drain(&state, reason.as_deref()),
            JobsCommand::ExportTrace { state, job, out } => {
                cmd_jobs_export_trace(&state, &job, out.as_deref())
            }
            JobsCommand::WaitApproval {
                state,
                worker_id,
                lease_ttl_ms,
                approval_id,
                approval_expires_ms,
                approval_reason,
            } => cmd_jobs_wait_approval(
                &state,
                &worker_id,
                lease_ttl_ms,
                &approval_id,
                approval_expires_ms,
                &approval_reason,
            ),
            JobsCommand::Approvals { state } => cmd_jobs_approvals(&state),
            JobsCommand::Approval { command } => match command {
                JobsApprovalCommand::Decide {
                    state,
                    job,
                    approval_id,
                    decision,
                    actor,
                    reason,
                } => cmd_jobs_approval_decide(&state, &job, &approval_id, decision, &actor, reason),
                JobsApprovalCommand::Audit { state, job } => cmd_jobs_approval_audit(&state, &job),
            },
            JobsCommand::Loop { command } => match command {
                JobsLoopCommand::Limits {
                    state,
                    job,
                    max_steps,
                    max_wall_ms,
                    max_spend_usd,
                    max_tool_calls,
                } => cmd_jobs_loop_limits(
                    &state,
                    &job,
                    max_steps,
                    max_wall_ms,
                    max_spend_usd,
                    max_tool_calls,
                ),
                JobsLoopCommand::Record {
                    state,
                    job,
                    steps,
                    wall_ms,
                    spend_usd,
                    tool_calls,
                    actor,
                } => cmd_jobs_loop_record(
                    &state, &job, steps, wall_ms, spend_usd, tool_calls, &actor,
                ),
                JobsLoopCommand::Usage { state, job } => cmd_jobs_loop_usage(&state, &job),
                JobsLoopCommand::Heartbeat {
                    state,
                    job,
                    actor,
                    message,
                } => cmd_jobs_loop_heartbeat(&state, &job, &actor, message),
                JobsLoopCommand::StallPolicy {
                    state,
                    job,
                    stall_after_ms,
                    action,
                } => cmd_jobs_loop_stall_policy(&state, &job, stall_after_ms, action),
                JobsLoopCommand::CheckStall { state, job, actor } => {
                    cmd_jobs_loop_check_stall(&state, &job, &actor)
                }
            },
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
            JobsCommand::Limit { command } => match command {
                JobsLimitCommand::Set {
                    state,
                    scope,
                    task,
                    max_leased,
                } => cmd_jobs_limit_set(&state, scope, task.as_deref(), max_leased),
                JobsLimitCommand::List { state } => cmd_jobs_limit_list(&state),
            },
            JobsCommand::Checkpoint { command } => match command {
                JobsCheckpointCommand::Add {
                    state,
                    job,
                    kind,
                    label,
                    payload,
                    payload_fingerprint,
                } => cmd_jobs_checkpoint_add(
                    &state,
                    &job,
                    kind,
                    &label,
                    &payload,
                    payload_fingerprint,
                ),
                JobsCheckpointCommand::List { state, job } => {
                    cmd_jobs_checkpoint_list(&state, &job)
                }
                JobsCheckpointCommand::Resume { state, job } => {
                    cmd_jobs_checkpoint_resume(&state, &job)
                }
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
        Some(Command::Connectors { command }) => cmd_connectors(command),
        Some(Command::Auth { command }) => cmd_auth(command),
        Some(Command::Approvals { command }) => cmd_approvals(command),
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


fn cmd_connectors(command: ConnectorsCommand) -> Result<u8> {
    match command {
        ConnectorsCommand::List { json } => {
            let entries = connectors_cmd::run_list()?;
            if json {
                let out = serde_json::to_string_pretty(&entries.iter().map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "provider": e.provider,
                        "modes": e.modes,
                        "scope_count": e.scope_count,
                        "write_scopes": e.write_scopes,
                        "rate_limit": e.rate_limit_summary,
                        "redaction_count": e.redaction_count,
                    })
                }).collect::<Vec<_>>())?;
                println!("{out}");
            } else {
                println!("{:<10} {:<14} {:<22} {:<6} {:<28} {}",
                    "NAME", "PROVIDER", "MODES", "SCOPES", "RATE LIMIT", "WRITE SCOPES");
                for e in &entries {
                    println!("{:<10} {:<14} {:<22} {:<6} {:<28} {}",
                        e.name,
                        e.provider,
                        e.modes.join(","),
                        e.scope_count,
                        if e.rate_limit_summary.len() > 27 {
                            format!("{}…", &e.rate_limit_summary[..26])
                        } else {
                            e.rate_limit_summary.clone()
                        },
                        e.write_scopes.join(","),
                    );
                }
            }
            Ok(0)
        }
        ConnectorsCommand::Check { live, json } => {
            let entries = connectors_cmd::run_check(live)?;
            let any_invalid = entries.iter().any(|e| !e.valid);
            if json {
                let out = serde_json::to_string_pretty(&entries.iter().map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "valid": e.valid,
                        "diagnostics": e.diagnostics,
                    })
                }).collect::<Vec<_>>())?;
                println!("{out}");
            } else {
                println!("{:<12} {:<7} DIAGNOSTICS", "NAME", "VALID");
                for e in &entries {
                    let status = if e.valid { "✓" } else { "✗" };
                    println!("{:<12} {:<7} {}", e.name, status, e.diagnostics.join("; "));
                }
            }
            Ok(if any_invalid { 1 } else { 0 })
        }
        ConnectorsCommand::Run {
            connector,
            operation,
            scope,
            mode,
            payload,
            mock,
            approval_id,
            replay_key,
            tenant_id,
            actor_id,
            token_id,
            now_ms,
        } => {
            let payload_value = match payload {
                Some(path) => {
                    let raw = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading payload from `{}`", path.display()))?;
                    Some(serde_json::from_str(&raw).with_context(|| "payload is not JSON")?)
                }
                None => None,
            };
            let mock_value = match mock {
                Some(path) => {
                    let raw = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading mock from `{}`", path.display()))?;
                    Some(serde_json::from_str(&raw).with_context(|| "mock is not JSON")?)
                }
                None => None,
            };
            let resolved_now_ms = now_ms.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            });
            let output = connectors_cmd::run_run(connectors_cmd::ConnectorRunArgs {
                connector,
                operation,
                scope_id: scope,
                mode,
                payload: payload_value,
                mock_payload: mock_value,
                approval_id,
                replay_key,
                tenant_id,
                actor_id,
                token_id,
                now_ms: resolved_now_ms,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "connector": output.connector,
                "operation": output.operation,
                "mode": output.mode,
                "payload": output.payload,
            }))?);
            Ok(0)
        }
        ConnectorsCommand::Oauth { command } => match command {
            ConnectorsOauthCommand::Init {
                provider,
                client_id,
                redirect_uri,
                scope,
            } => {
                let output = connectors_cmd::run_oauth_init(connectors_cmd::OauthInitArgs {
                    provider,
                    client_id,
                    redirect_uri,
                    scopes: scope,
                })?;
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "provider": output.provider,
                    "state": output.state,
                    "code_verifier": output.code_verifier,
                    "code_challenge": output.code_challenge,
                    "authorization_url": output.authorization_url,
                }))?);
                Ok(0)
            }
            ConnectorsOauthCommand::Rotate {
                provider,
                token_id,
                access_token,
                refresh_token,
                client_id,
                client_secret,
            } => {
                let output = connectors_cmd::run_oauth_rotate(connectors_cmd::OauthRotateArgs {
                    provider,
                    token_id,
                    access_token,
                    refresh_token,
                    client_id,
                    client_secret,
                })?;
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "provider": output.provider,
                    "access_token": output.access_token,
                    "refresh_token": output.refresh_token,
                    "expires_at_ms": output.expires_at_ms,
                }))?);
                Ok(0)
            }
        },
        ConnectorsCommand::VerifyWebhook {
            signature,
            secret_env,
            body_file,
            provider,
            headers,
        } => {
            let parsed_headers = headers
                .iter()
                .map(|h| {
                    let mut parts = h.splitn(2, '=');
                    let name = parts.next().unwrap_or_default().to_string();
                    let value = parts.next().unwrap_or_default().to_string();
                    (name, value)
                })
                .collect::<Vec<_>>();
            let output = connectors_cmd::run_verify_webhook(connectors_cmd::WebhookVerifyArgs {
                signature,
                secret_env,
                body_file,
                provider,
                headers: parsed_headers,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "valid": output.valid,
                "algorithm": output.algorithm,
                "outcome": output.outcome,
            }))?);
            Ok(if output.valid { 0 } else { 1 })
        }
    }
}

fn cmd_auth(command: AuthCommand) -> Result<u8> {
    match command {
        AuthCommand::Migrate {
            auth_state,
            approvals_state,
        } => {
            let out = auth_cmd::run_auth_migrate(auth_cmd::AuthMigrateArgs {
                auth_state,
                approvals_state,
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "auth_state": out.auth_state,
                    "approvals_state": out.approvals_state,
                    "auth_initialised": out.auth_initialised,
                    "approvals_initialised": out.approvals_initialised,
                }))?
            );
            Ok(0)
        }
        AuthCommand::Keys { command } => match command {
            AuthKeysCommand::Issue {
                auth_state,
                key_id,
                service_actor,
                tenant,
                raw_key,
                scope_fingerprint,
                display_name,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_issue(auth_cmd::AuthKeyIssueArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    raw_key,
                    scope_fingerprint,
                    display_name,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "service_actor_id": out.service_actor_id,
                        "tenant_id": out.tenant_id,
                        "key_hash_prefix": out.key_hash_prefix,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                        "raw_key": out.raw_key,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Revoke {
                auth_state,
                key_id,
            } => {
                let out = auth_cmd::run_auth_key_revoke(auth_cmd::AuthKeyRevokeArgs {
                    auth_state,
                    key_id,
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key_id": out.key_id,
                        "revoked_at_ms": out.revoked_at_ms,
                    }))?
                );
                Ok(0)
            }
            AuthKeysCommand::Rotate {
                auth_state,
                key_id,
                service_actor,
                tenant,
                new_key_id,
                new_raw_key,
                expires_at_ms,
            } => {
                let out = auth_cmd::run_auth_key_rotate(auth_cmd::AuthKeyRotateArgs {
                    auth_state,
                    key_id,
                    service_actor_id: service_actor,
                    tenant_id: tenant,
                    new_key_id,
                    new_raw_key,
                    expires_at_ms: expires_at_ms.unwrap_or(u64::MAX),
                })?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "revoked_key_id": out.revoked_key_id,
                        "new_key_id": out.new_key_id,
                        "raw_key": out.raw_key,
                        "scope_fingerprint": out.scope_fingerprint,
                        "expires_at_ms": out.expires_at_ms,
                    }))?
                );
                Ok(0)
            }
        },
    }
}

fn cmd_approvals(command: ApprovalsCommand) -> Result<u8> {
    use approvals_cmd::*;
    match command {
        ApprovalsCommand::Queue {
            approvals_state,
            tenant,
            status,
        } => {
            let out = run_approvals_queue(ApprovalsQueueArgs {
                approvals_state,
                tenant_id: tenant,
                status,
            })?;
            println!("{}", serde_json::to_string_pretty(&serde_json::to_value(approvals_queue_summary(&out))?)?);
            Ok(0)
        }
        ApprovalsCommand::Inspect {
            approvals_state,
            tenant,
            approval_id,
        } => {
            let out = run_approvals_inspect(ApprovalsInspectArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
            })?;
            println!("{}", serde_json::to_string_pretty(&approvals_inspect_summary(&out))?);
            Ok(0)
        }
        ApprovalsCommand::Approve {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_approve(ApprovalsTransitionArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Deny {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_deny(ApprovalsTransitionArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Expire {
            approvals_state,
            tenant,
            actor,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_expire(ApprovalsExpireArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Comment {
            approvals_state,
            tenant,
            actor,
            comment,
            approval_id,
        } => {
            let event = run_approvals_comment(ApprovalsCommentArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                comment,
            })?;
            println!("{}", serde_json::to_string_pretty(&audit_event_value(&event))?);
            Ok(0)
        }
        ApprovalsCommand::Delegate {
            approvals_state,
            tenant,
            actor,
            role,
            delegate_to,
            reason,
            approval_id,
        } => {
            let summary = run_approvals_delegate(ApprovalsDelegateArgs {
                approvals_state,
                tenant_id: tenant,
                approval_id,
                actor_id: actor,
                role,
                delegate_to,
                reason,
            })?;
            println!("{}", serde_json::to_string_pretty(&approval_summary_value(&summary))?);
            Ok(0)
        }
        ApprovalsCommand::Batch {
            approvals_state,
            tenant,
            actor,
            role,
            reason,
            ids,
        } => {
            let out = run_approvals_batch(ApprovalsBatchArgs {
                approvals_state,
                tenant_id: tenant,
                actor_id: actor,
                role,
                approval_ids: ids,
                reason,
            })?;
            let approved = out
                .approved
                .iter()
                .map(approval_summary_value)
                .collect::<Vec<_>>();
            let failed = out
                .failed
                .iter()
                .map(|f| serde_json::json!({"approval_id": f.approval_id, "reason": f.reason}))
                .collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "approved": approved,
                    "failed": failed,
                }))?
            );
            Ok(if out.failed.is_empty() { 0 } else { 1 })
        }
        ApprovalsCommand::Export {
            approvals_state,
            tenant,
            since_ms,
            out,
        } => {
            let result = run_approvals_export(ApprovalsExportArgs {
                approvals_state,
                tenant_id: tenant,
                since_ms,
            })?;
            let entries: Vec<serde_json::Value> = result
                .approvals
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "approval": approval_summary_value(&e.approval),
                        "audit_events": e.audit_events.iter().map(audit_event_value).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let payload = serde_json::json!({
                "tenant_id": result.tenant_id,
                "approvals": entries,
            });
            let serialized = serde_json::to_string_pretty(&payload)?;
            if let Some(path) = out {
                std::fs::write(&path, &serialized)
                    .with_context(|| format!("writing export to `{}`", path.display()))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "wrote_to": path,
                        "approval_count": result.approvals.len(),
                    }))?
                );
            } else {
                println!("{serialized}");
            }
            Ok(0)
        }
    }
}


