//! Local, deterministic feature discovery over ORBIT's existing data sources.
//! Search never runs diagnostics, writes history, or contacts Ollama.

use serde::Serialize;
use std::collections::HashSet;
use tauri::State;

use crate::{
    diagnostics::{DiagnosticResult, ResultStatus, TestType},
    history::{self, Database, HistorySession},
    intelligence::{self, ConfidenceCategory},
    SystemInventory,
};

const MAX_QUERY_CHARS: usize = 200;
const MAX_RESULTS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchSource {
    Page,
    History,
    Finding,
    Action,
    Intelligence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchDestination {
    Section,
    History,
    Intelligence,
    ActionPreview,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub id: String,
    pub source: SearchSource,
    pub destination: SearchDestination,
    pub title: String,
    pub description: String,
    pub section: Option<&'static str>,
    pub session_id: Option<String>,
    pub test_type: Option<TestType>,
    pub recorded_at: Option<String>,
    pub status: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub history_available: bool,
    pub history_note: Option<String>,
}

#[derive(Default, Debug)]
struct Topics {
    dns: bool,
    latency: bool,
    connectivity: bool,
    gateway: bool,
    system: bool,
    diagnostic: bool,
    history: bool,
    latest: bool,
    failed: bool,
    timed_out: bool,
    passed: bool,
    read_only: bool,
    explain: bool,
    settings: bool,
    export: bool,
}

impl Topics {
    fn network(&self) -> bool {
        self.dns || self.latency || self.connectivity || self.gateway || self.diagnostic
    }

    fn specific_test(&self) -> bool {
        self.dns || self.latency || self.connectivity || self.gateway
    }

    fn matches_test(&self, test_type: TestType) -> bool {
        if !self.specific_test() {
            return self.network() || self.history;
        }
        match test_type {
            TestType::Dns => self.dns,
            TestType::Latency => self.latency,
            TestType::Connectivity => self.connectivity,
            TestType::Gateway => self.gateway,
        }
    }
}

fn parse_query(query: &str) -> Result<(String, Topics), String> {
    let value = query.trim();
    if value.is_empty()
        || value.chars().count() > MAX_QUERY_CHARS
        || value.chars().any(char::is_control)
    {
        return Err("Enter 1 to 200 printable characters to search ORBIT.".into());
    }
    let lower = value.to_lowercase();
    let words: HashSet<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    let word = |options: &[&str]| options.iter().any(|option| words.contains(option));
    let phrase = |options: &[&str]| options.iter().any(|option| lower.contains(option));
    let read_only = phrase(&[
        "read only",
        "read-only",
        "without changing",
        "without modifying",
        "no changes",
        "safe check",
    ]);
    let system = word(&[
        "hardware",
        "processor",
        "cpu",
        "ram",
        "memory",
        "specs",
        "inventory",
    ]) || phrase(&[
        "system information",
        "system info",
        "system details",
        "computer information",
        "computer details",
        "windows version",
        "windows build",
        "about this pc",
    ]);
    Ok((
        value.to_owned(),
        Topics {
            dns: word(&["dns", "resolver", "resolve", "resolution", "lookup"]),
            latency: word(&[
                "latency", "lag", "slow", "slowness", "ping", "delay", "jitter", "speed",
            ]),
            connectivity: word(&[
                "connectivity",
                "connect",
                "connection",
                "connections",
                "internet",
                "offline",
                "online",
                "network",
            ]),
            gateway: word(&["gateway", "router", "route"]),
            system,
            diagnostic: word(&[
                "diagnostic",
                "diagnostics",
                "test",
                "tests",
                "check",
                "checks",
                "timeout",
                "timeouts",
            ]),
            history: word(&[
                "history", "recent", "previous", "past", "recorded", "result", "results",
                "session", "sessions",
            ]) || phrase(&["last test", "last diagnostic"]),
            latest: word(&["last", "latest"]) || lower.contains("most recent"),
            failed: word(&["fail", "failed", "failure", "failures", "error", "errors"]),
            timed_out: word(&["timeout", "timeouts"]) || lower.contains("timed out"),
            passed: word(&["passed", "success", "successful", "working"]),
            read_only,
            explain: word(&[
                "why",
                "explain",
                "cause",
                "causes",
                "understand",
                "meaning",
                "troubleshoot",
                "intelligence",
            ]),
            settings: word(&["theme", "appearance", "dark", "light"])
                || (word(&["settings", "preferences"]) && !read_only),
            export: word(&["export", "report", "html"]),
        },
    ))
}

fn page(id: &str, title: &str, description: &str, section: &'static str) -> SearchResult {
    SearchResult {
        id: format!("page-{id}"),
        source: SearchSource::Page,
        destination: SearchDestination::Section,
        title: title.into(),
        description: description.into(),
        section: Some(section),
        session_id: None,
        test_type: None,
        recorded_at: None,
        status: None,
    }
}

fn action(kind: TestType) -> SearchResult {
    let (title, description) = match kind {
        TestType::Connectivity => ("Check connectivity", "Preview a bounded TCP connection check to port 443; you choose the hostname."),
        TestType::Dns => ("Resolve a hostname", "Preview a Windows DNS lookup for a hostname you choose."),
        TestType::Gateway => ("Check default gateway", "Preview one bounded ICMP echo to a Windows-reported IPv4 gateway. Filtering can make the result inconclusive."),
        TestType::Latency => ("Measure latency samples", "Preview five bounded TCP connection attempts to a hostname you choose."),
    };
    SearchResult {
        id: format!("action-{}", kind.as_str()),
        source: SearchSource::Action,
        destination: SearchDestination::ActionPreview,
        title: title.into(),
        description: description.into(),
        section: None,
        session_id: None,
        test_type: Some(kind),
        recorded_at: None,
        status: None,
    }
}

fn history_result(session: &HistorySession, result: &DiagnosticResult) -> SearchResult {
    let target = result
        .target_host
        .as_deref()
        .unwrap_or("reported default gateway");
    SearchResult {
        id: format!("history-{}-{}", session.id, result.test_type.as_str()),
        source: SearchSource::History,
        destination: SearchDestination::History,
        title: format!(
            "{} · {}",
            test_label(result.test_type),
            result.status.as_str()
        ),
        description: format!(
            "Saved result for {target}; {} ms. Open the session for measured details.",
            result.duration_ms
        ),
        section: None,
        session_id: Some(session.id.clone()),
        test_type: Some(result.test_type),
        recorded_at: result.completed_at.clone(),
        status: Some(result.status.as_str().into()),
    }
}

fn test_label(kind: TestType) -> &'static str {
    match kind {
        TestType::Connectivity => "Connectivity",
        TestType::Dns => "DNS resolution",
        TestType::Gateway => "Default gateway",
        TestType::Latency => "Latency samples",
    }
}

