    use super::*;
    use crate::diagnostic::line_col_of;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    const OK_SRC: &str = r#"
tool get_order(id: String) -> Order
type Order:
    id: String

agent fetch(id: String) -> Order:
    return get_order(id)
"#;

    const BAD_EFFECT_SRC: &str = r#"
tool issue_refund(id: String, amount: Float) -> Receipt dangerous
type Receipt:
    id: String

agent bad(id: String, amount: Float) -> Receipt:
    return issue_refund(id, amount)
"#;

    fn serve_once(path: &'static str, body: impl Into<String>) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let status = if request.starts_with(&format!("GET {path} ")) {
                "HTTP/1.1 200 OK"
            } else {
                "HTTP/1.1 404 Not Found"
            };
            let body = if status.contains("200") {
                body.as_str()
            } else {
                "not found"
            };
            write!(
                stream,
                "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.as_bytes().len(),
                body
            )
            .unwrap();
        });
        format!("http://{addr}{path}")
    }

    #[test]
    fn clean_source_produces_python() {
        let r = compile(OK_SRC);
        assert!(r.diagnostics.is_empty(), "diagnostics: {:?}", r.diagnostics);
        assert!(r.python_source.is_some());
        let py = r.python_source.unwrap();
        assert!(py.contains("async def fetch(id):"));
    }

    #[test]
    fn missing_approve_surfaces_as_diagnostic() {
        let r = compile(BAD_EFFECT_SRC);
        assert!(r.python_source.is_none());
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("dangerous") && d.message.contains("issue_refund")),
            "diagnostics: {:?}",
            r.diagnostics
        );
        let hint = r
            .diagnostics
            .iter()
            .find_map(|d| d.hint.clone())
            .expect("expected a hint for the UnapprovedDangerousCall");
        assert!(hint.contains("approve IssueRefund"), "hint was: {hint}");
    }

    #[test]
    fn build_to_disk_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("hello.cor");
        std::fs::write(&src_path, OK_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        let path = out.output_path.expect("expected output path");
        assert!(path.exists(), "expected {} to exist", path.display());
        let py = std::fs::read_to_string(&path).unwrap();
        assert!(py.contains("async def fetch"));
    }

    #[test]
    fn build_to_disk_resolves_corvid_imports() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("types.cor"),
            "\
public type Receipt:
    id: String
",
        )
        .unwrap();
        let src_path = tmp.path().join("main.cor");
        std::fs::write(
            &src_path,
            "\
import \"./types\" as t

agent read(r: t.Receipt) -> String:
    return r.id
",
        )
        .unwrap();

        let out = build_to_disk(&src_path).unwrap();
        assert!(
            out.diagnostics.is_empty(),
            "build should resolve imported type: {:?}",
            out.diagnostics
        );
        let path = out.output_path.expect("expected output path");
        let py = std::fs::read_to_string(&path).unwrap();
        assert!(py.contains("async def read"));
    }

    #[test]
    fn compile_to_ir_at_path_resolves_imported_struct_signatures() {
        let tmp = tempfile::tempdir().unwrap();
        let types_path = tmp.path().join("types.cor");
        std::fs::write(
            &types_path,
            "\
public type Receipt:
    id: String
",
        )
        .unwrap();
        let main_src = "\
import \"./types\" as t

agent read(r: t.Receipt) -> String:
    return r.id
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("file-backed compile should resolve imported struct");
        let read = ir.agents.iter().find(|agent| agent.name == "read").unwrap();
        match &read.params[0].ty {
            corvid_types::Type::ImportedStruct(imported) => {
                assert_eq!(imported.name, "Receipt");
                assert!(imported.module_path.ends_with("types.cor"));
            }
            other => panic!("expected imported struct param, got {other:?}"),
        }
    }

    #[test]
    fn compile_to_ir_at_path_reports_private_imported_type() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("hidden.cor"),
            "\
type Internal:
    secret: String
",
        )
        .unwrap();
        let main_src = "\
import \"./hidden\" as h

agent bad(x: h.Internal) -> String:
    return \"no\"
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("private imported type should fail");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("declaration `Internal` in the module imported as `h` is private")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_lowers_qualified_imported_agent_call() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent apply_policy_default() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" as p

