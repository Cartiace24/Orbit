use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use tauri::State;
use tokio::{
    net::{lookup_host, TcpStream},
    time::timeout,
};

use crate::history::Database;

const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ICMP_TIMEOUT_MS: u32 = 1_200;
const LATENCY_SAMPLES: usize = 5;
const LATENCY_PAUSE: Duration = Duration::from_millis(200);
const MAX_HOST_LENGTH: usize = 253;
static DIAGNOSTIC_PERMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TestType {
    Connectivity,
    Dns,
    Gateway,
    Latency,
}

impl TestType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connectivity => "connectivity",
            Self::Dns => "dns",
            Self::Gateway => "gateway",
            Self::Latency => "latency",
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ResultStatus {
    Success,
    Failed,
    Inconclusive,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

fn tcp_probe_status(connected: bool, timed_out: bool) -> ResultStatus {
    if connected {
        ResultStatus::Success
    } else if timed_out {
        ResultStatus::Inconclusive
    } else {
        ResultStatus::Failed
    }
}

fn latency_status(successful: usize, attempted: usize) -> ResultStatus {
    if attempted > 0 && successful == attempted {
        ResultStatus::Success
    } else {
        ResultStatus::Inconclusive
    }
}

fn latency_statistics(samples: &[u64]) -> Option<(u64, f64, u64)> {
    if samples.is_empty() {
        return None;
    }
    Some((
        *samples.iter().min()?,
        samples.iter().sum::<u64>() as f64 / samples.len() as f64,
        *samples.iter().max()?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub test_type: TestType,
    pub target_host: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResult {
    pub test_type: TestType,
    pub target_host: Option<String>,
    pub status: ResultStatus,
    pub duration_ms: u64,
    pub details: Value,
    pub error_message: Option<String>,
    pub completed_at: Option<String>,
}

pub fn validate_hostname(host: &str) -> Result<String, String> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty()
        || host.len() > MAX_HOST_LENGTH
        || !host.is_ascii()
        || host.contains('/')
        || host.contains('@')
        || host.contains(':')
    {
        return Err("Enter a valid hostname without a scheme, path, or port.".into());
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host.to_ascii_lowercase());
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return Err("Enter a valid DNS hostname (letters, numbers, and internal hyphens).".into());
    }
    Ok(host.to_ascii_lowercase())
}

fn result(
    test_type: TestType,
    target_host: Option<String>,
    status: ResultStatus,
    started: Instant,
    details: Value,
    error_message: Option<&str>,
) -> DiagnosticResult {
    DiagnosticResult {
        test_type,
        target_host,
        status,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        details,
        error_message: error_message.map(str::to_owned),
        completed_at: Some(Utc::now().to_rfc3339()),
    }
}

async fn resolve(host: &str) -> Result<Vec<IpAddr>, ResultStatus> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    match timeout(DNS_TIMEOUT, lookup_host((host, 443))).await {
        Err(_) => Err(ResultStatus::Inconclusive),
        Ok(Err(_)) => Err(ResultStatus::Failed),
        Ok(Ok(addresses)) => {
            let mut ips: Vec<IpAddr> = addresses.map(|address| address.ip()).collect();
            ips.sort();
            ips.dedup();
            if ips.is_empty() {
                Err(ResultStatus::Failed)
            } else {
                Ok(ips)
            }
        }
    }
}

async fn execute(request: TestRequest) -> Result<DiagnosticResult, String> {
    let started = Instant::now();
    let needs_host = matches!(
        request.test_type,
        TestType::Connectivity | TestType::Dns | TestType::Latency
    );
    let host = if needs_host {
        Some(validate_hostname(
            request.target_host.as_deref().unwrap_or_default(),
        )?)
    } else {
        None
    };
    match request.test_type {
        TestType::Dns => {
            let host = host.expect("validated target");
            let lookup_started = Instant::now();
            match resolve(&host).await {
                Ok(ips) => Ok(result(request.test_type, Some(host), ResultStatus::Success, started, json!({"addresses": ips, "resolutionMs": lookup_started.elapsed().as_millis()}), None)),
                Err(ResultStatus::Inconclusive) => Ok(result(request.test_type, Some(host), ResultStatus::Inconclusive, started, json!({}), Some("DNS lookup exceeded the 4 second limit."))),
                Err(_) => Ok(result(request.test_type, Some(host), ResultStatus::Failed, started, json!({}), Some("The hostname could not be resolved. Check the spelling or try again later."))),
            }
        }
        TestType::Connectivity => {
            let host = host.expect("validated target");
            let ips = match resolve(&host).await {
                Ok(ips) => ips,
                Err(ResultStatus::Inconclusive) => return Ok(result(request.test_type, Some(host), ResultStatus::Inconclusive, started, json!({}), Some("Name resolution exceeded the 4 second limit."))),
                Err(_) => return Ok(result(request.test_type, Some(host), ResultStatus::Failed, started, json!({}), Some("The hostname could not be resolved; this does not establish whether the remote server is online."))),
            };
            let address = SocketAddr::new(ips[0], 443);
            let connect_started = Instant::now();
            match timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
                Ok(Ok(_stream)) => Ok(result(request.test_type, Some(host), tcp_probe_status(true, false), started, json!({"address": address.ip(), "port": 443, "connectMs": connect_started.elapsed().as_millis()}), None)),
                Err(_) => Ok(result(request.test_type, Some(host), tcp_probe_status(false, true), started, json!({"address": address.ip(), "port": 443}), Some("TCP port 443 did not respond within 3 seconds. Filtering or a slow route can produce this result."))),
                Ok(Err(_)) => Ok(result(request.test_type, Some(host), tcp_probe_status(false, false), started, json!({"address": address.ip(), "port": 443}), Some("A TCP connection to port 443 could not be established. This single probe does not prove the server is offline."))),
            }
        }
        TestType::Gateway => gateway_result(started).await,
        TestType::Latency => {
            let host = host.expect("validated target");
            let ips = match resolve(&host).await {
                Ok(ips) => ips,
                Err(ResultStatus::Inconclusive) => {
                    return Ok(result(
                        request.test_type,
                        Some(host),
                        ResultStatus::Inconclusive,
                        started,
                        json!({"samples": [], "sampleCount": LATENCY_SAMPLES}),
                        Some("Name resolution exceeded the 4 second limit."),
                    ))
                }
                Err(_) => {
                    return Ok(result(
                        request.test_type,
                        Some(host),
                        ResultStatus::Failed,
                        started,
                        json!({"samples": [], "sampleCount": LATENCY_SAMPLES}),
                        Some("The hostname could not be resolved."),
                    ))
                }
            };
            let address = SocketAddr::new(ips[0], 443);
            let mut samples = Vec::with_capacity(LATENCY_SAMPLES);
            let mut successes = Vec::with_capacity(LATENCY_SAMPLES);
            for index in 0..LATENCY_SAMPLES {
                if index > 0 {
                    tokio::time::sleep(LATENCY_PAUSE).await;
                }
                let probe = Instant::now();
                let sample = match timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
                    Ok(Ok(_)) => {
                        let ms = probe.elapsed().as_millis() as u64;
                        successes.push(ms);
                        json!({"status":"success", "latencyMs": ms})
                    }
                    Err(_) => {
                        let _status = tcp_probe_status(false, true);
                        json!({"status":"timeout"})
                    }
                    Ok(Err(_)) => {
                        let _status = tcp_probe_status(false, false);
                        json!({"status":"failed"})
                    }
                };
                samples.push(sample);
            }
            let status = latency_status(successes.len(), LATENCY_SAMPLES);
            let statistics = latency_statistics(&successes)
                .map(|(min, avg, max)| json!({"minimumMs":min,"averageMs":avg,"maximumMs":max}))
                .unwrap_or_else(|| json!({}));
            let err = (status != ResultStatus::Success).then_some("Some TCP latency samples did not complete. A timeout or refusal does not establish that the remote server is offline.");
            Ok(result(
                request.test_type,
                Some(host),
                status,
                started,
                json!({"address":address.ip(),"port":443,"sampleCount":LATENCY_SAMPLES,"successfulSamples":successes.len(),"samples":samples,"statistics":statistics}),
                err,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        latency_statistics, latency_status, tcp_probe_status, validate_hostname, ResultStatus,
    };

    #[test]
    fn hostname_validation_accepts_dns_and_ip_and_rejects_command_shapes() {
        assert_eq!(validate_hostname("Example.COM.").unwrap(), "example.com");
        assert_eq!(validate_hostname("192.0.2.1").unwrap(), "192.0.2.1");
        for invalid in [
            "",
            "https://example.com",
            "example.com:443",
            "example.com/path",
            "bad..host",
            "-bad.example",
            "bad host",
        ] {
            assert!(validate_hostname(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn tcp_probe_maps_success_refusal_and_timeout_without_claiming_offline() {
        assert_eq!(tcp_probe_status(true, false), ResultStatus::Success);
        assert_eq!(tcp_probe_status(false, false), ResultStatus::Failed);
        assert_eq!(tcp_probe_status(false, true), ResultStatus::Inconclusive);
    }

    #[test]
    fn latency_reports_statistics_for_successful_samples_and_marks_incomplete() {
        assert_eq!(latency_statistics(&[]), None);
        assert_eq!(latency_statistics(&[10, 20, 30]), Some((10, 20.0, 30)));
        assert_eq!(latency_status(5, 5), ResultStatus::Success);
        assert_eq!(latency_status(3, 5), ResultStatus::Inconclusive);
        assert_eq!(latency_status(0, 5), ResultStatus::Inconclusive);
    }
}

#[tauri::command]
pub async fn run_diagnostic_test(request: TestRequest) -> Result<DiagnosticResult, String> {
    let _permit = DIAGNOSTIC_PERMIT.try_acquire().map_err(|_| {
        "A network diagnostic is already running. Wait for it to finish and retry.".to_owned()
    })?;
    execute(request).await
}

async fn gateway_result(started: Instant) -> Result<DiagnosticResult, String> {
    let gateway = match tokio::task::spawn_blocking(find_default_gateway).await {
        Ok(Ok(gateway)) => gateway,
        Ok(Err(_)) => {
            return Ok(result(
                TestType::Gateway,
                None,
                ResultStatus::Inconclusive,
                started,
                json!({}),
                Some("Windows did not report an IPv4 default gateway."),
            ))
        }
        Err(_) => {
            return Ok(result(
                TestType::Gateway,
                None,
                ResultStatus::Inconclusive,
                started,
                json!({}),
                Some("The network interface query could not be completed."),
            ))
        }
    };
    #[cfg(windows)]
    let reachable = tokio::task::spawn_blocking(move || icmp_echo(gateway))
        .await
        .unwrap_or(false);
    #[cfg(not(windows))]
    let reachable = false;
    if reachable {
        Ok(result(
            TestType::Gateway,
            Some(gateway.to_string()),
            ResultStatus::Success,
            started,
            json!({"gateway":gateway,"method":"IPv4 ICMP echo"}),
            None,
        ))
    } else {
        Ok(result(TestType::Gateway, Some(gateway.to_string()), ResultStatus::Inconclusive, started, json!({"gateway":gateway,"method":"IPv4 ICMP echo"}), Some("The gateway did not answer an ICMP echo within 1.2 seconds. Firewall or ICMP filtering can make this inconclusive.")))
    }
}

fn find_default_gateway() -> Result<Ipv4Addr, ()> {
    ipconfig::get_adapters()
        .map_err(|_| ())?
        .into_iter()
        .flat_map(|adapter| adapter.gateways().to_vec())
        .find_map(|ip| match ip {
            IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
            _ => None,
        })
        .ok_or(())
}

#[cfg(windows)]
fn icmp_echo(address: Ipv4Addr) -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho},
    };
    unsafe {
        let handle = IcmpCreateFile();
        if handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let destination = u32::from_ne_bytes(address.octets());
        let request = b"ORBIT";
        let mut reply = [0u8; 256];
        let count = IcmpSendEcho(
            handle,
            destination,
            request.as_ptr().cast::<c_void>(),
            request.len() as u16,
            std::ptr::null(),
            reply.as_mut_ptr().cast(),
            reply.len() as u32,
            ICMP_TIMEOUT_MS,
        );
        IcmpCloseHandle(handle);
        count > 0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionRequest {
    pub id: String,
    pub started_at: String,
    pub completed_at: String,
    pub duration_ms: u64,
    pub results: Vec<DiagnosticResult>,
}

#[tauri::command]
pub fn save_diagnostic_session(
    request: SaveSessionRequest,
    db: State<'_, Database>,
) -> Result<(), String> {
    crate::history::save_session(&db.0, request)
}