fn history_matches(query: &str, topics: &Topics, result: &DiagnosticResult) -> bool {
    let host_match = result
        .target_host
        .as_deref()
        .is_some_and(|host| query.to_lowercase().contains(&host.to_lowercase()));
    if !host_match && !topics.matches_test(result.test_type) {
        return false;
    }
    if topics.failed && result.status != ResultStatus::Failed {
        return false;
    }
    if topics.timed_out {
        let error_timeout = result.error_message.as_deref().is_some_and(|message| {
            let lower = message.to_ascii_lowercase();
            lower.contains("timeout") || lower.contains("timed out")
        });
        let sample_timeout = result
            .details
            .get("samples")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|samples| {
                samples.iter().any(|sample| {
                    sample.get("status").and_then(serde_json::Value::as_str) == Some("timeout")
                })
            });
        if !error_timeout && !sample_timeout {
            return false;
        }
    }
    if topics.passed && result.status != ResultStatus::Success {
        return false;
    }
    true
}

fn matching_history(
    query: &str,
    topics: &Topics,
    sessions: &[HistorySession],
) -> Vec<SearchResult> {
    let mut found = Vec::new();
    let host_query = sessions
        .iter()
        .flat_map(|session| &session.results)
        .any(|result| {
            result
                .target_host
                .as_deref()
                .is_some_and(|host| query.to_lowercase().contains(&host.to_lowercase()))
        });
    if !(topics.history
        || topics.network()
        || topics.failed
        || topics.passed
        || topics.timed_out
        || host_query)
    {
        return found;
    }
    for session in sessions {
        // Results are inserted in execution order. In a newest-first session list,
        // the last result is the best available "last test" approximation.
        for result in session.results.iter().rev() {
            if history_matches(query, topics, result) {
                found.push(history_result(session, result));
                if topics.latest || found.len() >= 6 {
                    return found;
                }
            }
        }
    }
    found
}

