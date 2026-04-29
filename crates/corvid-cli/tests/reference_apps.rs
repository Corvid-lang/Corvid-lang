use std::fs;
use std::path::{Path, PathBuf};

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

fn execute_sql_dir(conn: &Connection, dir: &Path) {
    let mut migrations = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read migration dir {}: {err}", dir.display()))
        .map(|entry| entry.expect("migration entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect::<Vec<_>>();
    migrations.sort();

    assert_eq!(
        migrations.len(),
        5,
        "Personal Executive Agent must keep five app migrations"
    );
    for migration in migrations {
        let sql = fs::read_to_string(&migration)
            .unwrap_or_else(|err| panic!("read migration {}: {err}", migration.display()));
        conn.execute_batch(&sql)
            .unwrap_or_else(|err| panic!("execute migration {}: {err}", migration.display()));
    }
}

#[test]
fn personal_executive_agent_data_model_migrations_and_connectors_are_real() {
    let app = personal_executive_agent_root();
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");

    execute_sql_dir(&conn, &app.join("migrations"));

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