agent main() -> Bool:
    return p.apply_policy_default()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("qualified imported agent call should compile");
        let imported = ir
            .agents
            .iter()
            .find(|agent| agent.name == "apply_policy_default")
            .expect("imported agent should be appended to IR");
        let main = ir
            .agents
            .iter()
            .find(|agent| agent.name == "main")
            .expect("main agent");
        let corvid_ir::IrStmt::Return {
            value: Some(value), ..
        } = &main.body.stmts[0]
        else {
            panic!("expected return call");
        };
        let corvid_ir::IrExprKind::Call { kind, .. } = &value.kind else {
            panic!("expected call expression");
        };
        assert!(matches!(
            kind,
            corvid_ir::IrCallKind::Agent { def_id } if *def_id == imported.id
        ));
    }

    #[test]
    fn compile_to_ir_at_path_lowers_import_use_agent_call() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent apply_policy_default() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" use apply_policy_default as apply_policy

agent main() -> Bool:
    return apply_policy()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("import-use agent call should compile");
        let imported = ir
            .agents
            .iter()
            .find(|agent| agent.name == "apply_policy_default")
            .expect("imported agent should be appended to IR");
        let main = ir.agents.iter().find(|agent| agent.name == "main").unwrap();
        let corvid_ir::IrStmt::Return {
            value: Some(value), ..
        } = &main.body.stmts[0]
        else {
            panic!("expected return call");
        };
        let corvid_ir::IrExprKind::Call {
            kind,
            callee_name,
            ..
        } = &value.kind
        else {
            panic!("expected call expression");
        };
        assert_eq!(callee_name, "apply_policy_default");
        assert!(matches!(
            kind,
            corvid_ir::IrCallKind::Agent { def_id } if *def_id == imported.id
        ));
    }

    #[test]
    fn compile_to_ir_at_path_resolves_import_use_type_alias() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("types.cor"),
            "\
public type Receipt:
    id: String
",
        )
        .unwrap();
        let main_src = "\
import \"./types\" use Receipt as ReviewReceipt

agent read(r: ReviewReceipt) -> String:
    return r.id
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("import-use type alias should compile");
        let read = ir.agents.iter().find(|agent| agent.name == "read").unwrap();
        match &read.params[0].ty {
            corvid_types::Type::ImportedStruct(imported) => {
                assert_eq!(imported.name, "Receipt");
                assert!(imported.module_path.ends_with("types.cor"));
            }
            other => panic!("expected imported struct param, got {other:?}"),
        }
    }

    #[test]
    fn compile_to_ir_at_path_rejects_unknown_import_use_member() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent apply_policy_default() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" use missing_policy

agent main() -> Bool:
    return true
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("unknown import-use member should fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic
                    .message
                    .contains("has no declaration named `missing_policy`")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_rejects_import_use_shadowing_local_decl() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("types.cor"),
            "\
public type Receipt:
    id: String
",
        )
        .unwrap();
        let main_src = "\
import \"./types\" use Receipt

type Receipt:
    local_id: String

agent main(r: Receipt) -> String:
    return r.local_id
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("import-use names must not silently shadow local declarations");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("duplicate declaration `Receipt`")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_accepts_import_requires_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public @deterministic
agent safe() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @deterministic as p

agent main() -> Bool:
    return p.safe()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("deterministic import contract should accept deterministic public exports");
    }

    #[test]
    fn compile_to_ir_at_path_rejects_import_requires_deterministic_for_agent_export() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent unsafe_policy() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @deterministic as p

agent main() -> Bool:
    return true
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("non-deterministic public export should violate import contract");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("exported agent is not marked `@deterministic`")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_rejects_import_requires_deterministic_for_prompt_export() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public prompt decide() -> Bool:
    \"true\"
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @deterministic as p

agent main() -> Bool:
    return true
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("prompt export should violate deterministic import contract");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("which is not deterministic at a module boundary")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_checks_import_required_budget() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
effect paid:
    cost: $1.00

tool pay() -> String uses paid

public agent expensive() -> String:
    return pay()
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @budget($0.50) as p

agent main() -> String:
    return p.expensive()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("import budget contract should reject expensive exported agent");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("dimension `cost`: constraint requires $0.5000, but composed value is $1.0000")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_accepts_import_required_budget_within_bound() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
effect paid:
    cost: $1.00

tool pay() -> String uses paid

public agent expensive() -> String:
    return pay()
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @budget($2.00) as p

agent main() -> String:
    return p.expensive()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("import budget contract should accept exported agent within budget");
    }

    #[test]
    fn compile_to_ir_at_path_accepts_import_requires_replayable() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public @replayable
