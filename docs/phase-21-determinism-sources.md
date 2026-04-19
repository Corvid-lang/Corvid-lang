# Phase 21 — Determinism source audit

Enumerates every nondeterministic input the runtime consumes
today, and names the record-hook + trace-event variant that
Phase 21's replay machinery must attach to each one.

This document is the contract between my compile-side lane
(slices `21-A-determinism-hooks` and `21-inv-A` / `21-inv-F`)
and Dev B's runtime lane (slice `21-B-rec-interp` onward).
If a new nondeterministic source appears in the code, it must
land here before a recording tier is expected to capture it.

## 1. Source-level builtins (user-callable from Corvid source)

**None as of Phase 21 v1.** Corvid source today exposes no
`now()`, `random()`, `uuid()`, or `env()` builtins. All
nondeterminism is injected implicitly by the runtime or by
user-declared tools.

The `corvid_types::determinism` module carries an empty
catalog + a `classify_call_target` helper. Any future source-
level builtin must register here first; the `@replayable`
checker reads the catalog to decide whether a call in a
`@replayable` body introduces an unrecoverable source.

## 2. Runtime-injected nondeterminism (interceptable by recording)

### 2.1 Wall-clock reads — `corvid_runtime::tracing::now_ms`

**Call site:** `crates/corvid-runtime/src/tracing.rs:270-275`.
Reads `SystemTime::now()`, normalizes to epoch-ms, returns
`u64`.

**Who calls it today:**
- Every `TraceEvent` constructor (timestamps every emitted event).
- `fresh_run_id()` — derives the run id from the wall clock.
- `Runtime::builder().build()` — defaults `rollout_seed` to
  `now_ms()` if the caller hasn't specified one.

**Replay treatment:** Trace timestamps are metadata, not
program behavior. Recording-tier interception should emit a
`TraceEvent::ClockRead { source: "wall", value: <ms> }` **only
when the read is part of program behavior**, not when it is
populating the timestamp on another event (which would be
infinite-recursive). Dev B's `21-B-rec-interp` is responsible
for drawing that distinction.

**Run-id reconstruction on replay:** the first `SchemaHeader` +
`RunStarted` pair in the trace carries the original `run_id`
string. Replay reuses it directly; no new wall-clock read
needed.

**Default-rollout-seed reconstruction on replay:** replay
reads the first `SeedRead` with `purpose = "rollout_default_seed"`
(or similar agreed convention) and injects it as the
rollout state before any `next_rollout_sample` call.

### 2.2 Pseudo-random draws — `Runtime::next_rollout_sample`

**Call site:** `crates/corvid-runtime/src/runtime.rs:99-116`.
Implements an LCG on the atomic `rollout_state: Arc<AtomicU64>`,
CAS-updated on every draw. Returns `f64` in `[0.0, 1.0)` used
by `choose_rollout_variant` for A/B dispatch.

**Who calls it today:** `Runtime::choose_rollout_variant`,
invoked by the interpreter's prompt-dispatch path when a
prompt declares a `rollout N% variant, else baseline` clause
(Phase 20h slice I-rt).

**Seeded-PRNG confirmation:** the LCG is entirely
deterministic given a fixed initial `rollout_seed`. Running
`Runtime::builder().rollout_seed(12345).build()` twice produces
byte-identical draw sequences — confirmed by
`rollout_seed_produces_stable_sequence_across_restarts` in
`crates/corvid-runtime/src/runtime.rs`.

**Replay treatment:** Dev B's `21-B-rec-interp` must intercept
every `next_rollout_sample` call and emit
`TraceEvent::SeedRead { purpose: "rollout_cohort", value: <raw
u64> }`. The raw `u64` (pre-mantissa-normalization) must be
captured so replay can reproduce the exact `f64` via the same
`mantissa >> 11` transformation.

On replay, the runtime's `next_rollout_sample` is replaced by
a function that pops the next `SeedRead` event from the trace,
validates `purpose == "rollout_cohort"`, and returns the
captured `value` transformed into `f64` the same way.

### 2.3 Model-dispatch decisions (already recorded in Phase 20h)

