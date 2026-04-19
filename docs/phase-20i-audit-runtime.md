# Phase 20i - Runtime-lane audit record

Rubric sweep of every Rust source file in the runtime-lane file scope:
`corvid-vm`, `corvid-runtime`, `corvid-codegen-cl`,
`corvid-differential-verify`. Per `CLAUDE.md`, a file fails when it
(1) mixes unrelated top-level concepts, (2) has 5+ public items across
unrelated domains, or (3) has 3+ internal sections sharing no state.
Line count is a heuristic, not the rule.

## Files repaired or split before this audit

| File | Slice | Result |
|---|---|---|
| `corvid-runtime/tests/gc_verify.rs` | `20i-fix` | formatting restored after `f00ffb8` |
| `corvid-runtime/tests/cycle_collector.rs` | `20i-fix` | formatting restored after `f00ffb8` |
| `corvid-vm/src/lib.rs` | `20i-7` (4 commits) | test suite extracted; crate surface reduced to 44 lines |
| `corvid-vm/src/interp.rs` | `20i-6` (4 commits) | split into shell + `effect_compose` / `expr` / `prompt` / `stmt` |
| `corvid-codegen-cl/tests/parity.rs` | `20i-8` (12 commits) | split into 12 focused parity families |
| `corvid-codegen-cl/src/lowering.rs` | `20i-5` (10 commits) | 6,405 -> 282 lines; split across lowering submodules |

## Files audited - rubric PASS (no split needed)

Every file below was inspected against the rubric and found to hold one
coherent responsibility, or at most two tightly coupled
responsibilities that stay within the 1-2 rule.

### `corvid-vm`

- `corvid-vm/src/conv.rs` (352) - clean; JSON <-> `Value` conversion only.
- `corvid-vm/src/cycle_collector.rs` (195) - clean; cycle collector only.
- `corvid-vm/src/env.rs` (28) - clean; interpreter environment bindings only.
- `corvid-vm/src/errors.rs` (102) - clean; interpreter diagnostics only.
- `corvid-vm/src/interp.rs` (779) - clean; interpreter shell plus public run-entry APIs. Two tightly coupled responsibilities within the 1-2 rule.
- `corvid-vm/src/interp/effect_compose.rs` (95) - clean; effect-composition helpers only.
- `corvid-vm/src/interp/expr.rs` (167) - clean; expression primitive evaluation only.
- `corvid-vm/src/interp/prompt.rs` (912) - clean; prompt execution strategies and prompt-side trace emission. Large, but one concern.
- `corvid-vm/src/interp/stmt.rs` (396) - clean; statement/block evaluation only.
- `corvid-vm/src/lib.rs` (44) - clean; crate surface and re-exports only.
- `corvid-vm/src/repl_display.rs` (155) - clean; value rendering for REPL/UI only.
- `corvid-vm/src/schema.rs` (223) - clean; schema derivation for runtime values only.
- `corvid-vm/src/step.rs` (371) - clean; step/replay control surface only.
- `corvid-vm/src/tests/core.rs` (291) - clean; core interpreter semantics tests only.
- `corvid-vm/src/tests/dispatch.rs` (1284) - clean; runtime/dispatch integration tests only. Large integration test module, one concern.
- `corvid-vm/src/tests/mod.rs` (44) - clean; shared VM test harness only.
- `corvid-vm/src/tests/stream.rs` (302) - clean; stream semantics tests only.
- `corvid-vm/src/value.rs` (971) - clean; runtime value/heap ownership model only. Large, but one concern.
- `corvid-vm/tests/cycle_collector.rs` (99) - clean; external cycle collector integration test only.
- `corvid-vm/tests/parity_native_vs_interp.rs` (145) - clean; native/interpreter parity integration test only.

### `corvid-runtime`