fn matching_findings(
    topics: &Topics,
    inventory: Result<&SystemInventory, &str>,
    history: Result<&[HistorySession], &str>,
) -> Vec<SearchResult> {
    if (topics.read_only && !topics.specific_test())
        || !(topics.network() || topics.system || (topics.explain && topics.history))
    {
        return vec![];
    }
    let canonical = if topics.system && !topics.network() {
        "system info"
    } else if topics.dns && !topics.latency && !topics.gateway {
        "dns"
    } else if topics.latency && !topics.dns && !topics.gateway {
        "latency"
    } else if topics.gateway && !topics.dns && !topics.latency {
        "gateway"
    } else {
        "network overview"
    };
    let Ok(analysis) = intelligence::analyze(canonical, inventory, history) else {
        return vec![];
    };
    analysis
        .findings
        .into_iter()
        .filter(|finding| {
            if finding.rule_id == "INTEL-EVIDENCE-001" {
                return false;
            }
            if topics.system {
                return finding.rule_id.starts_with("SYS-")
                    || (topics.network() && finding.rule_id.starts_with("NET-"));
            }
            if !topics.specific_test() {
                return true;
            }
            finding
                .evidence
                .iter()
                .any(|item| item.test_type.is_some_and(|kind| topics.matches_test(kind)))
                || finding.rule_id == "INTEL-HISTORY-001"
        })
        .take(4)
        .enumerate()
        .map(|(index, finding)| {
            let session_id = finding
                .evidence
                .iter()
                .find_map(|reference| reference.session_id.clone());
            let recorded_at = finding
                .evidence
                .iter()
                .find_map(|reference| reference.recorded_at.clone());
            SearchResult {
                id: format!("finding-{}-{index}", finding.rule_id),
                source: SearchSource::Finding,
                destination: if session_id.is_some() {
                    SearchDestination::History
                } else {
                    SearchDestination::Intelligence
                },
                title: finding.title,
                description: finding.observation,
                section: None,
                session_id,
                test_type: None,
                recorded_at,
                status: Some(
                    match finding.confidence {
                        ConfidenceCategory::ConfirmedObservation => "confirmed observation",
                        ConfidenceCategory::StrongIndication => "strong indication",
                        ConfidenceCategory::PossibleCause => "possible cause",
                        ConfidenceCategory::InsufficientEvidence => "insufficient evidence",
                    }
                    .into(),
                ),
            }
        })
        .collect()
}

