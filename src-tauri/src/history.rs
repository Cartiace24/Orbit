use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager, State};

use crate::diagnostics::{DiagnosticResult, ResultStatus, SaveSessionRequest, TestType};

pub struct Database(pub PathBuf);

pub fn initialize(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "ORBIT could not create its local data directory.".to_owned())?;
    }
    let conn = Connection::open(path).map_err(db_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_error)?;
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(db_error)?;
    if version > 2 {
        return Err("The diagnostic database was created by a newer ORBIT version.".into());
    }
    if version == 0 {
        let tx = conn.unchecked_transaction().map_err(db_error)?;
        tx.execute_batch(include_str!("../migrations/0001_diagnostic_history.sql"))
            .map_err(db_error)?;
        tx.pragma_update(None, "user_version", 1)
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
    }
    // A migration marker alone is not enough to decide whether this additive
    // migration ran. Older development builds could leave the marker and
    // schema out of sync. Inspect the real schema so both states are safe.
    if !has_result_completion_time(&conn)? {
        let tx = conn.unchecked_transaction().map_err(db_error)?;
        tx.execute_batch(include_str!(
            "../migrations/0002_result_completion_time.sql"
        ))
        .map_err(db_error)?;
        tx.pragma_update(None, "user_version", 2)
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
    } else if version < 2 {
        // The column may have been added before the migration marker was
        // recorded. The column is additive and already present, so only
        // advance the marker; never attempt a duplicate ALTER TABLE.
        conn.pragma_update(None, "user_version", 2)
            .map_err(db_error)?;
    }
    Ok(())
}

fn has_result_completion_time(conn: &Connection) -> Result<bool, String> {
    let mut statement = conn
        .prepare("PRAGMA table_info(diagnostic_results)")
        .map_err(db_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?;
    for column in columns {
        if column.map_err(db_error)? == "completed_at" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(db_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_error)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(db_error)?;
    Ok(conn)
}

fn db_error(error: rusqlite::Error) -> String {
    // SQLite details are useful in developer logs when reproducing a schema,
    // lock, or corruption issue. Keep table contents, paths, and SQL values
    // out of the UI response.
    eprintln!("[ORBIT][history] SQLite operation failed: {error}");
    match &error {
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            "The local diagnostic database appears damaged. Preserve the database file and contact support; ORBIT has not reset it.".into()
        }
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            "The local diagnostic database is busy. Close other ORBIT instances and try again.".into()
        }
        _ => "The local diagnostic database could not be read. Restart ORBIT; if the problem continues, preserve the database file and contact support.".into(),
    }
}

