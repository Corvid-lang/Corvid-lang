use crate::errors::RuntimeError;
use rusqlite::{params_from_iter, Connection};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Integer(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

impl DbValue {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "Null",
            Self::Integer(_) => "Int",
            Self::Float(_) => "Float",
            Self::Text(_) => "String",
            Self::Bool(_) => "Bool",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DbCell {
    pub kind: String,
    pub value: DbValue,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DbQueryRows {
    pub rows: Vec<BTreeMap<String, DbCell>>,
    pub row_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbExecuteResult {
    pub rows_affected: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbDecodeError {
    pub field_path: String,
    pub expected_type: String,
    pub received_kind: String,
    pub message: String,
}

pub struct SqliteDbRuntime {
    conn: Mutex<Connection>,
}

impl SqliteDbRuntime {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let conn = Connection::open(path.as_ref()).map_err(|err| {
            RuntimeError::Other(format!(
                "std.db sqlite open failed for `{}`: {err}",
                path.as_ref().display()
            ))
        })?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory()
            .map_err(|err| RuntimeError::Other(format!("std.db sqlite open failed: {err}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn execute(&self, sql: &str, params: &[DbValue]) -> Result<DbExecuteResult, RuntimeError> {
        let sql_params = params.iter().map(db_value_to_sql_value).collect::<Vec<_>>();
        let rows_affected = self
            .conn
            .lock()
            .unwrap()
            .execute(sql, params_from_iter(sql_params))
            .map_err(redacted_sql_error)?;
        Ok(DbExecuteResult {
            rows_affected: rows_affected as u64,
        })
    }

    pub fn execute_batch_transaction(&self, statements: &[&str]) -> Result<DbExecuteResult, RuntimeError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(redacted_sql_error)?;
        let mut rows_affected = 0_u64;
        for statement in statements {
            rows_affected = rows_affected.saturating_add(
                tx.execute(statement, [])
                    .map_err(redacted_sql_error)? as u64,
            );
        }
        tx.commit().map_err(redacted_sql_error)?;
        Ok(DbExecuteResult { rows_affected })
    }

    pub fn query(&self, sql: &str, params: &[DbValue]) -> Result<DbQueryRows, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let sql_params = params.iter().map(db_value_to_sql_value).collect::<Vec<_>>();
        let mut stmt = conn.prepare(sql).map_err(redacted_sql_error)?;
        let column_names = stmt
            .column_names()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let rows = stmt
            .query_map(params_from_iter(sql_params), |row| {
                let mut cells = BTreeMap::new();
                for (index, name) in column_names.iter().enumerate() {
                    let value = row.get_ref(index)?;
                    let db_value = db_value_from_ref(value);
                    cells.insert(
                        name.clone(),
                        DbCell {
                            kind: db_value.kind().to_string(),
                            value: db_value,
                            redacted: false,
                        },
                    );
                }
                Ok(cells)
            })
            .map_err(redacted_sql_error)?;
        let mut collected = Vec::new();
        for row in rows {
            collected.push(row.map_err(redacted_sql_error)?);
        }
        Ok(DbQueryRows {
            row_count: collected.len(),
            rows: collected,
        })
    }
}

pub fn decode_string(
    row: &BTreeMap<String, DbCell>,
    field: &str,
) -> Result<String, DbDecodeError> {
    let cell = row.get(field).ok_or_else(|| DbDecodeError {
        field_path: field.to_string(),
        expected_type: "String".to_string(),
        received_kind: "missing".to_string(),
        message: "missing column".to_string(),
    })?;
    match &cell.value {
        DbValue::Text(value) => Ok(value.clone()),
        other => Err(DbDecodeError {
            field_path: field.to_string(),
            expected_type: "String".to_string(),
            received_kind: other.kind().to_string(),
            message: "wrong value kind".to_string(),
        }),
    }
}

pub fn decode_i64(row: &BTreeMap<String, DbCell>, field: &str) -> Result<i64, DbDecodeError> {
    let cell = row.get(field).ok_or_else(|| DbDecodeError {
        field_path: field.to_string(),
        expected_type: "Int".to_string(),
        received_kind: "missing".to_string(),
        message: "missing column".to_string(),
    })?;
    match &cell.value {
        DbValue::Integer(value) => Ok(*value),
        other => Err(DbDecodeError {
            field_path: field.to_string(),
            expected_type: "Int".to_string(),
            received_kind: other.kind().to_string(),
            message: "wrong value kind".to_string(),
        }),
    }
}

fn db_value_to_sql_value(value: &DbValue) -> rusqlite::types::Value {
    match value {
        DbValue::Null => rusqlite::types::Value::Null,
        DbValue::Integer(value) => rusqlite::types::Value::Integer(*value),
        DbValue::Float(value) => rusqlite::types::Value::Real(*value),
        DbValue::Text(value) => rusqlite::types::Value::Text(value.clone()),
        DbValue::Bool(value) => rusqlite::types::Value::Integer(i64::from(*value)),
    }
}

fn db_value_from_ref(value: rusqlite::types::ValueRef<'_>) -> DbValue {
    match value {
        rusqlite::types::ValueRef::Null => DbValue::Null,
        rusqlite::types::ValueRef::Integer(value) => DbValue::Integer(value),
        rusqlite::types::ValueRef::Real(value) => DbValue::Float(value),
        rusqlite::types::ValueRef::Text(value) => {
            DbValue::Text(String::from_utf8_lossy(value).to_string())
        }
        rusqlite::types::ValueRef::Blob(_) => DbValue::Text("<blob:redacted>".to_string()),
    }
}

fn redacted_sql_error(err: rusqlite::Error) -> RuntimeError {
    RuntimeError::Other(format!("std.db sqlite error: {err}; values redacted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_execute_query_and_decode_round_trip() {
        let db = SqliteDbRuntime::open_in_memory().expect("sqlite");
        db.execute(
            "create table users(id integer primary key, email text not null)",
            &[],
        )
        .expect("create");
        let inserted = db
            .execute(
                "insert into users(id, email) values (?1, ?2)",
                &[
                    DbValue::Integer(7),
                    DbValue::Text("dev@example.com".to_string()),
                ],
            )
            .expect("insert");
        assert_eq!(inserted.rows_affected, 1);

        let rows = db
            .query(
                "select id, email from users where id = ?1",
                &[DbValue::Integer(7)],
            )
            .expect("query");
        assert_eq!(rows.row_count, 1);
        assert_eq!(decode_i64(&rows.rows[0], "id").unwrap(), 7);
        assert_eq!(
            decode_string(&rows.rows[0], "email").unwrap(),
            "dev@example.com"
        );
    }

    #[test]
    fn sqlite_transaction_rolls_back_on_failure() {
        let db = SqliteDbRuntime::open_in_memory().expect("sqlite");
        db.execute("create table tasks(id integer primary key)", &[])
            .expect("create");
        let failed = db.execute_batch_transaction(&[
            "insert into tasks(id) values (1)",
            "insert into missing_table(id) values (2)",
        ]);
        assert!(failed.is_err());
        let rows = db.query("select id from tasks", &[]).expect("query");
        assert_eq!(rows.row_count, 0);
    }

    #[test]
    fn sqlite_decode_reports_missing_and_wrong_kind() {
        let db = SqliteDbRuntime::open_in_memory().expect("sqlite");
        db.execute("create table users(id integer primary key)", &[])
            .expect("create");
        db.execute("insert into users(id) values (?1)", &[DbValue::Integer(1)])
            .expect("insert");
        let rows = db.query("select id from users", &[]).expect("query");

        let missing = decode_string(&rows.rows[0], "email").expect_err("missing");
        assert_eq!(missing.received_kind, "missing");
        let wrong = decode_string(&rows.rows[0], "id").expect_err("wrong kind");
        assert_eq!(wrong.received_kind, "Int");
    }
}