agent replay_safe() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @replayable as p

agent main() -> Bool:
    return p.replay_safe()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("replayable import contract should accept replayable exported agents");
    }

    #[test]
    fn compile_to_ir_at_path_rejects_import_requires_replayable_for_agent_export() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent ordinary() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @replayable as p

agent main() -> Bool:
    return true
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("ordinary exported agent should violate replayable import contract");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("exported agent is not marked `@replayable` or `@deterministic`")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_rejects_import_requires_wrapping_as_noop_contract() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent ordinary() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" requires @wrapping as p

agent main() -> Bool:
    return true
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("wrapping import requirement should be rejected instead of ignored");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("`@wrapping` is an agent execution mode")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_rejects_hash_pinned_import_drift() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent safe() -> Bool:
    return true
",
        )
        .unwrap();
        let wrong = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let main_src = format!(
            "\
import \"./policy\" hash:sha256:{wrong} as p

agent main() -> Bool:
    return true
"
        );
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, &main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(&main_src, &main_path, None)
            .expect_err("drifted pinned import should fail compilation");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("failed content-hash verification")),
            "diagnostics: {diagnostics:?}"
        );
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("review the imported source before updating the pin"))),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn compile_to_ir_at_path_lowers_remote_hash_pinned_import() {
        let tmp = tempfile::tempdir().unwrap();
        let policy_src = "\
public type RemoteReceipt:
    id: String
";
        let digest = crate::import_integrity::sha256_hex(policy_src.as_bytes());
        let url = serve_once("/policy.cor", policy_src);
        let main_src = format!(
            "\
import \"{url}\" hash:sha256:{digest} as p

agent main(r: p.RemoteReceipt) -> String:
    return r.id
"
        );
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, &main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(&main_src, &main_path, None)
            .expect("remote pinned import should compile after hash verification");
        assert!(ir.types.iter().any(|ty| ty.name == "RemoteReceipt"));
        assert!(matches!(
            ir.imports[0].source,
            corvid_ir::IrImportSource::RemoteCorvid
        ));
    }

    #[test]
    fn compile_to_ir_at_path_lowers_package_import_from_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let policy_src = "\
public type PackageReceipt:
    id: String
";
        let digest = crate::import_integrity::sha256_hex(policy_src.as_bytes());
        let url = serve_once("/safety-baseline-v2.3.cor", policy_src);
        std::fs::write(
            tmp.path().join("Corvid.lock"),
            format!(
                "\
[[package]]
uri = \"corvid://@anthropic/safety-baseline/v2.3\"
url = \"{url}\"
sha256 = \"{digest}\"
registry = \"https://registry.corvid.dev\"
signature = \"unsigned:test-fixture\"
"
            ),
        )
        .unwrap();
        let main_src = "\
import \"corvid://@anthropic/safety-baseline/v2.3\" as safety

agent main(r: safety.PackageReceipt) -> String:
    return r.id
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("locked package import should compile after hash verification");
        assert!(ir.types.iter().any(|ty| ty.name == "PackageReceipt"));
        assert!(matches!(
            ir.imports[0].source,
            corvid_ir::IrImportSource::PackageCorvid
        ));
    }

    #[test]
    fn import_semantic_summary_reports_effect_approval_grounding_and_replayability() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
effect paid:
    cost: $1.00
    trust: human_required

tool lookup() -> String uses retrieval
public tool refund() -> String uses paid

public @replayable
agent summarize() -> Grounded<String>:
    return lookup()
",
        )
        .unwrap();
        let main_path = tmp.path().join("main.cor");
        std::fs::write(
            &main_path,
            "\
import \"./policy\" as p

agent main() -> String:
    return p.refund()
",
        )
        .unwrap();

        let summaries = inspect_import_semantics(&main_path).expect("import summary");
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0].summary;
        let refund = summary.exports.get("refund").expect("refund export");
        assert!(refund.approval_required);
        assert_eq!(refund.effect_names, vec!["paid".to_string()]);
        let summarize = summary.exports.get("summarize").expect("agent export");
        assert!(summarize.replayable);
        assert!(summarize.grounded_return);
        let agent = summary.agents.get("summarize").expect("agent summary");
        assert!(agent.replayable);
        assert!(agent.grounded_return);
        assert!(agent
            .composed_dimensions
            .contains_key("data"));
    }

    #[test]
    fn render_import_semantic_summaries_includes_developer_flags() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
