use corvid_driver::compile_to_ir_with_config_at_path;
use std::fs;

#[test]
fn std_ai_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("ai.cor");
    let source = fs::read_to_string(&source_path).expect("std/ai.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.ai should compile as a standalone Corvid module");
}

#[test]
fn std_ai_imported_helpers_typecheck() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("std")).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    fs::copy(repo.join("std").join("ai.cor"), dir.path().join("std").join("ai.cor")).unwrap();
    fs::copy(
        repo.join("std").join("effects.cor"),
        dir.path().join("std").join("effects.cor"),
    )
    .unwrap();

    let main_path = dir.path().join("main.cor");
    let source = r#"
import "./std/ai" use AiMessage, AiSession, user_message, start_session, next_turn, tool_ok, confidence, render_prompt_pair, render_message, rendered_prompt

agent main() -> String:
    msg = user_message("hello")
    sess = start_session("s1", "demo")
    next = next_turn(sess)
    envelope = tool_ok("lookup", msg.content)
    conf = confidence(0.8, 0.5)
    prompt_line = render_prompt_pair("query", msg.content)
    rendered = rendered_prompt("search", render_message(msg))
    if conf.accepted:
        return envelope.value + " " + prompt_line + " " + rendered.template_name
    else:
        return next.title
"#;
    fs::write(&main_path, source).unwrap();

    compile_to_ir_with_config_at_path(source, &main_path, None)
        .expect("program importing std.ai helpers should compile");
}

#[test]
fn std_http_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("http.cor");
    let source = fs::read_to_string(&source_path).expect("std/http.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.http should compile as a standalone Corvid module");
}

#[test]
fn std_io_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("io.cor");
    let source = fs::read_to_string(&source_path).expect("std/io.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.io should compile as a standalone Corvid module");
}

#[test]
fn std_secrets_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("secrets.cor");
    let source = fs::read_to_string(&source_path).expect("std/secrets.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.secrets should compile as a standalone Corvid module");
}

#[test]
fn std_observe_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("observe.cor");
    let source = fs::read_to_string(&source_path).expect("std/observe.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.observe should compile as a standalone Corvid module");
}

#[test]
fn std_cache_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("cache.cor");
    let source = fs::read_to_string(&source_path).expect("std/cache.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.cache should compile as a standalone Corvid module");
}

#[test]
fn std_queue_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("queue.cor");
    let source = fs::read_to_string(&source_path).expect("std/queue.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.queue should compile as a standalone Corvid module");
}

#[test]
fn std_agent_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("agent.cor");
    let source = fs::read_to_string(&source_path).expect("std/agent.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.agent should compile as a standalone Corvid module");
}

#[test]
fn std_rag_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("rag.cor");
    let source = fs::read_to_string(&source_path).expect("std/rag.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.rag should compile as a standalone Corvid module");
}

#[test]
fn std_effects_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("effects.cor");
    let source = fs::read_to_string(&source_path).expect("std/effects.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.effects should compile as a standalone Corvid module");
}

#[test]
fn std_db_compiles_as_corvid_source() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("db.cor");
    let source = fs::read_to_string(&source_path).expect("std/db.cor");

    compile_to_ir_with_config_at_path(&source, &source_path, None)
        .expect("std.db should compile as a standalone Corvid module");
}

#[test]
fn std_db_token_surface_does_not_expose_raw_token_values() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let source_path = repo.join("std").join("db.cor");
    let source = fs::read_to_string(&source_path).expect("std/db.cor");

    for forbidden in ["access_token", "refresh_token", "raw_token", "token_value"] {
        assert!(
            !source.contains(forbidden),
            "std.db token storage surface must not expose raw token field `{forbidden}`"
        );
    }
    assert!(source.contains("redacted"), "token surface must carry redaction metadata");
    assert!(
        source.contains("ciphertext_hash"),
        "encrypted token surface must expose only ciphertext hash metadata"
    );
}

// ---------------------------------------------------------------
// Imported-helpers typecheck tests. Each exercises the full
// public surface of one std/*.cor module from a user-side
// `main.cor` and asserts the program goes through lex / parse /
// resolve / typecheck / IR lowering cleanly. Catches the failure
// mode where a stdlib module's exported types or agent
// signatures drift away from what user code expects.
// ---------------------------------------------------------------