fn search(
    query: &str,
    inventory: Result<&SystemInventory, &str>,
    history: Result<&[HistorySession], &str>,
) -> Result<SearchResponse, String> {
    let (query, topics) = parse_query(query)?;
    let sessions = history.as_ref().copied().unwrap_or_default();
    let records = matching_history(&query, &topics, sessions);
    let no_matching_records = records.is_empty();
    let mut features = Vec::new();
    if topics.system {
        features.push(page(
            "system",
            "System inventory",
            "View actual Windows, processor, memory, and manufacturer information.",
            "System",
        ));
        features.push(page(
            "overview",
            "System overview",
            "See the current Windows and hardware summary.",
            "Overview",
        ));
    }
    if topics.network() || topics.read_only {
        features.push(page(
            "diagnostics",
            "Network Diagnostics",
            "Run bounded, read-only checks and save actual outcomes to local History.",
            "Diagnostics",
        ));
    }
    if topics.history || topics.export {
        features.push(page(
            "history",
            "Diagnostic history",
            "Inspect saved diagnostic sessions and filter by test or status.",
            "History",
        ));
    }
    if topics.export {
        features.push(page(
            "export",
            "Export an HTML report",
            "Open a saved session in History and choose an explicit report destination.",
            "History",
        ));
    }
    if topics.settings {
        features.push(page(
            "settings",
            "Appearance settings",
            "Choose ORBIT's local theme preference.",
            "Settings",
        ));
    }
    let mut actions = Vec::new();
    for kind in [
        TestType::Connectivity,
        TestType::Dns,
        TestType::Gateway,
        TestType::Latency,
    ] {
        if topics.read_only
            || (topics.network()
                && topics.matches_test(kind)
                && (topics.specific_test() || topics.diagnostic))
        {
            actions.push(action(kind));
        }
    }
    let findings = matching_findings(&topics, inventory, history);
    if topics.explain && (topics.network() || topics.system || topics.history) {
        features.push(SearchResult {
            id: "intelligence-ask".into(), source: SearchSource::Intelligence,
            destination: SearchDestination::Intelligence, title: "Ask Intelligence about this".into(),
            description: "Open this question in ORBIT's deterministic assistant. Local AI is used only if enabled and chosen there.".into(),
            section: None, session_id: None, test_type: None, recorded_at: None, status: None,
        });
    }
    let mut results = Vec::new();
    if topics.history || topics.latest {
        results.extend(records);
        results.extend(findings);
        results.extend(features);
        results.extend(actions);
    } else if topics.explain || topics.failed {
        results.extend(findings);
        results.extend(records);
        results.extend(features);
        results.extend(actions);
    } else {
        results.extend(features);
        results.extend(actions);
        results.extend(findings);
        results.extend(records);
    }
    results.truncate(MAX_RESULTS);
    let history_note = if history.is_err() {
        Some("Saved diagnostic history is unavailable. Page and action matches are still shown; retry after local storage is accessible.".into())
    } else if (topics.history || topics.latest) && no_matching_records {
        Some("No saved diagnostic result matched this search.".into())
    } else {
        None
    };
    Ok(SearchResponse {
        query,
        results,
        history_available: history.is_ok(),
        history_note,
    })
}