effect paid:
    cost: $0.25
    trust: human_required

public tool refund() -> String uses paid
",
        )
        .unwrap();
        let main_path = tmp.path().join("main.cor");
        std::fs::write(
            &main_path,
            "\
import \"./policy\" as p

agent main() -> Bool:
    return true
",
        )
        .unwrap();

        let summaries = inspect_import_semantics(&main_path).expect("import summary");
        let rendered = render_import_semantic_summaries(&summaries);
        assert!(rendered.contains("import ./policy ->"));
        assert!(rendered.contains("refund (Tool)"));
        assert!(rendered.contains("approval_required"));
        assert!(rendered.contains("effects=[paid]"));
    }

    #[tokio::test]
    async fn run_with_runtime_executes_qualified_imported_agent_call() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent apply_policy_default() -> Bool:
    return true
",
        )
        .unwrap();
        let main_path = tmp.path().join("main.cor");
        std::fs::write(
            &main_path,
            "\
import \"./policy\" as p

agent main() -> Bool:
    return p.apply_policy_default()
",
        )
        .unwrap();

        let rt = Runtime::builder().build();
        let value = run_with_runtime(&main_path, Some("main"), vec![], &rt)
            .await
            .expect("imported agent call should run");
        assert_eq!(value, Value::Bool(true));
    }

    #[tokio::test]
    async fn run_with_runtime_executes_import_use_agent_call() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
public agent apply_policy_default() -> Bool:
    return true
",
        )
        .unwrap();
        let main_path = tmp.path().join("main.cor");
        std::fs::write(
            &main_path,
            "\
import \"./policy\" use apply_policy_default as apply_policy

agent main() -> Bool:
    return apply_policy()
",
        )
        .unwrap();

        let rt = Runtime::builder().build();
        let value = run_with_runtime(&main_path, Some("main"), vec![], &rt)
            .await
            .expect("import-use agent call should run");
        assert_eq!(value, Value::Bool(true));
    }

    #[test]
    fn compile_to_ir_at_path_lowers_qualified_imported_tool_call() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tools.cor"),
            "\
public tool lookup() -> String
",
        )
        .unwrap();
        let main_src = "\
import \"./tools\" as t

agent main() -> String:
    return t.lookup()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("qualified imported tool call should compile");
        let imported = ir
            .tools
            .iter()
            .find(|tool| tool.name == "lookup")
            .expect("imported tool should be appended to IR");
        let main = ir.agents.iter().find(|agent| agent.name == "main").unwrap();
        let corvid_ir::IrStmt::Return {
            value: Some(value), ..
        } = &main.body.stmts[0]
        else {
            panic!("expected return call");
        };
        let corvid_ir::IrExprKind::Call { kind, .. } = &value.kind else {
            panic!("expected call expression");
        };
        assert!(matches!(
            kind,
            corvid_ir::IrCallKind::Tool { def_id, .. } if *def_id == imported.id
        ));
    }

    #[test]
    fn compile_to_ir_at_path_lowers_qualified_imported_type_constructor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("types.cor"),
            "\
public type Receipt:
    id: String
",
        )
        .unwrap();
        let main_src = "\
import \"./types\" as t

agent main() -> t.Receipt:
    return t.Receipt(\"r_1\")
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let ir = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect("qualified imported type constructor should compile");
        let imported = ir
            .types
            .iter()
            .find(|ty| ty.name == "Receipt")
            .expect("imported type should be appended to IR");
        let main = ir.agents.iter().find(|agent| agent.name == "main").unwrap();
        let corvid_ir::IrStmt::Return {
            value: Some(value), ..
        } = &main.body.stmts[0]
        else {
            panic!("expected return call");
        };
        let corvid_ir::IrExprKind::Call { kind, .. } = &value.kind else {
            panic!("expected call expression");
        };
        assert!(matches!(
            kind,
            corvid_ir::IrCallKind::StructConstructor { def_id } if *def_id == imported.id
        ));
    }

    #[test]
    fn compile_to_ir_at_path_reports_private_imported_callable() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("policy.cor"),
            "\
agent hidden() -> Bool:
    return true
",
        )
        .unwrap();
        let main_src = "\
import \"./policy\" as p

agent main() -> Bool:
    return p.hidden()