/// Stage `std/<name>.cor` (and optionally its `std/effects.cor`
/// transitive dep) into a tempdir and compile a user-side
/// `main.cor` against it. Asserts the IR pipeline succeeds.
fn assert_imported_helpers_typecheck(module_name: &str, with_effects_dep: bool, main_source: &str) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("std")).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    let module_filename = format!("{module_name}.cor");
    fs::copy(
        repo.join("std").join(&module_filename),
        dir.path().join("std").join(&module_filename),
    )
    .unwrap_or_else(|e| panic!("copy std/{module_filename}: {e}"));
    if with_effects_dep {
        fs::copy(
            repo.join("std").join("effects.cor"),
            dir.path().join("std").join("effects.cor"),
        )
        .unwrap_or_else(|e| panic!("copy std/effects.cor: {e}"));
    }
    let main_path = dir.path().join("main.cor");
    fs::write(&main_path, main_source).unwrap();
    compile_to_ir_with_config_at_path(main_source, &main_path, None).unwrap_or_else(|errors| {
        panic!(
            "program importing std.{module_name} helpers should compile, got: {errors:?}"
        )
    });
}

#[test]
fn std_http_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "http",
        true,
        r#"
import "./std/http" use HttpHeader, HttpRequestEnvelope, HttpResponseEnvelope, http_get, http_post_json, http_with_retry, http_with_timeout, http_ok

agent main() -> Bool:
    req = http_get("https://api.example.com/v1/health")
    posted = http_post_json("https://api.example.com/v1/widgets", "{}")
    retried = http_with_retry(req, 3)
    bounded = http_with_timeout(retried, 5000)
    response = HttpResponseEnvelope(200, "ok", 1, 12, bounded.effect_meta)
    header = HttpHeader("X-Trace-Id", "abc123")
    return http_ok(response) and header.name != "" and posted.method_name == "POST"
"#,
    );
}

#[test]
fn std_io_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "io",
        true,
        r#"
import "./std/io" use PathInfo, FileReadEnvelope, FileWriteEnvelope, DirectoryEntryEnvelope, path, file_read, file_write, directory_entry

agent main() -> Bool:
    p = path("/tmp/data.txt")
    r = file_read(p.value, "hello", 5)
    w = file_write(p.value, 5)
    entry = directory_entry(p.value, "data.txt", false)
    return r.bytes == 5 and w.bytes == 5 and not entry.is_dir
"#,
    );
}

#[test]
fn std_secrets_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "secrets",
        true,
        r#"
import "./std/secrets" use SecretReadEnvelope, secret_present, secret_missing

agent main() -> Bool:
    have = secret_present("ANTHROPIC_API_KEY")
    miss = secret_missing("UNSET_KEY")
    return have.present and not miss.present
"#,
    );
}

#[test]
fn std_observe_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "observe",
        true,
        r#"
import "./std/observe" use MetricCounter, CostCounter, LatencyHistogram, RoutingDecision, ApprovalSummary, RuntimeObservationSummary, metric_counter, cost_counter, latency_histogram, routing_decision, approval_summary, runtime_summary

agent main() -> Bool:
    m = metric_counter("requests", 1.0, "count")
    c = cost_counter(1, 100, 50, 150, 0.01)
    l = latency_histogram("api", 100, 200, 300)
    r = routing_decision("classify", "fast", "deep", "low_confidence")
    a = approval_summary("IssueRefund", 5, 1)
    s = runtime_summary(10, 2, 1500, 0.05, 3, 0)
    return m.value == 1.0 and c.total_tokens == 150 and l.p99_ms == 300 and r.to_model == "deep" and a.approved == 5 and s.cost_usd > 0.0
"#,
    );
}

#[test]
fn std_cache_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "cache",
        true,
        r#"
import "./std/cache" use CacheKeyEnvelope, CacheEntryEnvelope, cache_key, cache_entry, cache_hit

agent main() -> Bool:
    k = cache_key("answers", "weather", "fp-1", "std.http.request", "prov-1")
    entry = cache_entry(k, true, "weather:fp-1")
    return cache_hit(entry)
