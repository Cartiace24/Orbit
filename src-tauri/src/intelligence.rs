//! Deterministic analysis of verified ORBIT inventory and persisted network-test results.
//! This module has no network, shell, mutation, or model dependency. A future local narrator
//! may consume `Finding` values, but this rule engine remains the source of observations.

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::{
    diagnostics::{DiagnosticResult, ResultStatus, TestType},
    history::{self, Database, HistorySession},
    SystemInventory,
};

const MAX_QUESTION_CHARS: usize = 500;
const LATENCY_HIGH_THRESHOLD_MS: f64 = 150.0;
const EXPECTED_LATENCY_SAMPLES: usize = 5;
const MIN_COMPLETE_LATENCY_SAMPLES: usize = 3;
const REPEATED_FAILURE_SESSION_THRESHOLD: usize = 3;

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ConfidenceCategory {
    ConfirmedObservation,
    StrongIndication,
    PossibleCause,
    InsufficientEvidence,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Informational,
    Low,
    Moderate,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceSource {
    SystemInventory,
    DiagnosticHistory,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReference {
    pub source: EvidenceSource,
    pub session_id: Option<String>,
    pub test_type: Option<TestType>,
    pub field: String,
    pub observed: String,
    pub recorded_at: Option<String>,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule_id: String,
    pub title: String,
    pub rule_condition: String,
    pub evidence_required: String,
    pub observation: String,
    pub interpretation: String,
    pub evidence: Vec<EvidenceReference>,
    pub possible_causes: Vec<String>,
    pub confidence: ConfidenceCategory,
    pub risk_level: RiskLevel,
    pub recommended_next_steps: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub suggested_tests: Vec<TestType>,
    pub limitations: Vec<String>,
    pub weakening_evidence: Vec<String>,
}

/// Registry entries document stable rule IDs, triggering conditions, required evidence,
/// severity, explanation, actions, known limitations, and observations that weaken the rule.
#[derive(Clone, Copy, Debug)]
pub struct RuleDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub condition: &'static str,
    pub evidence_required: &'static str,
    pub severity: RiskLevel,
    pub explanation: &'static str,
    pub recommended_actions: &'static [&'static str],
    pub limitations: &'static [&'static str],
    pub weakening_evidence: &'static [&'static str],
}

pub const RULE_REGISTRY: &[RuleDefinition] = &[
    RuleDefinition {
        id: "NET-DNS-001",
        title: "DNS failed while the gateway answered",
        condition: "Within one persisted session, DNS resolution is failed and the IPv4 gateway probe succeeds.",
        evidence_required: "A failed DNS result and a successful gateway result with the same session ID.",
        severity: RiskLevel::Low,
        explanation: "The recorded hostname lookup failed while the local gateway answered an ICMP echo. This narrows the observation to name resolution or the tested name; it does not identify the resolver as the only cause.",
        recommended_actions: &["Repeat DNS resolution for the same hostname.", "Compare with a fresh connectivity check after DNS succeeds."],
        limitations: &["The gateway ICMP result does not prove internet reachability.", "A mistyped, newly created, or intentionally unavailable hostname can also fail resolution."],
        weakening_evidence: &["A later successful lookup for the same hostname weakens a persistent DNS issue."],
    },
    RuleDefinition {
        id: "NET-GATEWAY-001",
        title: "Default gateway reachability was not confirmed",
        condition: "A persisted gateway test is inconclusive or failed.",
        evidence_required: "A gateway diagnostic result whose status is not success.",
        severity: RiskLevel::Low,
        explanation: "The gateway probe did not confirm an IPv4 echo response or an interface gateway was unavailable.",
        recommended_actions: &["Repeat the gateway check.", "Review the local connection and VPN state if the result repeats."],
        limitations: &["Firewalls and network policy can block ICMP even when routing works.", "A missing reported gateway can reflect interface or route selection."],
        weakening_evidence: &["A successful later gateway response weakens a claim of ongoing gateway unreachability."],
    },
    RuleDefinition {
        id: "NET-LATENCY-001",
        title: "Latency samples are slow or inconsistent",
        condition: "At least three latency samples completed and their average TCP connection setup is at least 150 ms, or successful and failed samples coexist.",
        evidence_required: "Per-sample statuses and latency values from a persisted latency result; fewer than three successes are insufficient for a pattern finding.",
        severity: RiskLevel::Moderate,
        explanation: "TCP connection setup samples were slow or mixed. The 150 ms threshold is a screening heuristic for this probe, not a service-quality standard.",
        recommended_actions: &["Repeat the five-sample latency test.", "Compare a second hostname to separate target-specific behavior from a broader route pattern."],
        limitations: &["The probe measures TCP connection setup to port 443, not ICMP round-trip time, TLS, or application response time.", "A small sample can vary with server load, routing, or local scheduling."],
        weakening_evidence: &["Repeated complete runs with stable low samples weaken an ongoing latency concern."],
    },
    RuleDefinition {
        id: "NET-TIMEOUT-001",
        title: "A test timed out or lacked interface evidence",
        condition: "A stored test is inconclusive, contains a timeout sample, or reports an unavailable gateway/interface.",
        evidence_required: "A persisted non-success status and/or an explicit timeout or unavailable-interface result detail.",
        severity: RiskLevel::Informational,
        explanation: "The check did not obtain enough evidence to classify the path as working or failing.",
        recommended_actions: &["Retry the specific check once.", "If the gateway was unavailable, check whether Windows reports an active network route."],
        limitations: &["A timeout is not proof that the destination is offline.", "ICMP filtering can make gateway checks inconclusive."],
        weakening_evidence: &["A later completed test with a response weakens the timeout observation as a current issue."],
    },
    RuleDefinition {
        id: "SYS-INVENTORY-001",
        title: "Some system inventory is unavailable",
        condition: "The inventory collection failed or one or more requested fields are explicitly unavailable.",
        evidence_required: "A collection error or fields marked unavailable in the live system inventory response.",
        severity: RiskLevel::Informational,
        explanation: "Windows did not provide every requested inventory field during this collection.",
        recommended_actions: &["Refresh the System inventory.", "Use the field-specific reason to determine whether Windows omitted or denied that value."],
        limitations: &["Unavailable values do not imply a hardware fault.", "This inventory does not measure CPU/GPU temperature, drive health, event logs, or process state."],
        weakening_evidence: &["A later complete inventory collection weakens this missing-data observation."],
    },
    RuleDefinition {
        id: "NET-REPEAT-001",
        title: "The same network test has repeated a non-success result",
        condition: "At least three distinct persisted sessions contain the same test type, normalized target, result status, and sanitized error text, with no newer success for that test and target.",
        evidence_required: "Three or more matching non-success results from distinct session IDs in the retained history window.",
        severity: RiskLevel::Low,
        explanation: "The same check produced the same non-success outcome in multiple recorded sessions. Repetition supports reproducibility, not a specific root cause.",
        recommended_actions: &["Run the same test once more after checking the target and local network state.", "Compare a different hostname if the repeated result is target-specific."],
        limitations: &["Only the newest 250 sessions are available to this analysis.", "The comparison groups by test, target, status, and stored error text; it cannot infer unrecorded network changes."],
        weakening_evidence: &["A newer success for the same test and target breaks the repeated-failure sequence."],
    },
    RuleDefinition {
        id: "INTEL-EVIDENCE-001",
        title: "More evidence is needed",
        condition: "A supported question has no finding that meets the rule's evidence threshold.",
        evidence_required: "At least one relevant observation with the required scope and sample completeness.",
        severity: RiskLevel::Informational,
        explanation: "The current stored evidence is not sufficient to establish a matching condition.",
        recommended_actions: &["Run a relevant read-only diagnostic check.", "Review the evidence threshold and missing evidence before drawing a conclusion."],
        limitations: &["No conclusion can be drawn from an absent or incomplete observation."],
        weakening_evidence: &["A new complete test result may satisfy the rule's evidence requirements."],
    },
    RuleDefinition {
        id: "INTEL-HISTORY-001",
        title: "Diagnostic history is unavailable",
        condition: "The existing local SQLite diagnostic-history query could not be read.",
        evidence_required: "A successful query of the existing ORBIT diagnostic history database.",
        severity: RiskLevel::Informational,
        explanation: "Network findings that depend on saved diagnostic sessions could not be evaluated.",
        recommended_actions: &["Retry after the local history service is available.", "Use Network Diagnostics to collect a new result if history remains unreadable."],
        limitations: &["This does not imply that the network or the database contents are corrupt."],
        weakening_evidence: &["A successful history read restores comparisons against saved results."],
    },
];

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
enum Intent {
    NetworkOverview,
    Dns,
    Gateway,
    Latency,
    Connectivity,
    SystemInventory,
    RepeatedFailures,
    Unsupported,
}