";
        let main_path = tmp.path().join("main.cor");
        std::fs::write(&main_path, main_src).unwrap();

        let diagnostics = compile_to_ir_with_config_at_path(main_src, &main_path, None)
            .expect_err("private imported callable should fail");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic
                    .message
                    .contains("declaration `hidden` in the module imported as `p` is private")),
            "diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn build_to_disk_with_src_dir_places_output_in_sibling_target() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_path = src_dir.join("main.cor");
        std::fs::write(&src_path, OK_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        let path = out.output_path.expect("expected output path");
        let expected = tmp.path().join("target").join("py").join("main.py");
        assert_eq!(path, expected);
    }

    #[test]
    fn build_emits_no_file_when_diagnostics_present() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("bad.cor");
        std::fs::write(&src_path, BAD_EFFECT_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        assert!(out.output_path.is_none());
        assert!(!out.diagnostics.is_empty());
    }

    #[test]
    fn scaffold_new_creates_expected_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = scaffold_new_in(tmp.path(), "my_bot").unwrap();
        assert!(root.join("corvid.toml").exists());
        assert!(root.join("src/main.cor").exists());
        assert!(root.join("tools.py").exists());
        assert!(root.join(".gitignore").exists());
    }

    #[test]
    fn scaffold_rejects_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("already_there")).unwrap();
        let err = scaffold_new_in(tmp.path(), "already_there").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn line_col_translation() {
        let src = "abc\ndef\nghi";
        assert_eq!(line_col_of(src, 0), (1, 1));
        assert_eq!(line_col_of(src, 2), (1, 3));
        assert_eq!(line_col_of(src, 4), (2, 1));
        assert_eq!(line_col_of(src, 8), (3, 1));
    }

    // ----------------------------------------------------------
    // Native run integration: drive the compiler + runtime end-to-end
    // ----------------------------------------------------------

    use serde_json::json;
    use std::sync::Arc;

    const REFUND_BOT_SRC: &str = r#"
type Ticket:
    order_id: String
    reason: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool

type Receipt:
    refund_id: String

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """Decide whether to refund: ticket={ticket} order={order}."""

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)
    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)
    return decision
"#;

    fn refund_bot_runtime(trace_dir: &Path) -> Runtime {
        Runtime::builder()
            .tool("get_order", |args| async move {
                let id = args[0].as_str().unwrap_or("");
                Ok(json!({ "id": id, "amount": 49.99 }))
            })
            .tool("issue_refund", |args| async move {
                let id = args[0].as_str().unwrap_or("");
                Ok(json!({ "refund_id": format!("rf_{id}") }))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1")
                    .reply("decide_refund", json!({ "should_refund": true })),
            ))
            .default_model("mock-1")
            .trace_to(trace_dir)
            .build()
    }

    #[tokio::test]
    async fn refund_bot_runs_end_to_end_via_driver() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("refund_bot.cor");
        std::fs::write(&src_path, REFUND_BOT_SRC).unwrap();
        let trace_dir = tmp.path().join("trace");

        let rt = refund_bot_runtime(&trace_dir);

        // Build a Ticket struct as the agent's input.
        let ir = compile_to_ir(REFUND_BOT_SRC).expect("clean compile");
        let ticket_id = ir.types.iter().find(|t| t.name == "Ticket").unwrap().id;
        let ticket = corvid_vm::build_struct(
            ticket_id,
            "Ticket",
            [
                ("order_id".to_string(), Value::String(Arc::from("ord_42"))),
                ("reason".to_string(), Value::String(Arc::from("damaged"))),
            ],
        );

        let v = run_with_runtime(&src_path, Some("refund_bot"), vec![ticket], &rt)
            .await
            .expect("run");

        match v {
            Value::Struct(s) => {
                assert_eq!(s.type_name(), "Decision");
                assert_eq!(s.get_field("should_refund").unwrap(), Value::Bool(true));
            }
            other => panic!("expected Decision struct, got {other:?}"),
        }

        // A trace file should have been written.
        let traces: Vec<_> = std::fs::read_dir(&trace_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "jsonl")
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(traces.len(), 1, "expected exactly one .jsonl trace file");
        let body = std::fs::read_to_string(traces[0].path()).unwrap();
        assert!(body.contains("\"kind\":\"run_started\""));
        assert!(body.contains("\"kind\":\"tool_call\""));
        assert!(body.contains("\"kind\":\"approval_response\""));
        assert!(body.contains("\"approved\":true"));
        assert!(body.contains("\"kind\":\"run_completed\""));
    }

    #[tokio::test]
    async fn run_errors_when_no_agent_selected_among_many() {
        let src = "agent a() -> Int:\n    return 1\nagent b() -> Int:\n    return 2\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("two.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let err = run_with_runtime(&path, None, vec![], &rt).await.unwrap_err();
        assert!(matches!(err, RunError::AmbiguousAgent { .. }));
    }

    #[tokio::test]
    async fn run_picks_main_when_present() {
        let src = "agent helper() -> Int:\n    return 1\nagent main() -> Int:\n    return 99\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("main.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let v = run_with_runtime(&path, None, vec![], &rt).await.unwrap();
        assert_eq!(v, Value::Int(99));
    }

    #[tokio::test]
    async fn run_rejects_agent_needing_args_with_clear_error() {
        let src = "agent needs(n: Int) -> Int:\n    return n + 1\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("needs.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let err = run_with_runtime(&path, None, vec![], &rt).await.unwrap_err();
        match err {
            RunError::NeedsArgs { agent, expected } => {
                assert_eq!(agent, "needs");
                assert_eq!(expected, 1);
            }
            other => panic!("expected NeedsArgs, got {other:?}"),
        }
    }

    // ========================================================
    // Native-tier dispatch plus compile cache.
    // ========================================================

    const NATIVE_ABLE_SRC: &str = "agent main() -> Int:\n    return 7 * 6\n";

    const TOOL_USING_SRC: &str = r#"