- `corvid-runtime/benches/memory_runtime.rs` (459) - clean; runtime memory benchmark harness only.
- `corvid-runtime/build.rs` (82) - clean; build-script glue only.
- `corvid-runtime/src/abi.rs` (290) - clean; ABI conversion/types only.
- `corvid-runtime/src/adversarial.rs` (59) - clean; adversarial helper functions only.
- `corvid-runtime/src/approvals.rs` (216) - clean; approval request/decision infrastructure only.
- `corvid-runtime/src/ensemble.rs` (61) - clean; ensemble voting helpers only.
- `corvid-runtime/src/env.rs` (81) - clean; dotenv discovery/loading only.
- `corvid-runtime/src/errors.rs` (107) - clean; runtime errors only.
- `corvid-runtime/src/ffi_bridge.rs` (1079) - clean; C ABI bridge only. Large because it owns the full FFI surface, but the concern is singular.
- `corvid-runtime/src/lib.rs` (78) - clean; crate surface, modules, and re-exports only.
- `corvid-runtime/src/llm/anthropic.rs` (262) - clean; Anthropic adapter only.
- `corvid-runtime/src/llm/gemini.rs` (212) - clean; Gemini adapter only.
- `corvid-runtime/src/llm/mock.rs` (338) - clean; mock adapter + benchmark hooks only.
- `corvid-runtime/src/llm/mod.rs` (194) - clean; LLM abstraction surface and registry only.
- `corvid-runtime/src/llm/ollama.rs` (215) - clean; Ollama adapter only.
- `corvid-runtime/src/llm/openai.rs` (251) - clean; OpenAI adapter only.
- `corvid-runtime/src/llm/openai_compat.rs` (223) - clean; OpenAI-compatible adapter only.
- `corvid-runtime/src/models.rs` (322) - clean; model catalog + selection only.
- `corvid-runtime/src/redact.rs` (141) - clean; redaction set only.
- `corvid-runtime/src/runtime.rs` (445) - clean; `Runtime` / `RuntimeBuilder` dispatch glue only. Two tightly coupled responsibilities within the 1-2 rule.
- `corvid-runtime/src/tools.rs` (103) - clean; tool registry only.
- `corvid-runtime/src/tracing.rs` (333) - clean; trace writing/rotation only.
- `corvid-runtime/tests/anthropic_integration.rs` (110) - clean; Anthropic integration tests only.
- `corvid-runtime/tests/cycle_collector.rs` (259) - clean; runtime cycle-collector integration tests only.
- `corvid-runtime/tests/gc_verify.rs` (259) - clean; GC verification integration tests only.
- `corvid-runtime/tests/openai_integration.rs` (110) - clean; OpenAI integration tests only.
- `corvid-runtime/tests/typeinfo_tracer.rs` (256) - clean; typeinfo/tracing integration tests only.
- `corvid-runtime/tests/weak.rs` (151) - clean; weak-reference integration tests only.

### `corvid-codegen-cl`