impl Intent {
    fn label(self) -> &'static str {
        match self {
            Self::NetworkOverview => "networkOverview",
            Self::Dns => "dns",
            Self::Gateway => "gateway",
            Self::Latency => "latency",
            Self::Connectivity => "connectivity",
            Self::SystemInventory => "systemInventory",
            Self::RepeatedFailures => "repeatedFailures",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedQuestion {
    pub prompt: String,
    pub reason: String,
    pub test_type: Option<TestType>,
    pub target_host: Option<String>,
    pub purpose: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntelligenceResponse {
    pub question: String,
    pub intent: String,
    pub supported: bool,
    pub summary: String,
    pub inventory_collected_at: Option<String>,
    pub history_available: bool,
    pub findings: Vec<Finding>,
    pub suggested_questions: Vec<SuggestedQuestion>,
}

fn parse_intent(question: &str) -> Result<Intent, &'static str> {
    let trimmed = question.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_QUESTION_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err("Enter a question of 1 to 500 printable characters.");
    }
    let normalized = trimmed.to_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| normalized.contains(term));
    Ok(
        if has(&[
            "dns",
            "name lookup",
            "name resolution",
            "resolve",
            "resolution",
        ]) {
            Intent::Dns
        } else if has(&["gateway", "router", "default route"]) {
            Intent::Gateway
        } else if has(&[
            "latency",
            "lag",
            "intermittent",
            "slow network",
            "network slow",
            "internet slow",
            "slow internet",
            "slow connection",
            "connection slow",
            "high ping",
        ]) {
            Intent::Latency
        } else if has(&[
            "repeat",
            "repeated",
            "recurring",
            "keeps failing",
            "again",
            "history",
        ]) {
            Intent::RepeatedFailures
        } else if has(&[
            "system inventory",
            "system info",
            "hardware details",
            "windows info",
            "missing system",
            "unavailable field",
        ]) {
            Intent::SystemInventory
        } else if has(&[
            "network overview",
            "network status",
            "network results",
            "last diagnostic result",
            "latest diagnostic result",
            "last test result",
            "internet connection",
            "network connection",
        ]) {
            Intent::NetworkOverview
        } else if has(&[
            "connectivity",
            "connect",
            "internet",
            "offline",
            "online",
            "connection",
        ]) {
            Intent::Connectivity
        } else {
            Intent::Unsupported
        },
    )
}

fn rule(id: &str) -> &'static RuleDefinition {
    RULE_REGISTRY
        .iter()
        .find(|entry| entry.id == id)
        .expect("registered rule ID")
}