tool lookup(id: String) -> Int
agent main(id: String) -> Int:
    return lookup(id)
"#;

    const PYTHON_IMPORT_SRC: &str = r#"
import python "math" as math effects: unsafe

agent main() -> Int:
    return 1
"#;

    const PROMPT_USING_SRC: &str = r#"
prompt greet(name: String) -> String:
    """
    Say hi to {name}.
    """

agent main() -> String:
    return greet("world")
"#;

    const NULLABLE_OPTION_STRING_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent main() -> Bool:
    return maybe(true) != None
"#;

    const WIDE_OPTION_INT_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent main() -> Bool:
    return maybe(true) != None
"#;

    const WIDE_OPTION_INT_TRY_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent unwrap(flag: Bool) -> Option<Int>:
    value = maybe(flag)?
    return Some(value + 1)

agent main() -> Bool:
    return unwrap(true) != None
"#;

    const WIDE_OPTION_INT_TRY_WIDEN_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent widen(flag: Bool) -> Option<Bool>:
    value = maybe(flag)?
    return Some(value > 0)

agent main() -> Bool:
    return widen(true) != None
"#;

    const NULLABLE_OPTION_TRY_WIDEN_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent widen(flag: Bool) -> Option<Bool>:
    value = maybe(flag)?
    return Some(value == "hi")

agent main() -> Bool:
    return widen(true) != None
"#;

    const NULLABLE_OPTION_TRY_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent unwrap(flag: Bool) -> Option<String>:
    value = maybe(flag)?
    return Some(value)

agent main() -> Bool:
    return unwrap(true) != None
"#;

    const NATIVE_RESULT_STRING_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent forward(flag: Bool) -> Result<String, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_TRY_WIDEN_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent widen(flag: Bool) -> Result<Bool, String>:
    value = fetch(flag)?
    return Ok(true)

agent main() -> Bool:
    first = widen(true)
    second = widen(false)
    return true
"#;

    const NATIVE_RESULT_RETRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent retrying(flag: Bool) -> Result<String, String>:
    return try fetch(flag) on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying(true)
    second = retrying(false)
    return true
"#;

    const NATIVE_OPTION_RETRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent retrying(flag: Bool) -> Option<Int>:
    return try fetch(flag) on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying(true)
    second = retrying(false)
    return true
"#;

    const NATIVE_NESTED_OPTION_INT_SRC: &str = r#"
agent fetch(mode: Int) -> Option<Option<Int>>:
    if mode == 0:
        return None
    if mode == 1:
        return Some(None)
    return Some(Some(7))

agent main() -> Bool:
    first = fetch(0)
    second = fetch(1)
    third = fetch(2)
    return first == None and second != None and third != None
"#;

    const NATIVE_NESTED_OPTION_INT_TRY_SRC: &str = r#"
agent fetch(mode: Int) -> Option<Option<Int>>:
    if mode == 0:
        return None
    if mode == 1:
        return Some(None)
    return Some(Some(7))

