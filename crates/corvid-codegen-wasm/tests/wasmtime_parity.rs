use corvid_codegen_wasm::emit_wasm_artifacts;
use corvid_ir::{lower, IrFile};
use corvid_resolve::resolve;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use corvid_vm::Value;
use std::sync::{Arc, Mutex};
use wasmtime::{Engine, Linker, Module, Store};

struct Fixture {
    name: &'static str,
    src: &'static str,
    agent: &'static str,
    args: Vec<i64>,
}

fn ir_of(src: &str) -> IrFile {
    let tokens = lex(src).expect("lex");
    let (parsed, parse_errors) = parse_file(&tokens);
    assert!(
        parse_errors.is_empty(),
        "parse errors: {:?}",
        parse_errors
    );
    let resolved = resolve(&parsed);
    assert!(
        resolved.errors.is_empty(),
        "resolve errors: {:?}",
        resolved.errors
    );
    let checked = typecheck(&parsed, &resolved);
    assert!(
        checked.errors.is_empty(),
        "type errors: {:?}",
        checked.errors
    );
    lower(&parsed, &resolved, &checked)
}

fn run_interpreter_i64(ir: &IrFile, agent: &str, args: &[i64]) -> i64 {
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let values = args.iter().copied().map(Value::Int).collect();
    let value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(ir, agent, values, &runtime).await })
        .expect("interpreter run");
    match value {
        Value::Int(value) => value,
        other => panic!("expected Int from interpreter, got {other:?}"),
    }
}

fn run_wasmtime_i64(ir: &IrFile, module_name: &str, agent: &str, args: &[i64]) -> i64 {
    let artifacts = emit_wasm_artifacts(ir, module_name).expect("wasm artifacts");
    wasmparser::Validator::new()
        .validate_all(&artifacts.wasm)
        .expect("valid wasm");

    let engine = Engine::default();
    let module = Module::new(&engine, &artifacts.wasm).expect("compile wasm module");
    let mut linker = Linker::new(&engine);
    let refunds = Arc::new(Mutex::new(Vec::<i64>::new()));
    let refunds_for_tool = Arc::clone(&refunds);

    linker
        .func_wrap("corvid:host", "prompt.refund_score", |amount: i64| -> i64 {
            if amount >= 100 {
                90
            } else if amount >= 50 {
                78
            } else {
                20
            }
        })
        .expect("prompt import");
    linker
        .func_wrap("corvid:host", "approve.IssueRefund", |_amount: i64| -> i32 { 1 })
        .expect("approval import");
    linker
        .func_wrap("corvid:host", "tool.issue_refund", move |amount: i64| -> i64 {
            refunds_for_tool.lock().unwrap().push(amount);
            amount
        })
        .expect("tool import");

    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("instantiate wasm module");
    match args {
        [] => {
            let func = instance
                .get_typed_func::<(), i64>(&mut store, agent)
                .expect("exported zero-arg agent");
            func.call(&mut store, ()).expect("wasm call")
        }
        [arg] => {
            let func = instance
                .get_typed_func::<i64, i64>(&mut store, agent)
                .expect("exported one-arg agent");
            func.call(&mut store, *arg).expect("wasm call")
        }
        _ => panic!("wasmtime parity helper currently supports zero or one Int argument"),
    }
}

#[test]
fn wasmtime_matches_interpreter_for_scalar_native_parity_subset() {
    let fixtures = [
        Fixture {
            name: "math",
            src: "agent main(x: Int) -> Int:\n    y = x + 7\n    return y * 2\n",
            agent: "main",
            args: vec![11],
        },
        Fixture {
            name: "branch",
            src: "agent choose(x: Int) -> Int:\n    if x > 10:\n        return x - 3\n    return x + 3\n",
            agent: "choose",
            args: vec![14],
        },
        Fixture {
            name: "agent_call",
            src: "agent inc(x: Int) -> Int:\n    return x + 1\n\nagent main(x: Int) -> Int:\n    return inc(x) + inc(2)\n",
            agent: "main",
            args: vec![5],
        },
    ];

    for fixture in fixtures {
        let ir = ir_of(fixture.src);
        let expected = run_interpreter_i64(&ir, fixture.agent, &fixture.args);
        let observed = run_wasmtime_i64(&ir, fixture.name, fixture.agent, &fixture.args);
        assert_eq!(
            observed, expected,
            "wasmtime/interpreter mismatch for {}",
            fixture.name
        );
    }
}

#[test]
fn wasmtime_executes_scalar_prompt_approval_tool_flow_with_typed_host_imports() {
    let src = r#"
prompt refund_score(amount: Int) -> Int:
    """Return the refund risk score."""

tool issue_refund(amount: Int) -> Int dangerous

agent review_refund(amount: Int) -> Int:
    score = refund_score(amount)
    if score > 75:
        approve IssueRefund(amount)
        return issue_refund(amount)
    return 0
"#;
    let ir = ir_of(src);
    let observed = run_wasmtime_i64(&ir, "refund_gate", "review_refund", &[120]);
    assert_eq!(observed, 120);
}