"#,
    );
}

#[test]
fn std_queue_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "queue",
        true,
        r#"
import "./std/queue" use QueueJobEnvelope, queued_job, pending_job, canceled_job

agent main() -> Bool:
    pending = pending_job("job-1", "summarize", 3, 0.50, "std.ai", "std.queue.job")
    queued = queued_job("job-2", "extract", "running", 5, 1.00, "std.ai", "std.queue.job")
    cancel = canceled_job(queued)
    return pending.status == "pending" and queued.status == "running" and cancel.status == "canceled"
"#,
    );
}

#[test]
fn std_db_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "db",
        true,
        r#"
import "./std/db" use DbConnection, DbParam, DbQuery, DbResult, DbError, DbColumn, DbRowDecode, DbTransaction, DbAuditRecord, DbAuditWrite, DbTokenRef, DbEncryptedToken, DbMigrationStatus, DbEffectTag, DbReplaySummary, sqlite_open, postgres_open, db_param, db_query, db_execute, db_result, db_error, db_parameterized, db_column, db_decode_ok, db_decode_missing_column, db_decode_wrong_kind, db_transaction, db_transaction_commit, db_transaction_rollback, db_transaction_nested_rejected, db_audit_record, db_audit_approved, db_audit_write, db_audit_write_safe, db_token_ref, db_encrypted_token, db_token_redacted, db_migration_status, db_migration_clean, db_effect_tag, db_effect_is_write, db_replay_summary, db_replay_redacted

agent main() -> Bool:
    db = sqlite_open("file:app.db", "db:app")
    pg = postgres_open("postgres://localhost/app", "db:pg")
    id = db_param("id", "String", false)
    read = db_query("select id from users where id = ?", 1, "db:users:read")
    write = db_execute("insert into users (id) values (?)", 1, "db:users:write")
    result = db_result(1, 0, "db:users:write")
    err = db_error("users.find", "no such table")
    col = db_column("id", "String", "String", true)
    ok = db_decode_ok("User")
    missing = db_decode_missing_column("User", "email", "String")
    wrong = db_decode_wrong_kind("User", "age", "Int", "String")
    tx = db_transaction("tx-1", "open", false, "db:tx:1")
    committed = db_transaction_commit(tx)
    rolled_back = db_transaction_rollback(tx)
    nested = db_transaction("tx-2", "rejected", true, "db:tx:2")
    audit = db_audit_record("user-1", "refund.requested", "order-1", "/refunds", "job-1", "approve_refund", "prompt-v1", "model-a", "issue_refund", "approved", 0.05, "trace-1", "replay-1")
    audit_write = db_audit_write(audit, true, true)
    token = db_token_ref("gmail", "acct-1", "tok-1", "key-1", "replay-token-1")
    encrypted = db_encrypted_token(token, "sha256:ciphertext", "xchacha20poly1305")
    pg_migration = db_migration_status("postgres", "0001_init.sql", "sha256:abc", "applied", false, "db:pg:migrate")
    write_tag = db_effect_tag("write", "db:effect:write")
    summary = db_replay_summary("write", "postgres", "sha256:query", 0, 1, "db:replay:1")
    return db.driver == "sqlite" and pg.driver == "postgres" and id.name == "id" and db_parameterized(read) and write.operation == "write" and result.rows_affected == 1 and err.redacted and col.present and ok.ok and not missing.ok and wrong.received_kind == "String" and committed.status == "committed" and rolled_back.status == "rolled_back" and db_transaction_nested_rejected(nested) and db_audit_approved(audit) and db_audit_write_safe(audit_write) and db_token_redacted(token) and encrypted.key_id == "key-1" and db_migration_clean(pg_migration) and db_effect_is_write(write_tag) and db_replay_redacted(summary)
"#,
    );
}