The following events are **already emitted** by the runtime and
already deterministic inputs to replay — no new hook needed for
Phase 21 v1:

- `ModelSelected` (capability / route / progressive / rollout
  dispatch outcomes)
- `ProgressiveEscalation`, `ProgressiveExhausted`
- `AbVariantChosen` (A/B rollout outcome)
- `EnsembleVote`, `AdversarialPipelineCompleted`,
  `AdversarialContradiction`

Replay treats each of these as the canonical dispatch decision;
if the replayed execution's dispatch logic would produce a
different outcome, it's a divergence, not something to
overrule.

## 3. Nondeterministic inputs from user tools

Tools declared by the user (e.g. `tool get_order(id: String) ->
Order`) are opaque to the runtime. They may read files, make
HTTP calls, or call system clocks internally. The runtime's
tool-dispatch layer already records `ToolCall` + `ToolResult`
events, so any nondeterminism inside a tool is captured by its
result being in the trace.

**Implication for `@replayable`:** the checker treats every
tool call as recorded (safe for replay). A tool is only
"non-replayable" if it's invoked without going through the
runtime's dispatch layer (which shouldn't happen in v1 —
there's no out-of-band tool call path).

## 4. Approve decisions

Approve decisions (interactive / programmatic approval prompts)
are already recorded via `ApprovalRequest` + `ApprovalResponse`
events. Replay substitutes the recorded `approved` bool for the
approver's live decision.

No new work here for Phase 21 v1 — the hook lives in
`corvid_runtime::approvals` and Dev B's `21-C-replay-interp`
simply swaps the live approver for a trace-reading one.

## 5. Scope boundary (what `@replayable` is NOT required to
capture)

Explicitly out of scope for Phase 21 v1:

- **Operating-system non-determinism** (thread scheduling order,
  file-system case sensitivity, socket selection). Corvid
  programs are single-threaded from the user's point of view
  (tokio runs the scheduler, but agent bodies see a linear
  event stream). Cross-tier reproducibility already assumes
  identical host OS state; replay does not attempt to
  synthesize it.
- **Floating-point NaN bit-patterns** (ARM / x86 differences).
  Corvid uses `f64` for cost / confidence / latency; the NaN
  payload difference is below the level of observability for
  every `@replayable` agent we care about.
- **Hardware-RNG instructions** (RDRAND, etc.). The runtime
  does not use them; if a user tool does, it falls under §3
  and is captured via `ToolResult`.

## 6. Confirmation checklist

- [x] Wall-clock reads catalogued (§2.1).
- [x] PRNG wiring confirmed — single seeded LCG, deterministic
  given a seed (§2.2).
- [x] Dispatch decisions already emitting trace events (§2.3).
- [x] Tool / approve paths already record via Phase 20h wiring
  (§§3, 4).
- [x] Source-level builtin catalog module in place
  (`corvid-types::determinism`); empty as of v1, ready to
  register new entries.
- [ ] Recording hooks landed in the interpreter tier — Dev B's
  `21-B-rec-interp`.
- [ ] Recording hooks landed in the native tier — Dev B's
  `21-B-rec-native`.
- [ ] Replay adapters substitute recorded values for live calls —
  Dev B's `21-C-replay-*` slices.
- [ ] `@replayable` checker enforces the catalog — my `21-inv-A`.

## 7. Adding a new nondeterministic source in a future slice

1. Add the `NondeterminismSource` variant (or reuse an
   existing one) in `corvid-types::determinism`.
2. Register the source-level builtin name in
   `KNOWN_NONDETERMINISTIC_BUILTINS` if the source is directly
   callable from Corvid code.
3. Extend `TraceEvent` (in `corvid-trace-schema`) with a new
   variant if no existing variant captures the value shape.
   Bump `SCHEMA_VERSION` if the addition changes an existing
   variant's shape.
4. Add a recording hook in the runtime (interpreter + native).
5. Add replay-substitution logic (same two tiers).
6. Update §1 / §2 of this document.
7. If `@replayable` should flag calls to the new source, the
   catalog entry is the whole enforcement point — no further
   checker changes needed.