fn safe_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn safe_timestamp(value: &str) -> bool {
    value.len() <= 40 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn store_result(
    tx: &Transaction<'_>,
    session_id: &str,
    result: &DiagnosticResult,
) -> Result<(), String> {
    if let Some(host) = result.target_host.as_deref() {
        crate::diagnostics::validate_hostname(host)
            .map_err(|_| "A diagnostic target is invalid.".to_owned())?;
    }
    if result.duration_ms > 120_000
        || result
            .completed_at
            .as_deref()
            .is_none_or(|value| !safe_timestamp(value))
    {
        return Err("A diagnostic result exceeded the allowed limits.".into());
    }
    let details = serde_json::to_string(&result.details)
        .map_err(|_| "Diagnostic results could not be encoded.".to_owned())?;
    if details.len() > 12_000 {
        return Err("Diagnostic details exceeded the storage limit.".into());
    }
    let error = result
        .error_message
        .as_deref()
        .map(|message| message.chars().take(280).collect::<String>());
    tx.execute("INSERT INTO diagnostic_results(session_id,test_type,target_host,status,duration_ms,details_json,error_message,completed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![session_id, result.test_type.as_str(), result.target_host, result.status.as_str(), result.duration_ms as i64, details, error, result.completed_at]).map_err(db_error)?;
    Ok(())
}

pub fn save_session(path: &Path, request: SaveSessionRequest) -> Result<(), String> {
    if !safe_session_id(&request.id)
        || !safe_timestamp(&request.started_at)
        || !safe_timestamp(&request.completed_at)
        || request.duration_ms > 300_000
        || request.results.is_empty()
        || request.results.len() > 4
    {
        return Err("The diagnostic session was invalid or exceeded storage limits.".into());
    }
    let mut seen = Vec::new();
    for result in &request.results {
        if seen.contains(&result.test_type) {
            return Err("A session cannot contain duplicate test types.".into());
        }
        seen.push(result.test_type);
    }
    let mut conn = open(path)?;
    let tx = conn.transaction().map_err(db_error)?;
    tx.execute("INSERT INTO diagnostic_sessions(id,started_at,completed_at,duration_ms) VALUES (?1,?2,?3,?4)", params![request.id, request.started_at, request.completed_at, request.duration_ms as i64]).map_err(db_error)?;
    for result in &request.results {
        store_result(&tx, &request.id, result)?;
    }
    tx.commit().map_err(db_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFilter {
    pub test_type: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistorySession {
    pub id: String,
    pub started_at: String,
    pub completed_at: String,
    pub duration_ms: u64,
    pub results: Vec<DiagnosticResult>,
}

#[tauri::command]
pub fn get_diagnostic_history(
    filter: HistoryFilter,
    db: State<'_, Database>,
) -> Result<Vec<HistorySession>, String> {
    read_history(&db.0, filter.test_type.as_deref(), filter.status.as_deref())
}

pub fn load_recent_sessions(path: &Path) -> Result<Vec<HistorySession>, String> {
    read_history(path, None, None)
}

fn read_history(
    path: &Path,
    test_type_filter: Option<&str>,
    status_filter: Option<&str>,
) -> Result<Vec<HistorySession>, String> {
    if test_type_filter.is_some_and(|v| !["connectivity", "dns", "gateway", "latency"].contains(&v))
        || status_filter.is_some_and(|v| !["success", "failed", "inconclusive"].contains(&v))
    {
        return Err("The history filter is invalid.".into());
    }
    let conn = open(path)?;
    let mut statement = conn.prepare("SELECT DISTINCT s.id,s.started_at,s.completed_at,s.duration_ms FROM diagnostic_sessions s JOIN diagnostic_results r ON r.session_id=s.id WHERE (?1 IS NULL OR r.test_type=?1) AND (?2 IS NULL OR r.status=?2) ORDER BY s.started_at DESC LIMIT 250").map_err(db_error)?;
    let rows = statement
        .query_map(params![test_type_filter, status_filter], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(db_error)?;
    let mut sessions = Vec::new();
    for row in rows {
        let (id, started_at, completed_at, duration_ms) = row.map_err(db_error)?;
        let mut tests = conn.prepare("SELECT test_type,target_host,status,duration_ms,details_json,error_message,completed_at FROM diagnostic_results WHERE session_id=?1 ORDER BY id").map_err(db_error)?;
        let results = tests
            .query_map([&id], |row| {
                let kind: String = row.get(0)?;
                let status: String = row.get(2)?;
                let details: String = row.get(4)?;
                let test_type = match kind.as_str() {
                    "connectivity" => TestType::Connectivity,
                    "dns" => TestType::Dns,
                    "gateway" => TestType::Gateway,
                    _ => TestType::Latency,
                };
                let status = match status.as_str() {
                    "success" => ResultStatus::Success,
                    "failed" => ResultStatus::Failed,
                    _ => ResultStatus::Inconclusive,
                };
                Ok(DiagnosticResult {
                    test_type,
                    target_host: row.get(1)?,
                    status,
                    duration_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
                    error_message: row.get(5)?,
                    completed_at: row.get(6)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        sessions.push(HistorySession {
            id,
            started_at,
            completed_at,
            duration_ms: duration_ms.max(0) as u64,
            results,
        });
    }
    Ok(sessions)
}

#[tauri::command]
pub fn export_diagnostic_report(
    app: AppHandle,
    session_id: String,
    output_path: String,
) -> Result<(), String> {
    if !safe_session_id(&session_id)
        || output_path.len() > 2_000
        || Path::new(&output_path)
            .extension()
            .and_then(|v| v.to_str())
            .is_none_or(|v| !v.eq_ignore_ascii_case("html"))
    {
        return Err("Choose a valid .html report destination.".into());
    }
    let path = app
        .path()
        .app_data_dir()
        .map_err(|_| "ORBIT could not locate its local data directory.".to_owned())?
        .join("diagnostics.sqlite3");
    export_session_report(&path, &session_id, Path::new(&output_path))
}

fn export_session_report(
    database_path: &Path,
    session_id: &str,
    output_path: &Path,
) -> Result<(), String> {
    let session = load_session(database_path, session_id)?;
    let html = render_report(&session);
    if html.len() > 256_000 {
        return Err("The report exceeded the export size limit.".into());
    }
    fs::write(output_path, html).map_err(|error| {
        eprintln!("[ORBIT][history] Report file write failed: {error}");
        "ORBIT could not write the report to that destination.".to_owned()
    })
}

fn load_session(path: &Path, id: &str) -> Result<HistorySession, String> {
    let conn = open(path)?;
    let row = conn
        .query_row(
            "SELECT id,started_at,completed_at,duration_ms FROM diagnostic_sessions WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?
        .ok_or("That diagnostic session is no longer available.")?;
    let mut stmt = conn.prepare("SELECT test_type,target_host,status,duration_ms,details_json,error_message,completed_at FROM diagnostic_results WHERE session_id=?1 ORDER BY id").map_err(db_error)?;
    let results = stmt
        .query_map([id], |row| {
            let kind: String = row.get(0)?;
            let status: String = row.get(2)?;
            let details: String = row.get(4)?;
            Ok(DiagnosticResult {
                test_type: match kind.as_str() {
                    "connectivity" => TestType::Connectivity,
                    "dns" => TestType::Dns,
                    "gateway" => TestType::Gateway,
                    _ => TestType::Latency,
                },
                target_host: row.get(1)?,
                status: match status.as_str() {
                    "success" => ResultStatus::Success,
                    "failed" => ResultStatus::Failed,
                    _ => ResultStatus::Inconclusive,
                },
                duration_ms: row.get::<_, i64>(3)?.max(0) as u64,
                details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
                error_message: row.get(5)?,
                completed_at: row.get(6)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(HistorySession {
        id: row.0,
        started_at: row.1,
        completed_at: row.2,
        duration_ms: row.3.max(0) as u64,
        results,
    })
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_report(session: &HistorySession) -> String {
    let mut sections = String::new();
    for result in &session.results {
        let name = result.test_type.as_str();
        let status = result.status.as_str();
        let target = result
            .target_host
            .as_deref()
            .map(escape_html)
            .unwrap_or_else(|| "Default gateway".into());
        let details =
            escape_html(&serde_json::to_string_pretty(&result.details).unwrap_or_default());
        let error = result
            .error_message
            .as_deref()
            .map(escape_html)
            .unwrap_or_default();
        let guidance = match result.status { ResultStatus::Success => "No follow-up indicated by this probe.", ResultStatus::Failed => "Check the hostname, local network connection, and whether the tested service is expected to accept connections.", ResultStatus::Inconclusive => "Repeat the test and consider firewall, VPN, routing, or ICMP filtering effects." };
        sections.push_str(&format!("<section><h2>{}</h2><p><b>Status:</b> {} · <b>Target:</b> {} · <b>Duration:</b> {} ms</p><pre>{}</pre><p>{}</p><p>{}</p></section>", escape_html(name), escape_html(status), target, result.duration_ms, details, error, guidance));
    }
    format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>ORBIT diagnostic report</title><style>body{{font:15px 'Segoe UI',sans-serif;max-width:900px;margin:40px auto;padding:0 24px;background:#151918;color:#e4e9e5}}h1,h2{{color:#a9c4ad}}section{{border-top:1px solid #38413b;padding:16px 0}}pre{{white-space:pre-wrap;background:#1d2321;padding:14px;border:1px solid #38413b}}small{{color:#9aa59d}}</style><h1>ORBIT diagnostic report</h1><p>ORBIT 0.1.0 · Session {}</p><p>Started {} · Completed {} · Duration {} ms</p>{}</html>", escape_html(&session.id), escape_html(&session.started_at), escape_html(&session.completed_at), session.duration_ms, sections)
}

#[cfg(test)]
mod tests {
    use super::{initialize, render_report, save_session, HistorySession};
    use crate::diagnostics::{DiagnosticResult, ResultStatus, SaveSessionRequest, TestType};
    use chrono::Utc;
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_database_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "orbit-history-test-{}-{}-{}.sqlite",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            TEST_DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn migrations_and_completed_sessions_persist_when_database_is_reopened() {
        let path = test_database_path();
        initialize(&path).unwrap();
        initialize(&path).unwrap();
        let now = Utc::now().to_rfc3339();
        let result = DiagnosticResult {
            test_type: TestType::Dns,
            target_host: Some("example.com".into()),
            status: ResultStatus::Success,
            duration_ms: 12,
            details: json!({"addresses":["192.0.2.4"]}),
            error_message: None,
            completed_at: Some(now.clone()),
        };
        save_session(
            &path,
            SaveSessionRequest {
                id: "session-1".into(),
                started_at: now.clone(),
                completed_at: now.clone(),
                duration_ms: 14,
                results: vec![result],
            },
        )
        .unwrap();
        drop(Connection::open(&path).unwrap());
        let reopened = Connection::open(&path).unwrap();
        let (version, sessions, results): (i32, i64, i64) = reopened.query_row("SELECT (SELECT user_version FROM pragma_user_version),(SELECT COUNT(*) FROM diagnostic_sessions),(SELECT COUNT(*) FROM diagnostic_results)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!((version, sessions, results), (2, 1, 1));
        drop(reopened);
        let persisted = super::load_recent_sessions(&path).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(
            persisted[0].results[0].completed_at.as_deref(),
            Some(now.as_str())
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn version_one_history_migrates_without_losing_sessions_or_results() {
        let path = test_database_path();
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(include_str!("../migrations/0001_diagnostic_history.sql"))
            .unwrap();
        legacy.pragma_update(None, "user_version", 1).unwrap();
        let now = Utc::now().to_rfc3339();
        legacy.execute("INSERT INTO diagnostic_sessions(id,started_at,completed_at,duration_ms) VALUES (?1,?2,?2,10)", rusqlite::params!["legacy-session", now]).unwrap();
        legacy.execute("INSERT INTO diagnostic_results(session_id,test_type,target_host,status,duration_ms,details_json,error_message) VALUES (?1,'dns','example.com','success',9,'{}',NULL)", ["legacy-session"]).unwrap();
        // Reproduce the schema mismatch that causes the History/export reader
        // to fail before migration. The V1 records themselves remain intact.
        let raw_error = legacy
            .prepare("SELECT completed_at FROM diagnostic_results")
            .err()
            .expect("V1 schema has no per-result completion time column");
        assert!(raw_error
            .to_string()
            .contains("no such column: completed_at"));
        drop(raw_error);
        assert!(super::read_history(&path, None, None).is_err());
        drop(legacy);

        initialize(&path).unwrap();
        let sessions = super::load_recent_sessions(&path).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "legacy-session");
        assert_eq!(sessions[0].results.len(), 1);
        assert_eq!(sessions[0].results[0].completed_at, None);
        let export_path = test_database_path().with_extension("html");
        super::export_session_report(&path, "legacy-session", &export_path).unwrap();
        let report = std::fs::read_to_string(&export_path).unwrap();
        assert!(report.contains("legacy-session"));
        assert!(report.contains("Status:</b> success"));
        std::fs::remove_file(export_path).unwrap();
        let version: i32 = Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        drop(sessions);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_repairs_schema_marker_mismatch_without_duplicate_alter() {
        let path = test_database_path();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("../migrations/0001_diagnostic_history.sql"))
            .unwrap();
        connection
            .execute_batch(include_str!(
                "../migrations/0002_result_completion_time.sql"
            ))
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);

        initialize(&path).unwrap();
        initialize(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        let version: i32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        assert!(super::has_result_completion_time(&connection).unwrap());
        drop(connection);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn version_two_marker_with_missing_column_is_additively_repaired() {
        let path = test_database_path();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("../migrations/0001_diagnostic_history.sql"))
            .unwrap();
        connection
            .execute("INSERT INTO diagnostic_sessions(id,started_at,completed_at,duration_ms) VALUES ('retained-session','2026-01-02T03:04:05Z','2026-01-02T03:04:06Z',1)", [])
            .unwrap();
        connection
            .execute("INSERT INTO diagnostic_results(session_id,test_type,status,duration_ms,details_json) VALUES ('retained-session','dns','success',1,'{}')", [])
            .unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);

        initialize(&path).unwrap();
        let sessions = super::load_recent_sessions(&path).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "retained-session");
        assert_eq!(sessions[0].results.len(), 1);
        assert_eq!(sessions[0].results[0].completed_at, None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reports_escape_html_and_include_status_guidance() {
        let now = Utc::now().to_rfc3339();
        let session = HistorySession {
            id: "safe-id".into(),
            started_at: now.clone(),
            completed_at: now.clone(),
            duration_ms: 20,
            results: vec![DiagnosticResult {
                test_type: TestType::Connectivity,
                target_host: Some("example.com".into()),
                status: ResultStatus::Failed,
                duration_ms: 20,
                details: json!({"note":"<unsafe>"}),
                error_message: Some("Could not connect".into()),
                completed_at: Some(now),
            }],
        };
        let report = render_report(&session);
        assert!(report.contains("&lt;unsafe&gt;"));
        assert!(report.contains("Status:</b> failed"));
        assert!(report.contains("Check the hostname"));
    }
}
