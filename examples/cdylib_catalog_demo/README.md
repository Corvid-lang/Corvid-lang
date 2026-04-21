# cdylib catalog demo

This example shows the `22-C` capability-store surface:

- embedded `CORVID_ABI_DESCRIPTOR`
- `corvid_abi_verify`
- `corvid_list_agents`
- `corvid_pre_flight`
- `corvid_call_agent`

The exported Corvid agent is scalar-only on purpose so the Phase `22-C`
catalog dispatcher can call it generically across the C ABI.

## Build

From the repository root:

```powershell
cargo run -q -p corvid-cli -- build examples/cdylib_catalog_demo/src/classify.cor --target=cdylib --all-artifacts
$hash = cargo run -q -p corvid-cli -- abi hash examples/cdylib_catalog_demo/src/classify.cor
```

The build writes:

- `examples/cdylib_catalog_demo/target/release/lib_classify.h`
- `examples/cdylib_catalog_demo/target/release/<platform library>`
- `examples/cdylib_catalog_demo/target/release/classify.corvid-abi.json`

## Run the C host

### Linux / macOS

```bash
cd examples/cdylib_catalog_demo
cc host_c/host.c -I target/release -o host_c/host -ldl
./host_c/host target/release/libclassify.so "$hash"
```

On macOS, replace `libclassify.so` with `libclassify.dylib`.

### Windows (MSVC Build Tools)

```powershell
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$cl = & $vswhere -latest -products * -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\cl.exe'
$devcmd = Join-Path ((Split-Path (Split-Path $cl -Parent) -Parent) -replace '\\VC\\Tools\\MSVC\\.*$', '') 'Common7\Tools\VsDevCmd.bat'
cmd /c "`"$devcmd`" -arch=x64 >nul && cd /d examples\\cdylib_catalog_demo && cl /nologo /W4 /I target\\release host_c\\host.c /Fe:host_c\\host.exe"
examples\cdylib_catalog_demo\host_c\host.exe examples\cdylib_catalog_demo\target\release\classify.dll $hash
```

Expected output:

```text
verified=1
agent_count=7
first_agent=classify
preflight_status=0 cost_bound_usd=0.01 requires_approval=0
call_status=0 result="positive"
```

On Windows, prefer passing an absolute DLL path to the host demos. That keeps
the loader pinned to the exact built `classify.dll` instead of depending on the
process working directory.

## Rust and Python hosts

- `host_rust/` shows the same flow using `libloading`
- `host_py/demo.py` shows the same flow using `ctypes`

Both demos expect:

- `CORVID_MODEL=mock-1`
- `CORVID_TEST_MOCK_LLM=1`
- `CORVID_TEST_MOCK_LLM_REPLIES={"classify_prompt":"positive"}`

The sample programs set those environment variables themselves so the demo
stays offline and deterministic.