#[tauri::command]
pub fn universal_search(query: String, db: State<'_, Database>) -> Result<SearchResponse, String> {
    let (_, topics) = parse_query(&query)?;
    let sessions = history::load_recent_sessions(&db.0);
    let inventory = if topics.network() || topics.system || topics.explain {
        crate::collect_system_inventory()
    } else {
        // The rule engine can still assess missing inventory honestly when no
        // collection was requested by this search.
        Err("Inventory was not requested for this search.".into())
    };
    search(
        &query,
        inventory
            .as_ref()
            .map_err(|_| "System inventory unavailable."),
        sessions
            .as_deref()
            .map_err(|_| "Diagnostic history unavailable."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::SaveSessionRequest;
    use serde_json::json;

    fn sample_session(id: &str, status: ResultStatus, test_type: TestType) -> HistorySession {
        HistorySession {
            id: id.into(),
            started_at: "2026-10-12T08:00:00Z".into(),
            completed_at: "2026-10-12T08:00:01Z".into(),
            duration_ms: 100,
            results: vec![DiagnosticResult {
                test_type,
                target_host: Some("example.com".into()),
                status,
                duration_ms: 25,
                details: json!({}),
                error_message: None,
                completed_at: None,
            }],
        }
    }

    #[test]
    fn page_features_and_everyday_phrases_match_without_ai() {
        let system = search(
            "Where can I see my system information?",
            Err("missing"),
            Ok(&[]),
        )
        .unwrap();
        assert!(system.results.iter().any(|r| r.id == "page-system"));
        let slow = search("Why is my internet slow?", Err("missing"), Ok(&[])).unwrap();
        assert!(slow.results.iter().any(|r| r.id == "action-latency"));
        assert!(slow.results.iter().any(|r| r.id == "intelligence-ask"));
        let safe = search(
            "What can I check without changing Windows settings?",
            Err("missing"),
            Ok(&[]),
        )
        .unwrap();
        assert_eq!(
            safe.results
                .iter()
                .filter(|r| r.source == SearchSource::Action)
                .count(),
            4
        );
        assert!(!safe.results.iter().any(|r| r.id == "page-settings"));
        let connection = search("Does ORBIT test connectivity?", Err("missing"), Ok(&[])).unwrap();
        assert!(connection
            .results
            .iter()
            .any(|r| r.id == "action-connectivity"));
    }

    #[test]
    fn actual_saved_history_is_filtered_and_references_existing_session() {
        let path = std::env::temp_dir().join(format!(
            "orbit-search-{}-{}.sqlite3",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        history::initialize(&path).unwrap();
        let session = sample_session("saved-dns", ResultStatus::Failed, TestType::Dns);
        history::save_session(
            &path,
            SaveSessionRequest {
                id: session.id.clone(),
                started_at: session.started_at.clone(),
                completed_at: session.completed_at.clone(),
                duration_ms: session.duration_ms,
                results: vec![
                    DiagnosticResult {
                        completed_at: Some(session.completed_at.clone()),
                        ..session.results[0].clone()
                    },
                    DiagnosticResult {
                        test_type: TestType::Gateway,
                        target_host: Some("192.0.2.1".into()),
                        status: ResultStatus::Success,
                        duration_ms: 12,
                        details: json!({}),
                        error_message: None,
                        completed_at: Some(session.completed_at.clone()),
                    },
                ],
            },
        )
        .unwrap();
        let stored = history::load_recent_sessions(&path).unwrap();
        let response = search("Show my recent DNS failures", Err("missing"), Ok(&stored)).unwrap();
        let record = response
            .results
            .iter()
            .find(|r| r.source == SearchSource::History)
            .unwrap();
        assert_eq!(record.session_id.as_deref(), Some("saved-dns"));
        assert_eq!(record.status.as_deref(), Some("failed"));
        assert_eq!(
            record.recorded_at.as_deref(),
            Some(session.completed_at.as_str())
        );
        let finding = response
            .results
            .iter()
            .find(|r| r.source == SearchSource::Finding && r.id.starts_with("finding-NET-DNS-001"))
            .unwrap();
        assert_eq!(finding.session_id.as_deref(), Some("saved-dns"));
        assert!(search(
            "Show recent successful DNS results",
            Err("missing"),
            Ok(&stored)
        )
        .unwrap()
        .results
        .iter()
        .all(|r| r.source != SearchSource::History));
        assert_eq!(history::load_recent_sessions(&path).unwrap().len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_unavailable_and_unsupported_searches_are_honest() {
        let empty = search("Show my last diagnostic result", Err("missing"), Ok(&[])).unwrap();
        assert!(empty
            .results
            .iter()
            .all(|r| r.source != SearchSource::History));
        assert!(empty.history_note.unwrap().contains("No saved"));
        let unavailable = search("recent DNS failures", Err("missing"), Err("db closed")).unwrap();
        assert!(!unavailable.history_available);
        assert!(unavailable.history_note.unwrap().contains("unavailable"));
        assert!(unavailable
            .results
            .iter()
            .all(|r| r.source != SearchSource::History));
        let unsupported = search("repair my graphics driver", Err("missing"), Ok(&[])).unwrap();
        assert!(unsupported.results.is_empty());
        let unsupported_explanation =
            search("explain my graphics driver", Err("missing"), Ok(&[])).unwrap();
        assert!(unsupported_explanation.results.is_empty());
    }

    #[test]
    fn search_validates_input_and_only_describes_actions() {
        for bad in [
            "",
            "  ",
            "run\npowershell",
            &"x".repeat(MAX_QUERY_CHARS + 1),
        ] {
            assert!(parse_query(bad).is_err());
        }
        let sessions = [sample_session(
            "existing",
            ResultStatus::Failed,
            TestType::Latency,
        )];
        let before = sessions[0].results.len();
        let response = search("internet slow", Err("missing"), Ok(&sessions)).unwrap();
        assert!(response
            .results
            .iter()
            .any(|r| r.source == SearchSource::Action
                && r.destination == SearchDestination::ActionPreview));
        assert_eq!(sessions[0].results.len(), before);
        assert!(response
            .results
            .iter()
            .filter(|r| r.source == SearchSource::History)
            .all(|r| r.session_id.as_deref() == Some("existing")));
        let record = response
            .results
            .iter()
            .find(|r| r.source == SearchSource::History)
            .unwrap();
        assert_eq!(record.recorded_at, None);
        let inconclusive = [sample_session(
            "uncertain",
            ResultStatus::Inconclusive,
            TestType::Dns,
        )];
        let failures = search("recent DNS failures", Err("missing"), Ok(&inconclusive)).unwrap();
        assert!(failures
            .results
            .iter()
            .all(|r| r.source != SearchSource::History));
    }
}