- `corvid-codegen-cl/benches/native_foundation_benchmarks.rs` (185) - clean; native benchmark harness only.
- `corvid-codegen-cl/build.rs` (36) - clean; build-script glue only.
- `corvid-codegen-cl/src/dataflow.rs` (1093) - clean; CFG/liveness/ownership analysis only. Large, but one analysis engine.
- `corvid-codegen-cl/src/dup_drop.rs` (792) - clean; Dup/Drop IR insertion pass only.
- `corvid-codegen-cl/src/errors.rs` (66) - clean; codegen errors only.
- `corvid-codegen-cl/src/latency_rc.rs` (259) - clean; prompt pin analysis only.
- `corvid-codegen-cl/src/lib.rs` (156) - clean; crate surface plus compile/build entry points only.
- `corvid-codegen-cl/src/link.rs` (188) - clean; native linker invocation only.
- `corvid-codegen-cl/src/lowering.rs` (282) - clean; lowering shell and shared orchestration only.
- `corvid-codegen-cl/src/lowering/agent.rs` (234) - clean; agent lowering only.
- `corvid-codegen-cl/src/lowering/entry.rs` (371) - clean; native CLI entry/runtime-reachability lowering only.
- `corvid-codegen-cl/src/lowering/expr.rs` (2025) - clean; expression lowering only. Large due to expression shape coverage, but still one concern.
- `corvid-codegen-cl/src/lowering/prompt.rs` (309) - clean; prompt-call lowering only.
- `corvid-codegen-cl/src/lowering/runtime.rs` (1980) - clean; runtime interop/typeinfo/refcount lowering support only. Large, but one concern.
- `corvid-codegen-cl/src/lowering/stmt.rs` (721) - clean; statement/block lowering only.
- `corvid-codegen-cl/src/module.rs` (65) - clean; object-module factory only.
- `corvid-codegen-cl/src/ownership.rs` (473) - clean; borrow inference + per-agent ownership summaries. Two tightly coupled responsibilities within the 1-2 rule.
- `corvid-codegen-cl/src/pair_elim.rs` (403) - clean; retain/release pair elimination only.
- `corvid-codegen-cl/src/scope_reduce.rs` (315) - clean; scope-reduction pass only.
- `corvid-codegen-cl/tests/baseline_rc_counts.rs` (292) - clean; RC baseline integration test only.
- `corvid-codegen-cl/tests/drop_specialization.rs` (165) - clean; drop-specialization integration test only.
- `corvid-codegen-cl/tests/dup_drop_pipeline.rs` (186) - clean; Dup/Drop pipeline integration test only.
- `corvid-codegen-cl/tests/ffi_bridge_smoke.rs` (217) - clean; FFI bridge smoke test only.
- `corvid-codegen-cl/tests/pair_elim.rs` (209) - clean; pair-elimination integration test only.
- `corvid-codegen-cl/tests/parity.rs` (380) - clean; parity test harness + module wiring only.
- `corvid-codegen-cl/tests/parity/bool.rs` (239) - clean; bool parity fixtures only.
- `corvid-codegen-cl/tests/parity/entry.rs` (235) - clean; CLI entry parity fixtures only.
- `corvid-codegen-cl/tests/parity/float.rs` (77) - clean; float parity fixtures only.
- `corvid-codegen-cl/tests/parity/int.rs` (88) - clean; int parity fixtures only.
- `corvid-codegen-cl/tests/parity/list.rs` (111) - clean; list parity fixtures only.
- `corvid-codegen-cl/tests/parity/method.rs` (43) - clean; method parity fixtures only.
- `corvid-codegen-cl/tests/parity/prompt.rs` (113) - clean; prompt parity fixtures only.
- `corvid-codegen-cl/tests/parity/string.rs` (75) - clean; string parity fixtures only.
- `corvid-codegen-cl/tests/parity/structs.rs` (132) - clean; struct parity fixtures only.
- `corvid-codegen-cl/tests/parity/sumtypes.rs` (314) - clean; `Option`/`Result` parity fixtures only.
- `corvid-codegen-cl/tests/parity/tool.rs` (221) - clean; tool parity fixtures only.
- `corvid-codegen-cl/tests/parity/weak.rs` (8) - clean; weak parity fixtures only.
- `corvid-codegen-cl/tests/scope_reduce.rs` (138) - clean; scope-reduction integration tests only.
- `corvid-codegen-cl/tests/stack_maps.rs` (273) - clean; stack-map integration tests only.
- `corvid-codegen-cl/tests/verifier_audit.rs` (232) - clean; verifier-audit integration tests only.

### `corvid-differential-verify`

- `corvid-differential-verify/src/fuzz.rs` (641) - clean; preserved-semantics fuzz harness only.
- `corvid-differential-verify/src/lib.rs` (952) - clean; cross-tier differential verification harness only. Large because it owns reports, rendering, and shrinker for the same harness.
- `corvid-differential-verify/src/rewrite.rs` (1486) - clean; AST rewrite engine only. Large because it carries all seven rewrite laws, but still one concern.
- `corvid-differential-verify/tests/rewrite_ast.rs` (82) - clean; rewrite AST round-trip tests only.

## Exceptions

- Large integration-test modules remain large by design:
  `corvid-vm/src/tests/dispatch.rs`,
  `corvid-codegen-cl/tests/parity.rs`,
  and the already-split `parity/*` family. Each still targets one
  crate surface, which is one responsibility under the rubric.
- Large crate-surface files that still pass:
  `corvid-vm/src/interp.rs`,
  `corvid-runtime/src/runtime.rs`,
  and `corvid-codegen-cl/src/lib.rs`. Each combines only the public
  entry surface with directly coupled orchestration, staying within the
  1-2 responsibility rule.

## Status

Runtime lane sweep complete. No further split candidates. Phase 20i is
ready for the joint closeout commit once `ROADMAP.md` is updated.