agent inspect(mode: Int) -> Option<Bool>:
    value = fetch(mode)?
    return Some(value == None or value != None)

agent main() -> Bool:
    return inspect(0) == None and inspect(1) != None and inspect(2) != None
"#;

    const NATIVE_RESULT_OPTION_INT_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Option<Int>, String>:
    if flag:
        return Ok(Some(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_OPTION_INT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Option<Int>, String>:
    if flag:
        return Ok(Some(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Option<Int>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_OPTION_INT_RETRY_SRC: &str = r#"
prompt probe() -> String:
    """
    Probe
    """

agent fetch() -> Result<Option<Int>, String>:
    value = probe()
    if value == "ok":
        return Ok(Some(7))
    return Err(value)

agent retrying() -> Result<Option<Int>, String>:
    return try fetch() on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying()
    return probe() == "marker"
"#;

    const NATIVE_RESULT_STRUCT_SRC: &str = r#"
type Boxed:
    value: Int

agent fetch(flag: Bool) -> Result<Boxed, String>:
    if flag:
        return Ok(Boxed(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_STRUCT_TRY_SRC: &str = r#"
type Boxed:
    value: Int

agent fetch(flag: Bool) -> Result<Boxed, String>:
    if flag:
        return Ok(Boxed(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Boxed, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_LIST_INT_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<List<Int>, String>:
    if flag:
        return Ok([1, 2, 3])
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_LIST_INT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<List<Int>, String>:
    if flag:
        return Ok([1, 2, 3])
    return Err("no")

agent forward(flag: Bool) -> Result<List<Int>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_OK_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:
    if flag:
        return Ok(Ok(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_OK_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:
    if flag:
        return Ok(Ok(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Result<Int, String>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_ERR_TRY_SRC: &str = r#"
agent inner_error() -> Result<String, Bool>:
    return Err(false)

agent fetch(flag: Bool) -> Result<Int, Result<String, Bool>>:
    if flag:
        return Ok(7)
    return Err(inner_error())

agent widen(flag: Bool) -> Result<Bool, Result<String, Bool>>:
    value = fetch(flag)?
    return Ok(value == 7)

agent main() -> Bool:
    first = widen(true)
    second = widen(false)
    return true
"#;

    const NATIVE_STRING_RETRY_REJECTED_SRC: &str = r#"
prompt lookup(id: String) -> String:
    """
    Lookup {id}
    """

agent load(id: String) -> String:
    return try lookup(id) on error retry 3 times backoff exponential 40
"#;

    #[test]
    fn native_ability_accepts_pure_computation() {
        let ir = compile_to_ir(NATIVE_ABLE_SRC).expect("compile");
        assert!(native_ability(&ir).is_ok());
    }

    #[test]
    fn native_ability_rejects_tool_call() {
        let ir = compile_to_ir(TOOL_USING_SRC).expect("compile");
        match native_ability(&ir) {
            Err(NotNativeReason::ToolCall { name }) => assert_eq!(name, "lookup"),
            other => panic!("expected ToolCall rejection, got {other:?}"),
        }
    }

    #[test]
    fn native_ability_rejects_python_import() {
        let ir = compile_to_ir(PYTHON_IMPORT_SRC).expect("compile");
        match native_ability(&ir) {
            Err(NotNativeReason::PythonImport { module }) => assert_eq!(module, "math"),
            other => panic!("expected PythonImport rejection, got {other:?}"),
        }
    }

    #[test]
    fn native_ability_accepts_prompt_calls() {
        // Prompt calls compile and run natively against the runtime's
        // bundled LLM adapters.
        let ir = compile_to_ir(PROMPT_USING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "prompt support is native now; scan should accept prompt-using IRs"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_with_refcounted_payload() {
        let ir = compile_to_ir(NULLABLE_OPTION_STRING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "nullable-pointer Option<String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_payloads() {
        let ir = compile_to_ir(WIDE_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "wide scalar Option<Int> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_try_propagation() {
        let ir = compile_to_ir(NULLABLE_OPTION_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "nullable Option<String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_try_propagation() {
        let ir = compile_to_ir(WIDE_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "wide scalar Option<Int> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_try_with_different_payload_type() {
        let ir = compile_to_ir(WIDE_OPTION_INT_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Int> `?` inside Option<Bool> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_try_with_wide_outer_payload() {
        let ir = compile_to_ir(NULLABLE_OPTION_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<String> `?` inside Option<Bool> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_subset() {
        let ir = compile_to_ir(NATIVE_RESULT_STRING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "one-word Result<String, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "same-shape Result<String, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_try_with_different_ok_type() {
        let ir = compile_to_ir(NATIVE_RESULT_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<A, E> `?` inside Result<B, E> should now compile natively when the error type matches"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_retry_subset() {
        let ir = compile_to_ir(NATIVE_RESULT_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over the native Result<T, E> subset should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_option_retry_subset() {
        let ir = compile_to_ir(NATIVE_OPTION_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over the native Option<T> subset should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nested_option_payloads() {
        let ir = compile_to_ir(NATIVE_NESTED_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Option<Int>> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nested_option_try_propagation() {
        let ir = compile_to_ir(NATIVE_NESTED_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Option<Int>> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Option<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Option<Int>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_retry() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over Result<Option<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_struct_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_STRUCT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Struct, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_struct_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_STRUCT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Struct, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_list_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_LIST_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<List<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_list_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_LIST_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<List<Int>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_ok_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_OK_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Result<Int, String>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_ok_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_OK_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Result<Int, String>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_error_try_widening() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_ERR_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<A, Result<B, C>> `?` inside Result<D, Result<B, C>> should now compile natively"
        );
    }

    #[test]
    fn retry_over_non_result_or_option_errors_before_native_scan() {
        let diagnostics = compile_to_ir(NATIVE_STRING_RETRY_REJECTED_SRC)
            .expect_err("retry over plain String should fail typecheck");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("can only be used on `Result` or `Option`")),
            "diagnostics: {diagnostics:?}"
        );
    }

    /// Second compilation of the same source hits the cache: no
    /// recompile, binary is the same path, mtime doesn't advance.
    #[test]
    fn native_cache_hits_on_second_call() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("hello.cor");
        std::fs::write(&src_path, NATIVE_ABLE_SRC).expect("write");
        let ir = compile_to_ir(NATIVE_ABLE_SRC).expect("compile");

        let first = build_or_get_cached_native(&src_path, NATIVE_ABLE_SRC, &ir, None).expect("first");
        assert!(!first.from_cache, "first call must compile (not cached yet)");
        assert!(first.path.exists(), "first build should produce a binary");
        let first_mtime = std::fs::metadata(&first.path).unwrap().modified().unwrap();

        let second = build_or_get_cached_native(&src_path, NATIVE_ABLE_SRC, &ir, None).expect("second");
        assert!(second.from_cache, "second call must reuse cached binary");
        assert_eq!(first.path, second.path, "same cache key => same path");
        let second_mtime = std::fs::metadata(&second.path).unwrap().modified().unwrap();
        assert_eq!(
            first_mtime, second_mtime,
            "cache hit must not rewrite the binary"
        );
    }

    /// Auto-dispatch on a native-able program runs via native and
    /// produces the binary under `target/cache/native/`. The exit code
    /// from `run_with_target` comes from the spawned binary itself.
    #[test]
    fn run_with_target_auto_uses_native_for_pure_program() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("pure.cor");
        std::fs::write(&src_path, NATIVE_ABLE_SRC).expect("write");

        let code = run_with_target(&src_path, RunTarget::Auto, None).expect("run");
        assert_eq!(code, 0, "pure program should exit 0");
        // Cache populated under <tmpdir>/target/cache/native/.
        let cache_dir = tmp.path().join("target").join("cache").join("native");
        assert!(
            cache_dir.exists(),
            "native cache dir should exist after auto-run, got missing: {}",
            cache_dir.display()
        );
        let entries: Vec<_> = std::fs::read_dir(&cache_dir).unwrap().collect();
        assert!(
            !entries.is_empty(),
            "native cache dir should contain at least one binary"
        );
    }

    /// `--target=native` on a tool-using program must NOT silently fall
    /// back — it must exit non-zero with the reason printed to stderr.
    /// Verified by checking `run_with_target` returns exit 1 and the
    /// program never runs. We don't capture stderr here (Rust tests
    /// don't expose a clean way without a process boundary), but the
    /// exit code is the contract this helper promises.
    #[test]
    fn run_with_target_native_required_errors_on_tool_use() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("tooly.cor");
        std::fs::write(&src_path, TOOL_USING_SRC).expect("write");

        let code = run_with_target(&src_path, RunTarget::Native, None).expect("run");
        assert_eq!(
            code, 1,
            "native-required on a tool-using program must exit 1"
        );
    }