#[test]
fn backend_audit_log_example_typechecks() {
    let dir = tempfile::tempdir().unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repo root");
    fs::create_dir_all(dir.path().join("std")).unwrap();
    fs::copy(repo.join("std").join("db.cor"), dir.path().join("std").join("db.cor")).unwrap();
    fs::copy(
        repo.join("std").join("effects.cor"),
        dir.path().join("std").join("effects.cor"),
    )
    .unwrap();
    let source_path = repo
        .join("examples")
        .join("backend")
        .join("audit_log")
        .join("src")
        .join("main.cor");
    let staged_path = dir.path().join("main.cor");
    fs::copy(&source_path, &staged_path).unwrap();
    let source = fs::read_to_string(&staged_path).expect("audit example");

    compile_to_ir_with_config_at_path(&source, &staged_path, None)
        .expect("backend audit log example should compile");
}

#[test]
fn std_agent_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "agent",
        true,
        r#"
import "./std/agent" use Classification, Extraction, Ranking, Judgement, PlanStep, ToolUse, Critique, Summary, RouteDecision, ActionRequest, ReviewVerdict, ToolLoopTurn, AnswerWithProvenance, classify, extraction_ok, extraction_error, rank, judge, plan_step, tool_use, critique, summarize, route_decision, action_request, review_verdict, tool_loop_turn, answer_with_provenance

agent main() -> Bool:
    c = classify("positive", 0.9, 0.7)
    ok = extraction_ok("Address", "{...}")
    bad = extraction_error("Address", "missing zip")
    rk = rank("doc-1", 0.85, "topic match")
    j = judge("answer-A", "more grounded", false)
    p = plan_step("step-1", "fetch context", "std.rag.search")
    t = tool_use("issue_refund", "ord_1", "IssueRefund", true)
    cr = critique(true, "well grounded", "")
    s = summarize("body", "concise", "operator")
    rd = route_decision("escalate", "uncertain", true)
    ar = action_request("send", "to=user", "Send", true)
    rv = review_verdict(true, "minor", "fine")
    tl = tool_loop_turn("t-1", "consider options", "search", "rank", false)
    awp = answer_with_provenance("yes", "doc-1#chunk-2")
    return c.accepted and ok.valid and not bad.valid and rk.score > 0.0 and not j.needs_human and p.id == "step-1" and t.approved and cr.accepted and s.style == "concise" and rd.needs_human and ar.ready and rv.accepted and not tl.done and awp.grounded
"#,
    );
}

#[test]
fn std_rag_imported_helpers_typecheck() {
    assert_imported_helpers_typecheck(
        "rag",
        true,
        r#"
import "./std/rag" use RagDocumentEnvelope, RagChunkEnvelope, RetrievedRagChunkEnvelope, EmbedderEnvelope, rag_document, rag_chunk, retrieved_chunk, openai_embedder, ollama_embedder, chunk_is_grounded

agent main() -> Bool:
    doc = rag_document("doc-1", "manual.pdf", "application/pdf", "page text")
    chunk = rag_chunk(doc.id, "chunk-1", doc.source, "page text", 0, 9, "manual.pdf#1")
    retrieved = retrieved_chunk(chunk, "manual")
    openai = openai_embedder("text-embedding-3-small")
    ollama = ollama_embedder("nomic-embed-text", "http://localhost:11434")
    return chunk_is_grounded(chunk) and retrieved.grounded and openai.provider == "openai" and ollama.provider == "ollama"
"#,
    );
}

#[test]
fn std_effects_imported_helpers_typecheck() {
    // std/effects.cor has no transitive deps — it's the leaf
    // module every other std/* sits on top of.
    assert_imported_helpers_typecheck(
        "effects",
        false,
        r#"
import "./std/effects" use EffectTag, EffectBudget, EffectEnvelope, effect_tag, effect_budget, effect_envelope, replay_safe, approval_required

agent main() -> Bool:
    tag = effect_tag("std.http.request", "network", "untrusted", "public", "stable")
    budget = effect_budget(0.10, 5000, 1000)
    safe_env = effect_envelope("std.http.request", "prov-1", "", "fp-1", "std.http.request")
    gated_env = effect_envelope("std.refund.issue", "prov-2", "IssueRefund", "fp-2", "std.refund.issue")
    return replay_safe(safe_env) and approval_required(gated_env) and tag.name != "" and budget.cost_usd > 0.0
"#,
    );
}
