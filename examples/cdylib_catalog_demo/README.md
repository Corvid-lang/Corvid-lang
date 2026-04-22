# cdylib catalog demo

This example shows the `22-C` capability-store surface and the `22-E`
approval bridge:

- embedded `CORVID_ABI_DESCRIPTOR`
- `corvid_abi_verify`
- `corvid_list_agents`
- `corvid_pre_flight`
- `corvid_call_agent`
- `corvid_register_approver_from_source`
- trace-backed `approval_decision` evidence

The exported Corvid surface is scalar-only on purpose so the current
Phase `22-C` catalog dispatcher can call it generically across the C ABI.
The demo still includes one real dangerous path, `issue_tag(tag: String)`,
so the approval bridge is exercised end to end.

## Build

From the repository root:

```powershell
cargo build -p corvid-test-tools --release

$tools = if ($IsWindows) {
  "target/release/corvid_test_tools.lib"
} else {
  "target/release/libcorvid_test_tools.a"
}

cargo run -q -p corvid-cli -- build `
  examples/cdylib_catalog_demo/src/classify.cor `
  --target=cdylib `
  --with-tools-lib $tools `
  --all-artifacts

$hash = cargo run -q -p corvid-cli -- abi hash examples/cdylib_catalog_demo/src/classify.cor
```

The build writes:

- `examples/cdylib_catalog_demo/target/release/lib_classify.h`
- `examples/cdylib_catalog_demo/target/release/<platform library>`
- `examples/cdylib_catalog_demo/target/release/classify.corvid-abi.json`

## The approval bridge in five minutes

The library exports two user capabilities:

- `classify(text: String) -> String`
- `issue_tag(tag: String) -> String`

`issue_tag` routes through a dangerous tool call, so the catalog marks it as
requiring approval. The host never gets to bypass that by calling only the
happy-path symbol:

- with an accepting Corvid approver, dispatch succeeds and the trace records
  `approval_decision` with `accepted=true`
- with a rejecting Corvid approver, dispatch returns
  `CORVID_CALL_APPROVAL_REQUIRED` and the trace records the rejection
- with no approver at all, dispatch fails closed with
  `CORVID_CALL_APPROVAL_REQUIRED` and the trace records
  `decider:"fail-closed-default"`

The runtime overlay from `22-E-follow-catalog-visibility` means the active
approver also appears in `corvid_list_agents` as `__corvid_approver`, so the
host can inspect governance the same way it inspects user-defined agents.

## Run the C approval host

### Linux / macOS

```bash
cd examples/cdylib_catalog_demo
cc host_c/approver_host.c -I target/release -o host_c/approver_host -ldl
./host_c/approver_host \
  "$(pwd)/target/release/libclassify.so" \
  "$(pwd)/src/approver.cor" \
  "$hash"
```

On macOS, replace `libclassify.so` with `libclassify.dylib`.

### Windows (MSVC Build Tools)

```powershell
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$cl = & $vswhere -latest -products * -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\cl.exe'
$devcmd = Join-Path ((Split-Path (Split-Path $cl -Parent) -Parent) -replace '\\VC\\Tools\\MSVC\\.*$', '') 'Common7\Tools\VsDevCmd.bat'
cmd /c "`"$devcmd`" -arch=x64 >nul && cd /d examples\\cdylib_catalog_demo && cl /nologo /W4 /I target\\release host_c\\approver_host.c /Fe:host_c\\approver_host.exe"
examples\cdylib_catalog_demo\host_c\approver_host.exe `
  (Resolve-Path examples\cdylib_catalog_demo\target\release\classify.dll) `
  (Resolve-Path examples\cdylib_catalog_demo\src\approver.cor) `
  $hash
```

Expected output contains these lines:

```text
verified_before=1
verified_after_registration=1
catalog_has_approver=1
preflight_status=0 requires_approval=1 cost_bound_usd=0.02
accept_call_status=0 result="approved"
reject_call_status=4 site=EchoString
fail_closed_call_status=4 site=EchoString
trace_path=trace_output/approval_demo.jsonl
```

The trace file then contains the three approval outcomes:

- an `approval_decision` from `corvid-agent:.../approver.cor` with `"accepted":true`
- an `approval_decision` from `corvid-agent:.../approver_reject.cor` with `"accepted":false`
- an `approval_decision` from `fail-closed-default` with `"accepted":false`

## Other host demos

- `host_c/host.c` remains the minimal `22-C` catalog smoke test
- `host_rust/` shows catalog loading through `libloading`
- `host_py/demo.py` shows the same flow through `ctypes`

Those smaller hosts only call `classify`; `approver_host.c` is the governance
demo that exercises approval registration, catalog overlay, and trace evidence.
