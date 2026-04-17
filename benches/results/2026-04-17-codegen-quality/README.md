# Codegen Quality / Hot-Loop Assessment

- Host CPU: `Intel(R) Core(TM) Ultra 7 155U`
- Host OS: `Microsoft Windows 11 Business 10.0.26200 (build 26200)`
- Disassembler: `dumpbin.exe` from Visual Studio 2022 Build Tools

## Scope

This archive checks whether machine-code quality is the next obvious lever for
the **shipped workflow benchmarks**.

It does **not** try to prove that Corvid's native code is globally optimal.
It answers a narrower question:

- are the current `tool_loop`, `approval_workflow`, `retry_workflow`, and
  `replay_trace` benchmark wins/limits plausibly bottlenecked on poor hot-loop
  code generation?

## Inputs Examined

Representative cached benchmark binaries:

- `tool_loop`: `benches/corvid/workloads/target/cache/native/9731f01716a0c651.exe`
- `approval_workflow`: `benches/corvid/workloads/target/cache/native/a8203500a077da72.exe`

Representative workload sources:

- [tool_loop.cor](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/benches/corvid/workloads/tool_loop.cor)
- [approval_workflow.cor](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/benches/corvid/workloads/approval_workflow.cor)

Codegen/runtime configuration reviewed:

- [module.rs](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/crates/corvid-codegen-cl/src/module.rs)
- [Cargo.toml](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/Cargo.toml)

## Important Constraint

The current worktree has unrelated `corvid-resolve` compile errors, so this
slice analyzes the **current cached shipped benchmark binaries** rather than
forcing a fresh full rebuild through the normal runner path.

That is acceptable for this slice because the goal is structural analysis of
the shipped workload shape and the generated machine-code style, not a
numerically fresh benchmark session.

## Evidence

### 1. The native path is already using optimized build settings

From [module.rs](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/crates/corvid-codegen-cl/src/module.rs):

- Cranelift `opt_level = "speed"`
- frame pointers preserved for GC stack walking
- verifier enabled

From the workspace [Cargo.toml](C:/Users/SBW/OneDrive%20-%20Axon%20Group/Documents/GitHub/corvid/Cargo.toml):

- release `opt-level = 3`
- `lto = "thin"`
- `codegen-units = 1`

So the current shipped benchmark binaries are not obviously in a low-quality
"debug-like" codegen configuration.

### 2. The shipped workflow workloads are straight-line orchestration programs

`tool_loop.cor` and `approval_workflow.cor` are short sequences of:

- prompt call
- tool call
- prompt/tool call
- return

They do **not** contain compute-heavy inner loops, numeric kernels, or data
parallel transforms where machine-code scheduling would dominate.

That matters because "hot-loop disassembly" only becomes the right next lever
when the workload actually contains a hot loop worth tuning.

### 3. Representative binaries are large call-heavy bridge executables, not tiny compute kernels

From `tool_loop_headers.txt`:

- PE32+ executable
- `.text` size about `0x284E00` bytes
- entry point `0x140273920`

The `tool_loop_disasm_excerpt.txt` and `approval_disasm_excerpt.txt` excerpts
show:

- dense call chains in the generated entry / bridge region
- short straight-line setup code leading into helper calls
- no obvious long-running arithmetic loop in the benchmark-shaped workload path

This matches the source shape: the shipped fixtures spend their time crossing
runtime / prompt / tool boundaries, not running compute loops inside generated
native code.

## Recommendation

For the **shipped workflow benchmarks**, codegen-quality / hot-loop work is
**not** the next benchmark lever.

Why:

- the workloads are orchestration-shaped, not compute-loop-shaped
- the binaries are already built with optimized settings
- the disassembly evidence is consistent with bridge-heavy execution rather than
  a machine-code hot loop bottleneck

So the honest conclusion is:

- if the goal is the current shipped workflow benchmark sheet, machine-code
  tuning can defer
- if the goal is future compute-heavy Corvid programs, revisit this slice with
  a benchmark that actually contains numeric or collection-heavy loops

## Files

- `tool_loop_headers.txt`
- `tool_loop_disasm_excerpt.txt`
- `approval_headers.txt`
- `approval_disasm_excerpt.txt`