fn history_evidence(
    session: &HistorySession,
    result: &DiagnosticResult,
    field: &str,
    observed: String,
) -> EvidenceReference {
    EvidenceReference {
        source: EvidenceSource::DiagnosticHistory,
        session_id: Some(session.id.clone()),
        test_type: Some(result.test_type),
        field: field.to_owned(),
        observed,
        recorded_at: result.completed_at.clone(),
    }
}

fn inventory_evidence(
    field: &str,
    observed: String,
    collected_at: Option<&str>,
) -> EvidenceReference {
    EvidenceReference {
        source: EvidenceSource::SystemInventory,
        session_id: None,
        test_type: None,
        field: field.to_owned(),
        observed,
        recorded_at: collected_at.map(str::to_owned),
    }
}

fn make_finding(
    definition: &RuleDefinition,
    observation: String,
    confidence: ConfidenceCategory,
    evidence: Vec<EvidenceReference>,
    causes: &[&str],
    missing_evidence: &[&str],
    suggested_tests: &[TestType],
) -> Finding {
    Finding {
        rule_id: definition.id.to_owned(),
        title: definition.title.to_owned(),
        rule_condition: definition.condition.to_owned(),
        evidence_required: definition.evidence_required.to_owned(),
        observation,
        interpretation: definition.explanation.to_owned(),
        evidence,
        possible_causes: causes.iter().map(|value| (*value).to_owned()).collect(),
        confidence,
        risk_level: definition.severity,
        recommended_next_steps: definition
            .recommended_actions
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        missing_evidence: missing_evidence
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        suggested_tests: suggested_tests.to_vec(),
        limitations: definition
            .limitations
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        weakening_evidence: definition
            .weakening_evidence
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

fn missing_inventory_fields(
    inventory: &SystemInventory,
    collected_at: &str,
) -> Vec<EvidenceReference> {
    let mut fields = Vec::new();
    macro_rules! check {
        ($label:expr, $field:expr) => {
            if $field.availability != "available" || $field.value.is_none() {
                fields.push(inventory_evidence(
                    $label,
                    $field
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Windows did not provide this value.".into()),
                    Some(collected_at),
                ));
            }
        };
    }
    check!(
        "operatingSystem.productName",
        inventory.operating_system.product_name
    );
    check!(
        "operatingSystem.edition",
        inventory.operating_system.edition
    );
    check!(
        "operatingSystem.displayVersion",
        inventory.operating_system.display_version
    );
    check!("operatingSystem.build", inventory.operating_system.build);
    check!(
        "operatingSystem.architecture",
        inventory.operating_system.architecture
    );
    check!("computer.name", inventory.computer.name);
    check!("computer.manufacturer", inventory.computer.manufacturer);
    check!("computer.model", inventory.computer.model);
    check!("processor.model", inventory.processor.model);
    check!(
        "processor.logicalProcessors",
        inventory.processor.logical_processors
    );
    check!("memory.installedBytes", inventory.memory.installed_bytes);
    fields
}

fn result_error_text(result: &DiagnosticResult) -> String {
    result
        .error_message
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn has_timeout_or_interface_gap(result: &DiagnosticResult) -> bool {
    let error = result_error_text(result);
    if result.status != ResultStatus::Success
        && [
            "timeout",
            "timed out",
            "exceeded",
            "did not respond",
            "could not be completed",
            "did not report",
            "unavailable",
        ]
        .iter()
        .any(|term| error.contains(term))
    {
        return true;
    }
    result
        .details
        .get("samples")
        .and_then(Value::as_array)
        .is_some_and(|samples| {
            samples
                .iter()
                .any(|sample| sample.get("status").and_then(Value::as_str) == Some("timeout"))
        })
}

fn latency_observation(result: &DiagnosticResult) -> (usize, usize, Vec<u64>, usize) {
    let samples = result
        .details
        .get("samples")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let expected = result
        .details
        .get("sampleCount")
        .and_then(Value::as_u64)
        .unwrap_or(EXPECTED_LATENCY_SAMPLES as u64)
        .min(EXPECTED_LATENCY_SAMPLES as u64) as usize;
    let mut values = Vec::new();
    let mut failed = 0;
    for sample in &samples {
        if sample.get("status").and_then(Value::as_str) == Some("success") {
            if let Some(ms) = sample.get("latencyMs").and_then(Value::as_u64) {
                values.push(ms);
            }
        } else if sample.get("status").and_then(Value::as_str).is_some() {
            failed += 1;
        }
    }
    (values.len(), expected, values, failed)
}

fn findings_from_inventory(inventory: Result<&SystemInventory, &str>) -> Vec<Finding> {
    let definition = rule("SYS-INVENTORY-001");
    match inventory {
        Err(error) => vec![make_finding(
            definition,
            format!("System information collection failed: {error}"),
            ConfidenceCategory::ConfirmedObservation,
            vec![inventory_evidence(
                "collection",
                "Native system inventory command returned an error.".into(),
                None,
            )],
            &[],
            &["A subsequent successful inventory refresh."],
            &[],
        )],
        Ok(inventory) => {
            let evidence = missing_inventory_fields(inventory, &inventory.collected_at);
            if evidence.is_empty() {
                Vec::new()
            } else {
                let labels = evidence
                    .iter()
                    .map(|item| item.field.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                vec![make_finding(
                    definition,
                    format!("These requested fields were unavailable: {labels}."),
                    ConfidenceCategory::ConfirmedObservation,
                    evidence,
                    &["Windows omitted a value or access to that value was denied."],
                    &["A fresh inventory collection."],
                    &[],
                )]
            }
        }
    }
}

fn rule_findings(
    sessions: &[HistorySession],
    history_available: bool,
    inventory: Result<&SystemInventory, &str>,
) -> Vec<Finding> {
    let mut findings = findings_from_inventory(inventory);
    if !history_available {
        findings.push(make_finding(rule("INTEL-HISTORY-001"), "Persisted diagnostic history could not be read, so network rules could not compare recorded tests.".into(), ConfidenceCategory::InsufficientEvidence, vec![], &[], &["A readable local diagnostic history database."], &[]));
        return findings;
    }

    for session in sessions {
        let dns_failed = session
            .results
            .iter()
            .find(|item| item.test_type == TestType::Dns && item.status == ResultStatus::Failed);
        let gateway_ok = session.results.iter().find(|item| {
            item.test_type == TestType::Gateway && item.status == ResultStatus::Success
        });
        if let (Some(dns), Some(gateway)) = (dns_failed, gateway_ok) {
            findings.push(make_finding(
                rule("NET-DNS-001"),
                "DNS resolution failed while the gateway echo succeeded in the same session."
                    .into(),
                ConfidenceCategory::ConfirmedObservation,
                vec![
                    history_evidence(
                        session,
                        dns,
                        "status",
                        format!(
                            "DNS status: {}; target: {}",
                            dns.status.as_str(),
                            dns.target_host.as_deref().unwrap_or("unavailable")
                        ),
                    ),
                    history_evidence(
                        session,
                        gateway,
                        "status",
                        format!("Gateway status: {}", gateway.status.as_str()),
                    ),
                ],
                &[
                    "The tested hostname may be misspelled or absent.",
                    "A resolver path or upstream DNS service may be having trouble.",
                ],
                &[
                    "A repeat DNS query for the same target.",
                    "A later successful resolution.",
                ],
                &[TestType::Dns],
            ));
        }
    }

    for session in sessions {
        for result in &session.results {
            if result.test_type == TestType::Gateway && result.status != ResultStatus::Success {
                let no_gateway = result.target_host.is_none();
                findings.push(make_finding(
                    rule("NET-GATEWAY-001"),
                    if no_gateway {
                        "Windows did not provide an IPv4 gateway address for this check.".into()
                    } else {
                        "The IPv4 gateway did not return an ICMP echo response.".into()
                    },
                    ConfidenceCategory::ConfirmedObservation,
                    vec![history_evidence(
                        session,
                        result,
                        "status",
                        format!(
                            "Gateway status: {}; target: {}",
                            result.status.as_str(),
                            result.target_host.as_deref().unwrap_or("unavailable")
                        ),
                    )],
                    &[
                        "ICMP may be filtered by a host firewall or network policy.",
                        "The active interface or route may have changed.",
                    ],
                    &[
                        "A fresh gateway probe.",
                        "The reported active network route.",
                    ],
                    &[TestType::Gateway],
                ));
            }

            if result.test_type == TestType::Latency {
                let (successful, expected, values, failed) = latency_observation(result);
                if expected == 0 {
                    continue;
                }
                if successful >= MIN_COMPLETE_LATENCY_SAMPLES {
                    let average = values.iter().sum::<u64>() as f64 / successful as f64;
                    let intermittent = failed > 0 || successful < expected;
                    if average >= LATENCY_HIGH_THRESHOLD_MS || intermittent {
                        let confidence = if intermittent {
                            ConfidenceCategory::PossibleCause
                        } else {
                            ConfidenceCategory::StrongIndication
                        };
                        let observation = if average >= LATENCY_HIGH_THRESHOLD_MS && intermittent {
                            format!("{successful} of {expected} latency samples completed; their mean TCP setup time was {average:.1} ms and at least one sample was incomplete.")
                        } else if average >= LATENCY_HIGH_THRESHOLD_MS {
                            format!("{successful} of {expected} samples completed with a mean TCP setup time of {average:.1} ms (screening threshold: {LATENCY_HIGH_THRESHOLD_MS:.0} ms).")
                        } else {
                            format!("{successful} of {expected} samples completed and at least one other sample failed or timed out.")
                        };
                        let causes: &[&str] = if intermittent {
                            &["Variable routing, transient filtering, or target-side load may contribute."]
                        } else {
                            &["A slower route, target-side load, or local scheduling may contribute."]
                        };
                        let missing = if successful < expected {
                            vec!["A complete five-sample run for comparison.".to_owned()]
                        } else {
                            Vec::new()
                        };
                        findings.push(make_finding(rule("NET-LATENCY-001"), observation, confidence, vec![history_evidence(session, result, "samples", format!("{successful}/{expected} successful TCP samples; observed values: {values:?} ms; {failed} explicit failed samples"))], causes, &[], &[TestType::Latency]));
                        if let Some(last) = findings.last_mut() {
                            last.missing_evidence = missing;
                        }
                    }
                } else if successful > 0 || failed > 0 || result.status != ResultStatus::Success {
                    findings.push(make_finding(rule("NET-LATENCY-001"), format!("Only {successful} of {expected} samples completed; fewer than {MIN_COMPLETE_LATENCY_SAMPLES} successful samples are available for a latency pattern."), ConfidenceCategory::InsufficientEvidence, vec![history_evidence(session, result, "samples", format!("{successful}/{expected} successful TCP samples; observed values: {values:?} ms; {failed} explicit failed samples"))], &[], &["At least three successful samples, preferably all five."], &[TestType::Latency]));
                }
            }

            if has_timeout_or_interface_gap(result) {
                findings.push(make_finding(rule("NET-TIMEOUT-001"), format!("{} returned an inconclusive or timed-out result: {}", result.test_type.as_str(), result.error_message.as_deref().unwrap_or("one or more samples timed out")), ConfidenceCategory::ConfirmedObservation, vec![history_evidence(session, result, "status", format!("{}: {}; {} ms", result.test_type.as_str(), result.status.as_str(), result.duration_ms))], &["A transient route, filtering rule, or unavailable interface may have prevented a response."], &["Repeat the same test when the connection is stable."], &[result.test_type]));
            }
        }
    }

    findings.extend(repeated_failure_findings(sessions));
    findings
}

#[derive(Clone, PartialEq, Eq)]
struct FailureKey {
    test_type: TestType,
    target: Option<String>,
    status: ResultStatus,
    error: Option<String>,
}

fn repeated_failure_findings(sessions: &[HistorySession]) -> Vec<Finding> {
    let mut groups: Vec<(FailureKey, Vec<(&HistorySession, &DiagnosticResult)>)> = Vec::new();
    for session in sessions {
        for result in &session.results {
            if result.status == ResultStatus::Success {
                continue;
            }
            let key = FailureKey {
                test_type: result.test_type,
                target: result.target_host.as_deref().map(str::to_ascii_lowercase),
                status: result.status,
                error: result.error_message.clone(),
            };
            if let Some((_, rows)) = groups.iter_mut().find(|(candidate, _)| *candidate == key) {
                if !rows.iter().any(|(prior, _)| prior.id == session.id) {
                    rows.push((session, result));
                }
            } else {
                groups.push((key, vec![(session, result)]));
            }
        }
    }
    let mut findings = Vec::new();
    for (key, rows) in groups {
        if rows.len() < REPEATED_FAILURE_SESSION_THRESHOLD {
            continue;
        }
        let newest_failure_at = rows
            .iter()
            .map(|(session, _)| session.started_at.as_str())
            .max()
            .unwrap_or("");
        let newer_success = sessions
            .iter()
            .flat_map(|session| session.results.iter().map(move |result| (session, result)))
            .any(|(session, result)| {
                result.test_type == key.test_type
                    && result.status == ResultStatus::Success
                    && result.target_host.as_deref().map(str::to_ascii_lowercase) == key.target
                    && session.started_at.as_str() > newest_failure_at
            });
        if newer_success {
            continue;
        }
        let evidence = rows
            .iter()
            .map(|(session, result)| {
                history_evidence(
                    session,
                    result,
                    "status",
                    format!(
                        "{}; {}",
                        result.status.as_str(),
                        result.error_message.as_deref().unwrap_or("no error detail")
                    ),
                )
            })
            .collect();
        findings.push(make_finding(
            rule("NET-REPEAT-001"),
            format!(
                "{} produced the same {} outcome for {} in {} distinct sessions.",
                key.test_type.as_str(),
                key.status.as_str(),
                key.target.as_deref().unwrap_or("the default gateway"),
                rows.len()
            ),
            ConfidenceCategory::StrongIndication,
            evidence,
            &["A reproducible target-specific or path-specific condition may be present."],
            &["A later successful run or a comparison target."],
            &[key.test_type],
        ));
    }
    findings
}

fn relevant_to_intent(intent: Intent, rule_id: &str) -> bool {
    match intent {
        Intent::NetworkOverview => true,
        Intent::Dns => {
            rule_id.starts_with("NET-DNS")
                || rule_id.starts_with("NET-TIMEOUT")
                || rule_id.starts_with("NET-REPEAT")
        }
        Intent::Gateway => {
            rule_id.starts_with("NET-GATEWAY")
                || rule_id.starts_with("NET-TIMEOUT")
                || rule_id.starts_with("NET-REPEAT")
        }
        Intent::Latency => {
            rule_id.starts_with("NET-LATENCY")
                || rule_id.starts_with("NET-TIMEOUT")
                || rule_id.starts_with("NET-REPEAT")
        }
        Intent::Connectivity => {
            rule_id.starts_with("NET-DNS")
                || rule_id.starts_with("NET-GATEWAY")
                || rule_id.starts_with("NET-TIMEOUT")
                || rule_id.starts_with("NET-REPEAT")
        }
        Intent::SystemInventory => rule_id.starts_with("SYS-INVENTORY"),
        Intent::RepeatedFailures => {
            rule_id.starts_with("NET-REPEAT") || rule_id.starts_with("NET-TIMEOUT")
        }
        Intent::Unsupported => false,
    }
}

fn suggested_questions(sessions: &[HistorySession]) -> Vec<SuggestedQuestion> {
    let mut suggestions = Vec::new();
    let target = sessions
        .iter()
        .flat_map(|session| session.results.iter())
        .find_map(|result| result.target_host.clone());
    let has_test = |kind| {
        sessions.iter().any(|session| {
            session
                .results
                .iter()
                .any(|result| result.test_type == kind)
        })
    };
    let latest_latency_incomplete = sessions
        .iter()
        .flat_map(|session| session.results.iter())
        .find(|result| result.test_type == TestType::Latency)
        .is_some_and(|result| result.status != ResultStatus::Success);
    if !has_test(TestType::Dns)
        || sessions.iter().any(|session| {
            session.results.iter().any(|result| {
                result.test_type == TestType::Dns && result.status != ResultStatus::Success
            })
        })
    {
        suggestions.push(SuggestedQuestion {
            prompt: "Could DNS resolution be the issue?".into(),
            reason: "Review recorded DNS checks or perform a fresh resolution.".into(),
            test_type: Some(TestType::Dns),
            target_host: target.clone(),
            purpose: Some(
                "Resolve the chosen hostname through Windows' configured resolver.".into(),
            ),
        });
    }
    if !has_test(TestType::Gateway)
        || sessions.iter().any(|session| {
            session.results.iter().any(|result| {
                result.test_type == TestType::Gateway && result.status != ResultStatus::Success
            })
        })
    {
        suggestions.push(SuggestedQuestion {
            prompt: "Did the default gateway respond?".into(),
            reason: "Review local gateway reachability evidence.".into(),
            test_type: Some(TestType::Gateway),
            target_host: None,
            purpose: Some("Send one bounded IPv4 ICMP echo to a reported gateway.".into()),
        });
    }
    if latest_latency_incomplete || !has_test(TestType::Latency) {
        suggestions.push(SuggestedQuestion {
            prompt: "Are the network latency samples consistent?".into(),
            reason: "A fresh five-sample TCP run can add or update latency evidence.".into(),
            test_type: Some(TestType::Latency),
            target_host: target.clone(),
            purpose: Some("Measure five TCP connection setup attempts on port 443.".into()),
        });
    }
    suggestions.truncate(3);
    suggestions
}

fn no_matching_evidence(intent: Intent) -> Finding {
    let text = match intent {
        Intent::Dns => "No DNS failure in the available history was paired with a successful gateway check in the same session.",
        Intent::Gateway => "No unsuccessful gateway result was found in the available diagnostic history.",
        Intent::Latency => "No latency result met the rule's minimum evidence requirement.",
        Intent::Connectivity => "No recorded result matched the supported connectivity rules.",
        Intent::SystemInventory => "No requested system inventory fields were reported unavailable.",
        Intent::RepeatedFailures => "No identical non-success result appeared in at least three distinct recorded sessions.",
        _ => "No current finding matched the selected supported question.",
    };
    make_finding(
        rule("INTEL-EVIDENCE-001"),
        text.into(),
        ConfidenceCategory::InsufficientEvidence,
        vec![],
        &[],
        &["A relevant completed diagnostic test or a larger history window."],
        &[],
    )
}

pub(crate) fn analyze(
    question: &str,
    inventory: Result<&SystemInventory, &str>,
    history: Result<&[HistorySession], &str>,
) -> Result<IntelligenceResponse, &'static str> {
    let intent = parse_intent(question)?;
    let history_available = history.is_ok();
    let sessions = history.unwrap_or_default();
    let all_findings = rule_findings(sessions, history_available, inventory);
    let mut findings: Vec<Finding> = if intent == Intent::Unsupported {
        Vec::new()
    } else {
        all_findings
            .into_iter()
            .filter(|finding| relevant_to_intent(intent, &finding.rule_id))
            .collect()
    };
    if intent != Intent::Unsupported && findings.is_empty() {
        findings.push(no_matching_evidence(intent));
    }
    let summary = if intent == Intent::Unsupported {
        "This question is outside ORBIT's deterministic rule set. The rules did not generate a diagnosis.".to_owned()
    } else if findings
        .iter()
        .any(|finding| finding.confidence == ConfidenceCategory::InsufficientEvidence)
    {
        "The available evidence is incomplete for a firm conclusion. Review the missing evidence and suggested checks below.".to_owned()
    } else if findings.is_empty() {
        "No matching issue was established by the available local evidence.".to_owned()
    } else {
        format!(
            "{} evidence-based finding{} matched this question.",
            findings.len(),
            if findings.len() == 1 { "" } else { "s" }
        )
    };
    Ok(IntelligenceResponse {
        question: question.trim().to_owned(),
        intent: intent.label().to_owned(),
        supported: intent != Intent::Unsupported,
        summary,
        inventory_collected_at: inventory.ok().map(|item| item.collected_at.clone()),
        history_available,
        findings,
        suggested_questions: suggested_questions(sessions),
    })
}

#[tauri::command]
pub fn analyze_intelligence(
    question: String,
    db: State<'_, Database>,
) -> Result<IntelligenceResponse, String> {
    // Validate before reading either data source to reject malformed IPC input consistently.
    parse_intent(&question).map_err(str::to_owned)?;
    let inventory = crate::collect_system_inventory();
    let sessions = history::load_recent_sessions(&db.0);
    let response = analyze(
        &question,
        inventory
            .as_ref()
            .map_err(|_| "System inventory collection failed."),
        sessions
            .as_deref()
            .map_err(|_| "Diagnostic history is unavailable."),
    )
    .map_err(str::to_owned)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Computer, InventoryField, Memory, OperatingSystem, Processor};
    use chrono::Utc;
    use serde_json::json;

    fn field(value: &str) -> InventoryField<String> {
        InventoryField {
            value: Some(value.into()),
            availability: "available",
            reason: None,
        }
    }
    fn system(missing: bool) -> SystemInventory {
        let missing_field = || InventoryField::<String> {
            value: None,
            availability: "unavailable",
            reason: Some("Windows did not report this value.".into()),
        };
        SystemInventory {
            collected_at: Utc::now().to_rfc3339(),
            status: if missing { "partial" } else { "complete" },
            operating_system: OperatingSystem {
                product_name: field("Windows"),
                edition: field("Pro"),
                display_version: field("25H2"),
                build: field("26200"),
                architecture: field("x86_64"),
            },
            computer: Computer {
                name: field("ORBIT-TEST"),
                manufacturer: if missing {
                    missing_field()
                } else {
                    field("Vendor")
                },
                model: field("Model"),
            },
            processor: Processor {
                model: field("CPU"),
                logical_processors: InventoryField {
                    value: Some(8),
                    availability: "available",
                    reason: None,
                },
            },
            memory: Memory {
                installed_bytes: InventoryField {
                    value: Some(8_000_000_000),
                    availability: "available",
                    reason: None,
                },
            },
        }
    }
    fn diagnostic(
        test_type: TestType,
        status: ResultStatus,
        target: Option<&str>,
        error: Option<&str>,
        details: Value,
    ) -> DiagnosticResult {
        DiagnosticResult {
            test_type,
            target_host: target.map(str::to_owned),
            status,
            duration_ms: 800,
            details,
            error_message: error.map(str::to_owned),
            completed_at: Some(Utc::now().to_rfc3339()),
        }
    }
    fn session(id: &str, results: Vec<DiagnosticResult>) -> HistorySession {
        let now = Utc::now().to_rfc3339();
        HistorySession {
            id: id.into(),
            started_at: now.clone(),
            completed_at: now,
            duration_ms: 2_000,
            results,
        }
    }
    fn inventory_result(system: &SystemInventory) -> Result<&SystemInventory, &str> {
        Ok(system)
    }

    #[test]
    fn registry_documents_stable_rules_and_evidence_requirements() {
        let ids: Vec<_> = RULE_REGISTRY.iter().map(|entry| entry.id).collect();
        assert_eq!(
            ids,
            [
                "NET-DNS-001",
                "NET-GATEWAY-001",
                "NET-LATENCY-001",
                "NET-TIMEOUT-001",
                "SYS-INVENTORY-001",
                "NET-REPEAT-001",
                "INTEL-EVIDENCE-001",
                "INTEL-HISTORY-001"
            ]
        );
        assert!(RULE_REGISTRY.iter().all(|entry| !entry.condition.is_empty()
            && !entry.evidence_required.is_empty()
            && !entry.limitations.is_empty()
            && !entry.weakening_evidence.is_empty()));
    }

    #[test]
    fn dns_failure_with_same_session_gateway_success_is_observed_without_root_cause_claim() {
        let inventory = system(false);
        let history = vec![session(
            "dns-gateway",
            vec![
                diagnostic(
                    TestType::Dns,
                    ResultStatus::Failed,
                    Some("bad.example"),
                    Some("lookup failed"),
                    json!({}),
                ),
                diagnostic(
                    TestType::Gateway,
                    ResultStatus::Success,
                    Some("192.0.2.1"),
                    None,
                    json!({}),
                ),
            ],
        )];
        let result = analyze(
            "Could DNS be the issue?",
            inventory_result(&inventory),
            Ok(&history),
        )
        .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "NET-DNS-001")
            .unwrap();
        assert_eq!(finding.confidence, ConfidenceCategory::ConfirmedObservation);
        assert_eq!(finding.evidence.len(), 2);
        assert!(finding
            .evidence
            .iter()
            .all(
                |evidence| evidence.session_id.as_deref() == Some("dns-gateway")
                    && evidence.source == EvidenceSource::DiagnosticHistory
            ));
        assert_eq!(finding.risk_level, RiskLevel::Low);
        assert!(finding
            .possible_causes
            .iter()
            .any(|cause| cause.contains("resolver")));
        assert!(finding
            .limitations
            .iter()
            .any(|item| item.contains("does not prove internet")));
    }

    #[test]
    fn dns_and_gateway_from_different_sessions_are_contradictory_not_correlated() {
        let inventory = system(false);
        let history = vec![
            session(
                "dns",
                vec![diagnostic(
                    TestType::Dns,
                    ResultStatus::Failed,
                    Some("bad.example"),
                    None,
                    json!({}),
                )],
            ),
            session(
                "gateway",
                vec![diagnostic(
                    TestType::Gateway,
                    ResultStatus::Success,
                    Some("192.0.2.1"),
                    None,
                    json!({}),
                )],
            ),
        ];
        let result = analyze("DNS resolution", inventory_result(&inventory), Ok(&history)).unwrap();
        assert!(!result
            .findings
            .iter()
            .any(|finding| finding.rule_id == "NET-DNS-001"));
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.confidence == ConfidenceCategory::InsufficientEvidence));
    }

    #[test]
    fn gateway_failure_is_an_observation_not_proof_that_the_network_is_down() {
        let inventory = system(false);
        let history = vec![session(
            "gateway-timeout",
            vec![diagnostic(
                TestType::Gateway,
                ResultStatus::Inconclusive,
                Some("192.0.2.1"),
                Some("ICMP did not respond"),
                json!({"gateway":"192.0.2.1"}),
            )],
        )];
        let result = analyze(
            "Did the gateway respond?",
            inventory_result(&inventory),
            Ok(&history),
        )
        .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "NET-GATEWAY-001")
            .unwrap();
        assert_eq!(finding.confidence, ConfidenceCategory::ConfirmedObservation);
        assert!(finding
            .limitations
            .iter()
            .any(|item| item.contains("block ICMP")));
    }

    #[test]
    fn latency_rules_distinguish_high_complete_interrupted_and_too_few_samples() {
        let inventory = system(false);
        let samples = json!({"sampleCount":5,"samples":[{"status":"success","latencyMs":180},{"status":"success","latencyMs":200},{"status":"success","latencyMs":190},{"status":"success","latencyMs":210},{"status":"success","latencyMs":220}]});
        let history = vec![session(
            "slow",
            vec![diagnostic(
                TestType::Latency,
                ResultStatus::Success,
                Some("host.test"),
                None,
                samples,
            )],
        )];
        let result = analyze(
            "Is my network latency high?",
            inventory_result(&inventory),
            Ok(&history),
        )
        .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "NET-LATENCY-001")
            .unwrap();
        assert_eq!(finding.confidence, ConfidenceCategory::StrongIndication);
        assert!(finding.evidence[0].observed.contains("180"));
        assert_eq!(finding.evidence[0].session_id.as_deref(), Some("slow"));
        let incomplete = json!({"sampleCount":5,"samples":[{"status":"success","latencyMs":15},{"status":"timeout"}]});
        let history = vec![session(
            "partial-latency",
            vec![diagnostic(
                TestType::Latency,
                ResultStatus::Inconclusive,
                Some("host.test"),
                Some("partial samples"),
                incomplete,
            )],
        )];
        let result = analyze("latency", inventory_result(&inventory), Ok(&history)).unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "NET-LATENCY-001")
            .unwrap();
        assert_eq!(finding.confidence, ConfidenceCategory::InsufficientEvidence);
    }

    #[test]
    fn timeout_and_unavailable_interface_results_are_classified_as_inconclusive_evidence() {
        let inventory = system(false);
        let history = vec![
            session(
                "tcp-timeout",
                vec![diagnostic(
                    TestType::Connectivity,
                    ResultStatus::Inconclusive,
                    Some("host.test"),
                    Some("TCP timed out"),
                    json!({}),
                )],
            ),
            session(
                "no-interface",
                vec![diagnostic(
                    TestType::Gateway,
                    ResultStatus::Inconclusive,
                    None,
                    Some("Windows did not report an IPv4 default gateway."),
                    json!({}),
                )],
            ),
            session(
                "failed-timeout",
                vec![diagnostic(
                    TestType::Connectivity,
                    ResultStatus::Failed,
                    Some("host.test"),
                    Some("TCP port 443 did not respond within 3 seconds."),
                    json!({}),
                )],
            ),
        ];
        let result = analyze(
            "network overview",
            inventory_result(&inventory),
            Ok(&history),
        )
        .unwrap();
        let findings: Vec<_> = result
            .findings
            .iter()
            .filter(|finding| finding.rule_id == "NET-TIMEOUT-001")
            .collect();
        assert_eq!(findings.len(), 3);
        assert!(findings
            .iter()
            .all(|finding| finding.confidence == ConfidenceCategory::ConfirmedObservation));
    }

    #[test]
    fn missing_system_fields_and_collection_errors_have_explicit_evidence() {
        let inventory = system(true);
        let result = analyze(
            "Which system inventory fields are missing?",
            inventory_result(&inventory),
            Ok(&[]),
        )
        .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "SYS-INVENTORY-001")
            .unwrap();
        assert_eq!(finding.confidence, ConfidenceCategory::ConfirmedObservation);
        assert_eq!(finding.evidence[0].field, "computer.manufacturer");
        let result = analyze("system info", Err("unavailable"), Ok(&[])).unwrap();
        assert_eq!(
            result.findings[0].confidence,
            ConfidenceCategory::ConfirmedObservation
        );
        assert!(result.findings[0].evidence[0]
            .observed
            .contains("returned an error"));
    }

    #[test]
    fn repeated_identical_non_success_runs_are_linked_to_all_sessions_and_new_success_weakens_them()
    {
        let inventory = system(false);
        let failed = || {
            diagnostic(
                TestType::Dns,
                ResultStatus::Failed,
                Some("host.test"),
                Some("lookup failed"),
                json!({}),
            )
        };
        let history = vec![
            session("one", vec![failed()]),
            session("two", vec![failed()]),
            session("three", vec![failed()]),
        ];
        let result = analyze(
            "Are failures repeating?",
            inventory_result(&inventory),
            Ok(&history),
        )
        .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "NET-REPEAT-001")
            .unwrap();
        assert_eq!(finding.evidence.len(), 3);
        assert_eq!(finding.risk_level, RiskLevel::Low);
        let mut recovered = history.clone();
        recovered.insert(
            0,
            session(
                "four",
                vec![diagnostic(
                    TestType::Dns,
                    ResultStatus::Success,
                    Some("host.test"),
                    None,
                    json!({"addresses":["192.0.2.2"]}),
                )],
            ),
        );
        let result = analyze(
            "repeated DNS failures?",
            inventory_result(&inventory),
            Ok(&recovered),
        )
        .unwrap();
        assert!(!result
            .findings
            .iter()
            .any(|finding| finding.rule_id == "NET-REPEAT-001"));
    }

    #[test]
    fn unknown_intent_and_malformed_questions_are_distinct() {
        let inventory = system(false);
        let result = analyze("Why is my CPU hot?", inventory_result(&inventory), Ok(&[])).unwrap();
        assert!(!result.supported);
        assert_eq!(result.intent, "unsupported");
        assert!(result.findings.is_empty());
        assert!(parse_intent(" ").is_err());
        assert!(parse_intent(&"a".repeat(MAX_QUESTION_CHARS + 1)).is_err());
        assert!(parse_intent("network\nstatus").is_err());
    }

    #[test]
    fn search_examples_map_to_supported_rule_intents() {
        assert_eq!(
            parse_intent("Why is my internet slow?").unwrap(),
            Intent::Latency
        );
        assert_eq!(
            parse_intent("Explain my last diagnostic result").unwrap(),
            Intent::NetworkOverview
        );
    }

    #[test]
    fn unavailable_history_is_explicit_and_read_only_recommendations_do_not_mutate_evidence() {
        let inventory = system(false);
        let history = vec![session(
            "before",
            vec![diagnostic(
                TestType::Dns,
                ResultStatus::Failed,
                Some("host.test"),
                None,
                json!({}),
            )],
        )];
        let before = history.clone();
        let result = analyze(
            "network overview",
            inventory_result(&inventory),
            Err("database unavailable"),
        )
        .unwrap();
        assert!(!result.history_available);
        assert_eq!(history, before);
        let failed = result
            .findings
            .iter()
            .find(|finding| finding.rule_id == "INTEL-HISTORY-001")
            .unwrap();
        assert_eq!(failed.confidence, ConfidenceCategory::InsufficientEvidence);
        assert!(result
            .suggested_questions
            .iter()
            .all(|suggestion| suggestion.test_type.is_some()));
    }

    #[test]
    fn reading_unavailable_history_returns_a_sanitized_error() {
        let path = std::env::temp_dir().join(format!(
            "orbit-missing-{}/child.sqlite",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        assert!(history::load_recent_sessions(&path).is_err());
    }
}
