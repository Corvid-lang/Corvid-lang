use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn personal_executive_agent_root() -> PathBuf {
    repo_root()
        .join("examples")
        .join("backend")
        .join("personal_executive_agent")
}

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

fn execute_sql_dir(conn: &Connection, dir: &Path, expected_migrations: usize) {
    let mut migrations = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read migration dir {}: {err}", dir.display()))
        .map(|entry| entry.expect("migration entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect::<Vec<_>>();
    migrations.sort();

    assert_eq!(migrations.len(), expected_migrations, "migration count");
    for migration in migrations {
        let sql = fs::read_to_string(&migration)
            .unwrap_or_else(|err| panic!("read migration {}: {err}", migration.display()));
        conn.execute_batch(&sql)
            .unwrap_or_else(|err| panic!("execute migration {}: {err}", migration.display()));
    }
}

#[test]
fn release_policy_defines_channels_semver_and_blockers() {
    let policy = fs::read_to_string(repo_root().join("docs").join("release-policy.md"))
        .expect("read release policy");

    for required in [
        "## Channels",
        "### Nightly",
        "### Beta",
        "### Stable",
        "## SemVer Scope",
        "## Stability Classes",
        "## Breaking Change Rules",
        "## Release Blockers",
        "## Key Rotation",
        "## Maintainer Signoff",
        "signed binaries",
        "SHA256SUMS.txt",
        "corvid upgrade --check",
        "corvid claim audit",
    ] {
        assert!(
            policy.contains(required),
            "release policy is missing `{required}`"
        );
    }
}

#[test]
fn personal_executive_agent_data_model_migrations_and_connectors_are_real() {
    let app = personal_executive_agent_root();
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");

    execute_sql_dir(&conn, &app.join("migrations"), 5);

    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name LIKE 'executive_%'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(table_count, 12, "unexpected executive table count");

    let seed_sql = fs::read_to_string(app.join("seeds").join("demo.sql")).expect("read seed sql");
    conn.execute_batch(&seed_sql).expect("execute seed sql");

    let connector_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM executive_connector_accounts",
            [],
            |row| row.get(0),
        )
        .expect("connector rows");
    assert_eq!(connector_rows, 5, "all connector bindings must be seeded");

    let manifest_text = fs::read_to_string(app.join("connectors").join("mock_manifest.json"))
        .expect("read connector manifest");
    let manifest: Value = serde_json::from_str(&manifest_text).expect("parse connector manifest");
    let connectors = manifest["connectors"]
        .as_array()
        .expect("connector list must be an array");
    assert_eq!(connectors.len(), 5, "mock manifest connector count");
    assert!(connectors.iter().all(|connector| {
        connector["approval_required"].as_bool() == Some(true)
            && connector["replay_policy"].as_str() == Some("quarantine_writes")
    }));
}

#[test]
fn personal_executive_agent_inbox_and_draft_mocks_are_approval_gated() {
    let app = personal_executive_agent_root();
    let inbox_text =
        fs::read_to_string(app.join("mocks").join("inbox_threads.json")).expect("read inbox mock");
    let drafts_text =
        fs::read_to_string(app.join("mocks").join("draft_replies.json")).expect("read draft mock");
    let inbox: Value = serde_json::from_str(&inbox_text).expect("parse inbox mock");
    let drafts: Value = serde_json::from_str(&drafts_text).expect("parse draft mock");

    assert_eq!(inbox["mode"].as_str(), Some("mock"));
    assert_eq!(drafts["mode"].as_str(), Some("mock"));

    let threads = inbox["threads"].as_array().expect("threads array");
    let draft_items = drafts["drafts"].as_array().expect("drafts array");
    assert_eq!(threads.len(), 1);
    assert_eq!(draft_items.len(), 1);

    let thread = &threads[0];
    let draft = &draft_items[0];
    assert_eq!(thread["triage_status"].as_str(), Some("needs_draft"));
    assert_eq!(draft["thread_id"], thread["id"]);
    assert_eq!(draft["approval_label"].as_str(), Some("SendFollowUpEmail"));
    assert_eq!(draft["status"].as_str(), Some("approval_pending"));
    assert!(draft["replay_key"]
        .as_str()
        .is_some_and(|key| key.starts_with("replay:draft:")));
}

#[test]
fn personal_executive_agent_calendar_and_durable_job_mocks_are_bounded() {
    let app = personal_executive_agent_root();
    let calendar_text = fs::read_to_string(app.join("mocks").join("calendar_events.json"))
        .expect("read calendar mock");
    let jobs_text =
        fs::read_to_string(app.join("mocks").join("durable_jobs.json")).expect("read jobs mock");
    let calendar: Value = serde_json::from_str(&calendar_text).expect("parse calendar mock");
    let jobs: Value = serde_json::from_str(&jobs_text).expect("parse jobs mock");

    assert_eq!(calendar["mode"].as_str(), Some("mock"));
    let availability = calendar["availability"]
        .as_array()
        .expect("availability array");
    assert_eq!(availability.len(), 1);
    assert_eq!(
        availability[0]["approval_label"].as_str(),
        Some("ScheduleCalendarEvent")
    );
    assert!(availability[0]["confidence"]
        .as_f64()
        .is_some_and(|confidence| confidence >= 0.8));

    assert_eq!(jobs["queue"].as_str(), Some("personal_executive_agent"));
    let job_items = jobs["jobs"].as_array().expect("jobs array");
    assert_eq!(job_items.len(), 4);
    for job in job_items {
        assert!(job["replay_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("executive:")));
        assert!(job["uses"].as_array().is_some_and(|uses| !uses.is_empty()));
    }
    let follow_up = job_items
        .iter()
        .find(|job| job["kind"].as_str() == Some("follow_up"))
        .expect("follow-up job");
    assert_eq!(
        follow_up["approval_label"].as_str(),
        Some("SendExecutiveFollowUp")
    );
}

#[test]
fn personal_executive_agent_external_writes_are_approval_gated_and_auditable() {
    let app = personal_executive_agent_root();
    let source = fs::read_to_string(app.join("src").join("main.cor")).expect("read source");
    let surface_text = fs::read_to_string(app.join("mocks").join("approval_surface.json"))
        .expect("read approval surface");
    let surface: Value = serde_json::from_str(&surface_text).expect("parse approval surface");
    let contracts = surface["contracts"].as_array().expect("contracts array");
    assert_eq!(contracts.len(), 5);

    for contract in contracts {
        let label = contract["label"].as_str().expect("approval label");
        let route = contract["route"].as_str().expect("approval route");
        assert_eq!(contract["audit_required"].as_bool(), Some(true));
        assert!(
            source.contains(&format!("approve {label}("))
                || source.contains(&format!("\"{label}\"")),
            "missing approve or contract for {label}"
        );
        let route_path = route
            .strip_prefix("POST ")
            .expect("approval surface route must be POST");
        assert!(
            source.contains(&format!("route POST \"{route_path}\"")),
            "missing route {route}"
        );
    }

    for dangerous_tool in [
        "send_follow_up_email",
        "edit_calendar_event",
        "edit_task_item",
        "send_chat_message",
    ] {
        assert!(
            source.contains(&format!("tool {dangerous_tool}(")) && source.contains("dangerous"),
            "missing dangerous tool contract for {dangerous_tool}"
        );
    }
}

#[test]
fn personal_executive_agent_hardening_bundle_runs_and_covers_risks() {
    let app = personal_executive_agent_root();
    let eval = app.join("evals").join("hardening_eval.cor");
    let out = Command::new(corvid_bin())
        .arg("eval")
        .arg(&eval)
        .current_dir(repo_root())
        .output()
        .expect("run hardening eval");
    assert!(
        out.status.success(),
        "hardening eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed, 0 failed"), "{stdout}");
    assert!(stdout.contains("values: 10/10 passed"), "{stdout}");

    let adversarial = fs::read_dir(app.join("adversarial"))
        .expect("read adversarial dir")
        .count();
    assert_eq!(adversarial, 5, "expected five adversarial cases");

    let trace = fs::read_to_string(app.join("traces").join("demo.lineage.jsonl"))
        .expect("read trace fixture");
    assert!(trace.contains("\"kind\":\"approval\""));
    assert!(trace.contains("\"redaction_policy_hash\":\"sha256:redacted\""));
    assert!(!trace.contains("DO_NOT_COMMIT_RAW_EMAIL"));

    assert!(app.join("ops").join("runbook.md").exists());
    assert!(app.join("deploy").join("docker-compose.yml").exists());
}

#[test]
fn personal_knowledge_agent_ingestion_is_private_and_provenanced() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_knowledge_agent");
    let source = app.join("src").join("main.cor");
    let out = Command::new(corvid_bin())
        .arg("check")
        .arg(&source)
        .current_dir(repo_root())
        .output()
        .expect("check knowledge app");
    assert!(
        out.status.success(),
        "knowledge app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    execute_sql_dir(&conn, &app.join("migrations"), 3);
    let seed_sql = fs::read_to_string(app.join("seeds").join("demo.sql")).expect("read seed sql");
    conn.execute_batch(&seed_sql).expect("execute seed sql");

    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name LIKE 'knowledge_%'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(table_count, 6);

    let local_only: i64 = conn
        .query_row("SELECT local_only FROM knowledge_embeddings", [], |row| {
            row.get(0)
        })
        .expect("local only");
    assert_eq!(local_only, 1);

    let mock_text =
        fs::read_to_string(app.join("mocks").join("files_index.json")).expect("read files mock");
    let mock: Value = serde_json::from_str(&mock_text).expect("parse files mock");
    assert_eq!(mock["mode"].as_str(), Some("mock"));
    assert_eq!(mock["privacy"]["local_only"].as_bool(), Some(true));
    assert_eq!(mock["privacy"]["raw_text_committed"].as_bool(), Some(false));
    assert!(mock["documents"][0]["provenance_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("files:notes:")));
}

#[test]
fn personal_knowledge_agent_search_answers_are_grounded_and_evaluated() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_knowledge_agent");
    let source = app.join("src").join("main.cor");
    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(&source)
        .current_dir(repo_root())
        .output()
        .expect("check knowledge app");
    assert!(
        check.status.success(),
        "knowledge app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let eval = app.join("evals").join("search_answer_eval.cor");
    let eval_out = Command::new(corvid_bin())
        .arg("eval")
        .arg(&eval)
        .current_dir(repo_root())
        .output()
        .expect("run knowledge eval");
    assert!(
        eval_out.status.success(),
        "knowledge eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&eval_out.stdout),
        String::from_utf8_lossy(&eval_out.stderr)
    );
    let eval_stdout = String::from_utf8_lossy(&eval_out.stdout);
    assert!(eval_stdout.contains("values: 5/5 passed"), "{eval_stdout}");

    let search_text = fs::read_to_string(app.join("mocks").join("search_results.json"))
        .expect("read search mock");
    let search: Value = serde_json::from_str(&search_text).expect("parse search mock");
    assert_eq!(search["local_only"].as_bool(), Some(true));
    assert_eq!(search["hits"][0]["grounded"].as_bool(), Some(true));
    assert!(search["hits"][0]["citation"]["provenance_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("files:notes:")));
    assert_eq!(search["answer"]["citation_count"].as_i64(), Some(1));

    let trace = fs::read_to_string(app.join("traces").join("demo.lineage.jsonl"))
        .expect("read knowledge trace");
    assert!(trace.contains("\"guarantee_id\":\"grounded-search\""));
    assert!(trace.contains("\"guarantee_id\":\"provenance-answer\""));
    assert!(!trace.contains("raw document"));
}

#[test]
fn finance_operations_agent_readonly_snapshot_is_non_advice() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("finance_operations_agent");
    let source = app.join("src").join("main.cor");
    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(&source)
        .current_dir(repo_root())
        .output()
        .expect("check finance app");
    assert!(
        check.status.success(),
        "finance app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    execute_sql_dir(&conn, &app.join("migrations"), 2);
    let seed_sql = fs::read_to_string(app.join("seeds").join("demo.sql")).expect("read seed sql");
    conn.execute_batch(&seed_sql).expect("execute seed sql");

    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name LIKE 'finance_%'",
            [],
            |row| row.get(0),
        )
        .expect("table count");
    assert_eq!(table_count, 7);

    let snapshot_text = fs::read_to_string(app.join("mocks").join("readonly_snapshot.json"))
        .expect("read finance mock");
    let snapshot: Value = serde_json::from_str(&snapshot_text).expect("parse finance mock");
    assert_eq!(snapshot["readonly"].as_bool(), Some(true));
    assert_eq!(snapshot["regulated_advice"].as_bool(), Some(false));
    assert_eq!(snapshot["accounts"].as_array().expect("accounts").len(), 1);
    assert!(snapshot["anomalies"][0]["confidence"]
        .as_f64()
        .is_some_and(|confidence| confidence >= 0.8));
}

#[test]
fn finance_operations_agent_payment_intents_are_approval_gated_and_audited() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("finance_operations_agent");
    let source = fs::read_to_string(app.join("src").join("main.cor")).expect("read finance source");
    assert!(source.contains("tool submit_payment_intent("));
    assert!(source.contains("dangerous uses payment_write"));
    assert!(source.contains("approve SubmitPaymentIntent("));
    assert!(source.contains("route POST \"/payments/intents/submit\""));

    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(app.join("src").join("main.cor"))
        .current_dir(repo_root())
        .output()
        .expect("check finance app");
    assert!(
        check.status.success(),
        "finance app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let payment_text = fs::read_to_string(app.join("mocks").join("payment_intents.json"))
        .expect("read payment intents mock");
    let payment: Value = serde_json::from_str(&payment_text).expect("parse payment mock");
    assert_eq!(payment["regulated_advice"].as_bool(), Some(false));
    assert_eq!(
        payment["payment_execution"].as_str(),
        Some("approval_required")
    );
    assert_eq!(
        payment["intents"][0]["approval_label"].as_str(),
        Some("SubmitPaymentIntent")
    );
    assert_eq!(payment["audit"][0]["redacted"].as_bool(), Some(true));

    let eval = app.join("evals").join("payment_audit_eval.cor");
    let eval_out = Command::new(corvid_bin())
        .arg("eval")
        .arg(&eval)
        .current_dir(repo_root())
        .output()
        .expect("run finance eval");
    assert!(
        eval_out.status.success(),
        "finance eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&eval_out.stdout),
        String::from_utf8_lossy(&eval_out.stderr)
    );
}

#[test]
fn customer_support_agent_triage_and_drafts_are_policy_grounded() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("customer_support_agent");
    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(app.join("src").join("main.cor"))
        .current_dir(repo_root())
        .output()
        .expect("check support app");
    assert!(
        check.status.success(),
        "support app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    execute_sql_dir(&conn, &app.join("migrations"), 2);
    let seed_sql = fs::read_to_string(app.join("seeds").join("demo.sql")).expect("read seed sql");
    conn.execute_batch(&seed_sql).expect("execute seed sql");

    let mock_text =
        fs::read_to_string(app.join("mocks").join("triage_reply.json")).expect("read support mock");
    let mock: Value = serde_json::from_str(&mock_text).expect("parse support mock");
    assert_eq!(mock["triage"]["grounded"].as_bool(), Some(true));
    assert_eq!(
        mock["draft"]["approval_label"].as_str(),
        Some("SendSupportReply")
    );
    assert!(mock["citation"]["provenance_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("policy:")));
}

#[test]
fn customer_support_agent_approvals_sla_and_eval_dashboard_work() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("customer_support_agent");
    let source = fs::read_to_string(app.join("src").join("main.cor")).expect("read support source");
    assert!(source.contains("approve SendSupportReply("));
    assert!(source.contains("approve IssueSupportRefund("));
    assert!(source.contains("tool send_support_reply("));
    assert!(source.contains("tool issue_support_refund("));

    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(app.join("src").join("main.cor"))
        .current_dir(repo_root())
        .output()
        .expect("check support app");
    assert!(
        check.status.success(),
        "support app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let mock_text = fs::read_to_string(app.join("mocks").join("approvals_sla.json"))
        .expect("read support approval mock");
    let mock: Value = serde_json::from_str(&mock_text).expect("parse support approval mock");
    assert_eq!(mock["approvals"].as_array().expect("approvals").len(), 2);
    assert_eq!(
        mock["sla_jobs"][0]["replay_key"].as_str(),
        Some("support:sla:ticket-1")
    );
    assert_eq!(mock["eval_dashboard"]["eval_passes"].as_i64(), Some(4));

    let eval_out = Command::new(corvid_bin())
        .arg("eval")
        .arg(app.join("evals").join("support_ops_eval.cor"))
        .current_dir(repo_root())
        .output()
        .expect("run support eval");
    assert!(
        eval_out.status.success(),
        "support eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&eval_out.stdout),
        String::from_utf8_lossy(&eval_out.stderr)
    );
}

#[test]
fn code_maintenance_agent_ingests_and_triages_ci_aware_risk() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("code_maintenance_agent");
    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(app.join("src").join("main.cor"))
        .current_dir(repo_root())
        .output()
        .expect("check code app");
    assert!(
        check.status.success(),
        "code app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    execute_sql_dir(&conn, &app.join("migrations"), 2);
    let seed_sql = fs::read_to_string(app.join("seeds").join("demo.sql")).expect("read seed sql");
    conn.execute_batch(&seed_sql).expect("execute seed sql");

    let mock_text =
        fs::read_to_string(app.join("mocks").join("triage.json")).expect("read code mock");
    let mock: Value = serde_json::from_str(&mock_text).expect("parse code mock");
    assert_eq!(mock["ci"]["status"].as_str(), Some("failed"));
    assert_eq!(mock["risk"]["severity"].as_str(), Some("high"));
    assert!(mock["risk"]["confidence"]
        .as_f64()
        .is_some_and(|confidence| confidence >= 0.8));
}

#[test]
fn code_maintenance_agent_write_actions_require_approval() {
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("code_maintenance_agent");
    let source = fs::read_to_string(app.join("src").join("main.cor")).expect("read code source");
    assert!(source.contains("approve PostReviewComment("));
    assert!(source.contains("approve CreatePatchProposal("));
    assert!(source.contains("tool post_review_comment("));
    assert!(source.contains("tool create_patch_proposal("));

    let check = Command::new(corvid_bin())
        .arg("check")
        .arg(app.join("src").join("main.cor"))
        .current_dir(repo_root())
        .output()
        .expect("check code app");
    assert!(
        check.status.success(),
        "code app check failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let mock_text =
        fs::read_to_string(app.join("mocks").join("write_plan.json")).expect("read write mock");
    let mock: Value = serde_json::from_str(&mock_text).expect("parse write mock");
    assert_eq!(mock["writes_gated"].as_bool(), Some(true));
    assert_eq!(mock["approvals"].as_array().expect("approvals").len(), 2);
    assert_eq!(mock["plan"]["approval_count"].as_i64(), Some(2));

    let eval_out = Command::new(corvid_bin())
        .arg("eval")
        .arg(app.join("evals").join("write_approval_eval.cor"))
        .current_dir(repo_root())
        .output()
        .expect("run code eval");
    assert!(
        eval_out.status.success(),
        "code eval failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&eval_out.stdout),
        String::from_utf8_lossy(&eval_out.stderr)
    );
}

#[test]
fn phase_42_apps_have_hardening_pack_artifacts() {
    let apps = [
        "shared_app_template",
        "personal_executive_agent",
        "personal_knowledge_agent",
        "finance_operations_agent",
        "customer_support_agent",
        "code_maintenance_agent",
    ];
    for app_name in apps {
        let app = repo_root().join("examples").join("backend").join(app_name);
        assert!(app.join("seeds").exists(), "{app_name} missing seeds");
        assert!(app.join("mocks").exists(), "{app_name} missing mocks");
        assert!(
            app.join("traces").exists(),
            "{app_name} missing replay traces"
        );
        assert!(
            app.join("adversarial").exists(),
            "{app_name} missing adversarial cases"
        );
        assert!(
            app.join("deploy").join("env.example").exists(),
            "{app_name} missing env docs"
        );
        assert!(
            app.join("security-model.md").exists(),
            "{app_name} missing security model"
        );
        assert!(
            app.join("ops").join("runbook.md").exists(),
            "{app_name} missing runbook"
        );
    }
}

#[test]
fn phase_42_external_trial_packet_is_ready_but_not_faked() {
    let docs = repo_root().join("docs").join("external-trials");
    let trial = fs::read_to_string(docs.join("phase-42-trial-one.md")).expect("read trial packet");
    let triage =
        fs::read_to_string(docs.join("phase-42-feedback-triage.md")).expect("read triage packet");
    assert!(trial.contains("Pending real external reviewer"));
    assert!(trial.contains("Do not mark 42I1 or 42I2 complete"));
    assert!(triage.contains("No external reviewer signoff has been recorded yet"));
}

#[test]
fn phase_43_market_readiness_brief_defines_launch_gates() {
    let brief = fs::read_to_string(
        repo_root()
            .join("docs")
            .join("phase-43-market-readiness.md"),
    )
    .expect("read phase 43 brief");
    for required in [
        "Launch Gates",
        "Release Channels",
        "Support Posture",
        "Security Process",
        "Beta Criteria",
        "Non-Scope",
    ] {
        assert!(brief.contains(required), "missing {required}");
    }
    assert!(brief.contains("Final claims have runnable evidence or are removed"));
}

#[test]
fn deploy_package_emits_dockerfile_and_oci_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_executive_agent");
    let out = temp.path().join("package");
    let deploy = Command::new(corvid_bin())
        .arg("deploy")
        .arg("package")
        .arg(&app)
        .arg("--out")
        .arg(&out)
        .env(
            "CORVID_DEPLOY_SIGNING_KEY",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .current_dir(repo_root())
        .output()
        .expect("run deploy package");
    assert!(
        deploy.status.success(),
        "deploy package failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&deploy.stdout),
        String::from_utf8_lossy(&deploy.stderr)
    );

    let dockerfile = fs::read_to_string(out.join("Dockerfile")).expect("read Dockerfile");
    assert!(dockerfile.contains("FROM rust:1.78-slim AS build"));
    assert!(dockerfile.contains("HEALTHCHECK"));
    assert!(dockerfile.contains("personal_executive_agent"));

    let metadata_text = fs::read_to_string(out.join("oci-labels.json")).expect("read oci metadata");
    let metadata: Value = serde_json::from_str(&metadata_text).expect("parse oci metadata");
    assert_eq!(metadata["image"].as_str(), Some("personal_executive_agent"));
    assert!(metadata["labels"]["org.opencontainers.image.source"]
        .as_str()
        .is_some_and(
            |source| source.ends_with("src\\main.cor") || source.ends_with("src/main.cor")
        ));
    assert!(metadata["labels"]["dev.corvid.package.source_sha256"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64));

    let env_schema = fs::read_to_string(out.join("env.schema.json")).expect("read env schema");
    assert!(env_schema.contains("CORVID_DATABASE_URL"));
    assert!(env_schema.contains("CORVID_REQUIRE_APPROVALS"));
    let health = fs::read_to_string(out.join("health.json")).expect("read health config");
    assert!(health.contains("/healthz"));
    assert!(health.contains("/readyz"));
    let migrate = fs::read_to_string(out.join("migrate.sh")).expect("read migration runner");
    assert!(migrate.contains("corvid migrate up"));
    let startup = fs::read_to_string(out.join("startup-checks.md")).expect("read startup checks");
    assert!(startup.contains("CORVID_TRACE_DIR"));

    let attestation =
        fs::read_to_string(out.join("build-attestation.dsse.json")).expect("read attestation");
    let envelope: Value = serde_json::from_str(&attestation).expect("parse attestation");
    assert_eq!(
        envelope["payloadType"].as_str(),
        Some("application/vnd.corvid.deploy.attestation.v1+json")
    );
    assert_eq!(
        envelope["signatures"][0]["keyid"].as_str(),
        Some("deploy-package")
    );
    let verify = fs::read_to_string(out.join("VERIFY.md")).expect("read verify docs");
    assert!(verify.contains("DSSE envelope"));
}

#[test]
fn deploy_compose_emits_reference_app_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_executive_agent");
    let out = temp.path().join("compose");
    let deploy = Command::new(corvid_bin())
        .arg("deploy")
        .arg("compose")
        .arg(&app)
        .arg("--out")
        .arg(&out)
        .current_dir(repo_root())
        .output()
        .expect("run deploy compose");
    assert!(
        deploy.status.success(),
        "deploy compose failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&deploy.stdout),
        String::from_utf8_lossy(&deploy.stderr)
    );
    let compose =
        fs::read_to_string(out.join("docker-compose.yml")).expect("read compose manifest");
    assert!(compose.contains("personal_executive_agent"));
    assert!(compose.contains("CORVID_REQUIRE_APPROVALS"));
    assert!(compose.contains("healthcheck"));
    let env = fs::read_to_string(out.join(".env.example")).expect("read compose env");
    assert!(env.contains("CORVID_CONNECTOR_MODE=mock"));
}

#[test]
fn deploy_paas_emits_single_service_manifests() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_executive_agent");
    let out = temp.path().join("paas");
    let deploy = Command::new(corvid_bin())
        .arg("deploy")
        .arg("paas")
        .arg(&app)
        .arg("--out")
        .arg(&out)
        .current_dir(repo_root())
        .output()
        .expect("run deploy paas");
    assert!(
        deploy.status.success(),
        "deploy paas failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&deploy.stdout),
        String::from_utf8_lossy(&deploy.stderr)
    );
    let fly = fs::read_to_string(out.join("fly.toml")).expect("read fly manifest");
    assert!(fly.contains("personal_executive_agent"));
    assert!(fly.contains("path = \"/healthz\""));
    let render = fs::read_to_string(out.join("render.yaml")).expect("read render manifest");
    assert!(render.contains("healthCheckPath: /healthz"));
    assert!(render.contains("CORVID_REQUIRE_APPROVALS"));
    let secrets = fs::read_to_string(out.join("secrets.example")).expect("read secrets template");
    assert!(secrets.contains("CORVID_DATABASE_URL"));
}

#[test]
fn deploy_k8s_and_systemd_emit_operable_manifests() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = repo_root()
        .join("examples")
        .join("backend")
        .join("personal_executive_agent");

    let k8s_out = temp.path().join("k8s");
    let k8s = Command::new(corvid_bin())
        .arg("deploy")
        .arg("k8s")
        .arg(&app)
        .arg("--out")
        .arg(&k8s_out)
        .current_dir(repo_root())
        .output()
        .expect("run deploy k8s");
    assert!(
        k8s.status.success(),
        "deploy k8s failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&k8s.stdout),
        String::from_utf8_lossy(&k8s.stderr)
    );
    let k8s_yaml = fs::read_to_string(k8s_out.join("deployment.yaml")).expect("read k8s yaml");
    assert!(k8s_yaml.contains("kind: Deployment"));
    assert!(k8s_yaml.contains("readinessProbe"));
    assert!(k8s_yaml.contains("path: /healthz"));

    let systemd_out = temp.path().join("systemd");
    let systemd = Command::new(corvid_bin())
        .arg("deploy")
        .arg("systemd")
        .arg(&app)
        .arg("--out")
        .arg(&systemd_out)
        .current_dir(repo_root())
        .output()
        .expect("run deploy systemd");
    assert!(
        systemd.status.success(),
        "deploy systemd failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&systemd.stdout),
        String::from_utf8_lossy(&systemd.stderr)
    );
    let service = fs::read_to_string(systemd_out.join("personal_executive_agent.service"))
        .expect("read systemd service");
    assert!(service.contains("ExecStart=/usr/local/bin/corvid run"));
    assert!(service.contains("Restart=on-failure"));
    assert!(systemd_out
        .join("personal_executive_agent.sysusers")
        .exists());
    assert!(systemd_out
        .join("personal_executive_agent.tmpfiles")
        .exists());
}
