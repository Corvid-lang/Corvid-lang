use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

const RAG_QA_MOCK_CONTEXT: &str =
    "Refunds over one hundred dollars require approval before money moves.";

const RAG_QA_MOCK_TOOLS: &str =
    "{\"retrieve_context\":[\"Refunds over one hundred dollars require approval before money moves.\",\"Refunds over one hundred dollars require approval before money moves.\",\"Refunds over one hundred dollars require approval before money moves.\"]}";

#[test]
fn build_and_run_default_to_project_main_source() {
    let app = repo_root().join("examples").join("refund_bot");

    let build = Command::new(corvid_bin())
        .arg("build")
        .current_dir(&app)
        .output()
        .expect("run corvid build");
    assert!(
        build.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(build_stdout.contains("src\\main.cor") || build_stdout.contains("src/main.cor"));
    assert!(
        app.join("target").join("py").join("main.py").exists(),
        "default build should emit target/py/main.py"
    );

    let run = Command::new(corvid_bin())
        .arg("run")
        .current_dir(&app)
        .output()
        .expect("run corvid run");
    assert!(
        run.status.success(),
        "run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run_stdout.contains("refund_bot"), "{run_stdout}");
    assert!(run_stdout.contains("approval-gated refund"), "{run_stdout}");
}

#[test]
fn refund_bot_corvid_tests_and_unapproved_variant_are_covered() {
    let repo = repo_root();
    let app = repo.join("examples").join("refund_bot");

    for suite in ["unit.cor", "integration.cor"] {
        let out = Command::new(corvid_bin())
            .arg("test")
            .arg(app.join("tests").join(suite))
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|err| panic!("run corvid test {suite}: {err}"));
        assert!(
            out.status.success(),
            "{suite} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let bad_source = tmp.path().join("unapproved_refund.cor");
    std::fs::write(
        &bad_source,
        r#"
effect transfer_money:
    cost: $0.05
    trust: human_required
    reversible: false
    data: financial

type RefundRequest:
    order_id: String
    amount: Float
    reason: String

tool issue_refund(req: RefundRequest) -> String dangerous uses transfer_money

agent bypass_refund(req: RefundRequest) -> String uses transfer_money:
    return issue_refund(req)
"#,
    )
    .expect("write adversarial source");
    let out = Command::new(corvid_bin())
        .arg("check")
        .arg(&bad_source)
        .current_dir(repo)
        .output()
        .expect("run corvid check on adversarial source");
    assert!(
        !out.status.success(),
        "unapproved dangerous call should fail:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dangerous tool `issue_refund`"), "{stderr}");
    assert!(stderr.contains("approve IssueRefund"), "{stderr}");
}

#[test]
fn refund_bot_eval_harness_passes() {
    let repo = repo_root();
    let eval = repo
        .join("examples")
        .join("refund_bot")
        .join("evals")
        .join("refund_bot.cor");
    let out = Command::new(corvid_bin())
        .arg("eval")
        .arg(eval)
        .current_dir(repo)
        .output()
        .expect("run refund bot eval");
    assert!(
        out.status.success(),
        "refund bot eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("refund_bot_contract_eval"), "{stdout}");
    assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
}

#[test]
fn refund_bot_replay_fixture_is_deterministic() {
    let repo = repo_root();
    let trace = repo
        .join("examples")
        .join("refund_bot")
        .join("traces")
        .join("refund_bot_approval_gate.jsonl");
    let out = Command::new(corvid_bin())
        .arg("replay")
        .arg(trace)
        .current_dir(repo)
        .output()
        .expect("run refund bot replay");
    assert!(
        out.status.success(),
        "refund bot replay failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("trace loaded"), "{stdout}");
    assert!(stdout.contains("replay completed"), "{stdout}");
    assert!(stdout.contains("refund_bot"), "{stdout}");
    assert!(stdout.contains("approval-gated refund"), "{stdout}");
}

#[test]
fn local_model_demo_runs_with_mock_llm() {
    let app = repo_root().join("examples").join("local_model_demo");

    let out = Command::new(corvid_bin())
        .arg("run")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_RESPONSE",
            "provider-neutral local inference with deterministic replay.",
        )
        .env("CORVID_MODEL", "ollama:llama3.2")
        .current_dir(&app)
        .output()
        .expect("run local model demo");
    assert!(
        out.status.success(),
        "local model demo failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("provider-neutral local inference with deterministic replay."),
        "{stdout}"
    );
}

#[test]
fn local_model_demo_corvid_tests_pass_with_mock_llm() {
    let repo = repo_root();
    let app = repo.join("examples").join("local_model_demo");

    for suite in ["unit.cor", "integration.cor"] {
        let out = Command::new(corvid_bin())
            .arg("test")
            .arg(app.join("tests").join(suite))
            .env("CORVID_TEST_MOCK_LLM", "1")
            .env(
                "CORVID_TEST_MOCK_LLM_RESPONSE",
                "provider-neutral local inference with deterministic replay.",
            )
            .env("CORVID_MODEL", "ollama:llama3.2")
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|err| panic!("run local model test {suite}: {err}"));
        assert!(
            out.status.success(),
            "{suite} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
    }
}

#[test]
fn local_model_demo_eval_harness_passes_with_mock_llm() {
    let repo = repo_root();
    let eval = repo
        .join("examples")
        .join("local_model_demo")
        .join("evals")
        .join("local_model_demo.cor");
    let out = Command::new(corvid_bin())
        .arg("eval")
        .arg(eval)
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_RESPONSE",
            "provider-neutral local inference with deterministic replay.",
        )
        .env("CORVID_MODEL", "ollama:llama3.2")
        .current_dir(repo)
        .output()
        .expect("run local model eval");
    assert!(
        out.status.success(),
        "local model eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("local_model_demo_contract_eval"),
        "{stdout}"
    );
    assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
}

#[test]
fn local_model_demo_replay_fixture_is_deterministic() {
    let repo = repo_root();
    let trace = repo
        .join("examples")
        .join("local_model_demo")
        .join("traces")
        .join("local_model_demo_mock_chat.jsonl");
    let out = Command::new(corvid_bin())
        .arg("replay")
        .arg(trace)
        .current_dir(repo)
        .output()
        .expect("run local model replay");
    assert!(
        out.status.success(),
        "local model replay failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("trace loaded"), "{stdout}");
    assert!(stdout.contains("replay completed"), "{stdout}");
    assert!(
        stdout.contains("provider-neutral local inference with deterministic replay."),
        "{stdout}"
    );
}

#[test]
fn provider_routing_demo_runs_with_mock_llm() {
    let app = repo_root().join("examples").join("provider_routing_demo");

    let out = Command::new(corvid_bin())
        .arg("run")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_RESPONSE",
            "provider routing selected the expected mocked response.",
        )
        .current_dir(&app)
        .output()
        .expect("run provider routing demo");
    assert!(
        out.status.success(),
        "provider routing demo failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("provider routing selected the expected mocked response."),
        "{stdout}"
    );
}

#[test]
fn provider_routing_demo_corvid_tests_pass_with_mock_llm() {
    let repo = repo_root();
    let app = repo.join("examples").join("provider_routing_demo");

    for suite in ["unit.cor", "integration.cor"] {
        let out = Command::new(corvid_bin())
            .arg("test")
            .arg(app.join("tests").join(suite))
            .env("CORVID_TEST_MOCK_LLM", "1")
            .env(
                "CORVID_TEST_MOCK_LLM_RESPONSE",
                "provider routing selected the expected mocked response.",
            )
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|err| panic!("run provider routing test {suite}: {err}"));
        assert!(
            out.status.success(),
            "{suite} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
    }
}

#[test]
fn provider_routing_demo_eval_harness_passes_with_mock_llm() {
    let repo = repo_root();
    let eval = repo
        .join("examples")
        .join("provider_routing_demo")
        .join("evals")
        .join("provider_routing_demo.cor");
    let out = Command::new(corvid_bin())
        .arg("eval")
        .arg(eval)
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_RESPONSE",
            "provider routing selected the expected mocked response.",
        )
        .current_dir(repo)
        .output()
        .expect("run provider routing eval");
    assert!(
        out.status.success(),
        "provider routing eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("provider_routing_demo_contract_eval"),
        "{stdout}"
    );
    assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
}

#[test]
fn provider_routing_demo_replay_fixtures_are_deterministic() {
    let repo = repo_root();
    let trace_dir = repo
        .join("examples")
        .join("provider_routing_demo")
        .join("traces");
    let fixtures = [
        (
            "provider_routing_demo_openai.jsonl",
            "provider routing selected the expected mocked response.",
        ),
        (
            "provider_routing_demo_ollama.jsonl",
            "local provider route stays on the deterministic Ollama fixture.",
        ),
        (
            "provider_routing_demo_anthropic.jsonl",
            "deep provider route stays on the deterministic Anthropic fixture.",
        ),
    ];

    for (fixture, expected) in fixtures {
        let out = Command::new(corvid_bin())
            .arg("replay")
            .arg(trace_dir.join(fixture))
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|err| panic!("run provider routing replay {fixture}: {err}"));
        assert!(
            out.status.success(),
            "{fixture} replay failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("trace loaded"), "{stdout}");
        assert!(stdout.contains("replay completed"), "{stdout}");
        assert!(stdout.contains(expected), "{stdout}");
    }
}

#[test]
fn rag_qa_bot_runs_with_mock_retrieval_and_llm() {
    let app = repo_root().join("examples").join("rag_qa_bot");

    let out = Command::new(corvid_bin())
        .arg("run")
        .env("CORVID_TEST_MOCK_TOOLS", RAG_QA_MOCK_TOOLS)
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_RESPONSE", RAG_QA_MOCK_CONTEXT)
        .current_dir(&app)
        .output()
        .expect("run rag qa demo");
    assert!(
        out.status.success(),
        "rag qa demo failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(RAG_QA_MOCK_CONTEXT), "{stdout}");
}

#[test]
fn rag_qa_bot_corvid_tests_pass_with_mock_retrieval_and_llm() {
    let repo = repo_root();
    let app = repo.join("examples").join("rag_qa_bot");

    for suite in ["unit.cor", "integration.cor"] {
        let out = Command::new(corvid_bin())
            .arg("test")
            .arg(app.join("tests").join(suite))
            .env("CORVID_TEST_MOCK_TOOLS", RAG_QA_MOCK_TOOLS)
            .env("CORVID_TEST_MOCK_LLM", "1")
            .env("CORVID_TEST_MOCK_LLM_RESPONSE", RAG_QA_MOCK_CONTEXT)
            .current_dir(&repo)
            .output()
            .unwrap_or_else(|err| panic!("run rag qa test {suite}: {err}"));
        assert!(
            out.status.success(),
            "{suite} failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
    }
}

#[test]
fn rag_qa_bot_eval_harness_passes_with_mock_retrieval_and_llm() {
    let repo = repo_root();
    let eval = repo
        .join("examples")
        .join("rag_qa_bot")
        .join("evals")
        .join("rag_qa_bot.cor");
    let out = Command::new(corvid_bin())
        .arg("eval")
        .arg(eval)
        .env("CORVID_TEST_MOCK_TOOLS", RAG_QA_MOCK_TOOLS)
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_RESPONSE", RAG_QA_MOCK_CONTEXT)
        .current_dir(repo)
        .output()
        .expect("run rag qa eval");
    assert!(
        out.status.success(),
        "rag qa eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rag_qa_bot_contract_eval"), "{stdout}");
    assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
}

#[test]
fn rag_qa_bot_replay_fixture_is_deterministic() {
    let repo = repo_root();
    let trace = repo
        .join("examples")
        .join("rag_qa_bot")
        .join("traces")
        .join("rag_qa_bot_refund_policy.jsonl");
    let out = Command::new(corvid_bin())
        .arg("replay")
        .arg(trace)
        .current_dir(&repo)
        .output()
        .expect("run rag qa replay");
    assert!(
        out.status.success(),
        "rag qa replay failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("trace loaded"), "{stdout}");
    assert!(stdout.contains("replay completed"), "{stdout}");
    assert!(stdout.contains(RAG_QA_MOCK_CONTEXT), "{stdout}");
}

#[test]
fn rag_qa_bot_rejects_fabricated_grounded_answer() {
    let repo = repo_root();
    let tmp = tempfile::tempdir().expect("tempdir");
    let bad_source = tmp.path().join("fabricated_rag_answer.cor");
    std::fs::write(
        &bad_source,
        r#"
effect llm:
    cost: $0.01

prompt answer_from_memory(question: String) -> String uses llm:
    "Answer without retrieved context: {question}"

agent answer(question: String) -> Grounded<String>:
    return answer_from_memory(question)
"#,
    )
    .expect("write adversarial source");
    let out = Command::new(corvid_bin())
        .arg("check")
        .arg(&bad_source)
        .current_dir(repo)
        .output()
        .expect("run corvid check on adversarial rag source");
    assert!(
        !out.status.success(),
        "fabricated grounded answer should fail:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ungrounded return"), "{stderr}");
    assert!(
        stderr.contains("return value lacks a proven grounded source"),
        "{stderr}"
    );
}
