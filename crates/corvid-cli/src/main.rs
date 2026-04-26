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

mod abi_cmd;
mod approver_cmd;
mod bind_cmd;
mod bundle_cmd;
mod capsule_cmd;
mod cost_frontier;
mod eval_cmd;
mod receipt_cache;
mod receipt_cmd;
mod replay;
mod routing_report;
mod test_from_traces;
mod trace_cmd;
mod trace_dag;
mod trace_diff;
mod tour;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use corvid_differential_verify::{
    render_corpus_grid, render_report, shrink_program, verify_corpus,
};
use cost_frontier::{
    build_frontier, render_frontier as render_cost_frontier, CostFrontierOptions,
};
use routing_report::{build_report, render_report as render_routing_report, RoutingReportOptions};

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_spec_site, build_target_to_disk, build_to_disk, build_wasm_to_disk,
    compile, compile_with_config, diff_snapshots, file_github_issues_for_escapes,
    inspect_import_semantics, load_corvid_config_for, load_corvid_config_with_path_for,
    load_dotenv_walking, render_all_pretty, render_dimension_verification_report,
    render_adversarial_report, render_effect_diff, render_import_semantic_summaries,
    render_law_check_report, render_spec_report, render_spec_site_report, render_test_report,
    run_adversarial_suite, run_dimension_verification, run_law_checks, run_native,
    run_tests_at_path_with_options, run_with_target, scaffold_new, snapshot_revision,
    test_options, verify_spec_examples, BuildTarget, RunTarget, VerdictKind, DEFAULT_SAMPLES,
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
        /// Output target. `python` (default), `native`, `wasm`, `cdylib`, or `staticlib`.
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
            conflicts_with = "flake_detect",
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
    /// Start the interactive Corvid REPL.
    Repl,
    /// Check the local environment for required tools.
    Doctor,
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
    Dump { library: PathBuf },
    Hash { source: PathBuf },
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
        }) => cmd_build(
            &file,
            &target,
            with_tools_lib.as_deref(),
            header,
            abi_descriptor,
            all_artifacts,
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
                test_from_traces::run_test_from_traces(
                    test_from_traces::TestFromTracesArgs {
                        trace_dir: &dir,
                        source: from_traces_source.as_deref(),
                        replay_model: replay_model.as_deref(),
                        only_dangerous,
                        only_prompt: only_prompt.as_deref(),
                        only_tool: only_tool.as_deref(),
                        since: since.as_deref(),
                        promote,
                        flake_detect,
                    },
                )
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
        Some(Command::Verify { corpus, shrink, json }) => {
            cmd_verify(corpus.as_deref(), shrink.as_deref(), json)
        }
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
            ReceiptCommand::Verify { envelope, key } => {
                receipt_cmd::run_verify(&envelope, &key)
            }
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
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor(),
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
        ),
        "staticlib" => {
            if abi_descriptor {
                anyhow::bail!("`--abi-descriptor` and `--all-artifacts` are only valid for `cdylib`");
            }
            cmd_build_library(
                file,
                BuildTarget::Staticlib,
                &extra_libs_owned,
                header,
                false,
            )
        }
        other => {
            anyhow::bail!(
                "unknown target `{other}`; valid: `python` (default), `native`, `wasm`, `cdylib`, `staticlib`"
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
) -> Result<u8> {
    let out =
        build_target_to_disk(file, target, header, abi_descriptor, tools_libs).with_context(|| {
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
        (None, None) => anyhow::bail!(
            "use `corvid verify --corpus <dir>` or `corvid verify --shrink <file>`"
        ),
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
        .filter(|r| matches!(
            r.verdict,
            corvid_driver::LawVerdict::CounterExample { .. }
        ))
        .count();
    let proof_failures = report.proofs.iter().filter(|p| p.failed()).count();
    Ok(if law_failures == 0 && proof_failures == 0 { 0 } else { 1 })
}

fn cmd_test_spec() -> Result<u8> {
    let spec_dir = PathBuf::from("docs/effects-spec");
    if !spec_dir.exists() {
        anyhow::bail!(
            "`docs/effects-spec/` not found; run `corvid test spec` from the repository root"
        );
    }
    println!("corvid test spec — verify every fenced corvid block in {}\n", spec_dir.display());
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
    print!("{}", corvid_differential_verify::fuzz::render_coverage_matrix(&matrix));
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
    let any_change =
        !diff.added.is_empty() || !diff.removed.is_empty() || !diff.changed.is_empty();
    Ok(if any_change { 1 } else { 0 })
}

// ------------------------------------------------------------
// Dimension registry client
// ------------------------------------------------------------

fn cmd_add_dimension(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir =
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add-dimension {spec}\n");
    let outcome = corvid_driver::install_dimension_with_registry(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::AddDimensionOutcome::Added { name, target } => {
            println!(
                "installed `{name}` into {}",
                target.display()
            );
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
    let project_dir =
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
                if manifest_updated { "updated" } else { "unchanged" },
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
        print!("{}", corvid_driver::render_package_metadata_markdown(&metadata));
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
            println!(
                "  ✗ CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set"
            );
        }
        let openai_prefixes = ["gpt-", "o1-", "o3-", "o4-"];
        if openai_prefixes.iter().any(|p| m.starts_with(p))
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            println!(
                "  ✗ CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set"
            );
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
