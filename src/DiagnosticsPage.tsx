import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { DiagnosticResult, HistorySession, ResultStatus, TestType } from './diagnostics.types';

type Props = { onNavigateHistory: () => void };
const tests: { type: TestType; title: string; description: string; needsHost: boolean }[] = [
  { type: 'connectivity', title: 'Connectivity', description: 'Open a TCP connection to port 443. This checks one route and service port only.', needsHost: true },
  { type: 'dns', title: 'DNS resolution', description: 'Resolve the hostname using the Windows-configured resolver.', needsHost: true },
  { type: 'gateway', title: 'Default gateway', description: 'Send one bounded IPv4 ICMP echo to a reported default gateway.', needsHost: false },
  { type: 'latency', title: 'Latency samples', description: 'Measure five TCP connection attempts to port 443; this is not ICMP ping.', needsHost: true },
];

function statusLabel(status: ResultStatus | 'idle' | 'running' | 'error' | undefined): string {
  if (!status) return 'Not run';
  return ({ success: 'Passed', failed: 'Failed', inconclusive: 'Inconclusive', idle: 'Not run', running: 'Running', error: 'Error' })[status];
}

function formatDetails(result: DiagnosticResult): string {
  const d = result.details;
  if (result.testType === 'dns') return Array.isArray(d.addresses) ? d.addresses.join('\n') : 'No addresses returned.';
  if (result.testType === 'latency') {
    const samples = Array.isArray(d.samples) ? d.samples as Record<string, unknown>[] : [];
    const stats = d.statistics as Record<string, unknown> | undefined;
    const lines = [`Successful samples: ${String(d.successfulSamples ?? 0)}/${String(d.sampleCount ?? samples.length)}`, ...samples.map((sample, index) => `Sample ${index + 1}: ${sample.status === 'success' ? `${sample.latencyMs} ms` : sample.status === 'timeout' ? 'Timed out' : 'Connection failed'}`)];
    if (stats && typeof stats.minimumMs === 'number') lines.push(`Min ${stats.minimumMs} ms · Avg ${Number(stats.averageMs).toFixed(1)} ms · Max ${stats.maximumMs} ms`);
    return lines.join('\n') || 'No samples returned.';
  }
  if (result.testType === 'gateway') return typeof d.gateway === 'string' ? `${d.gateway} · ${String(d.method ?? 'ICMP')}` : 'No default gateway reported.';
  return typeof d.address === 'string' ? `${d.address}:${String(d.port ?? 443)} · ${String(d.connectMs ?? result.durationMs)} ms` : 'No connection established.';
}

export default function DiagnosticsPage({ onNavigateHistory }: Props) {
  const [host, setHost] = useState('example.com');
  const [results, setResults] = useState<Partial<Record<TestType, DiagnosticResult>>>({});
  const [running, setRunning] = useState<TestType | 'all' | null>(null);
  const [progress, setProgress] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [lastRun, setLastRun] = useState<string | null>(null);
  const [lastSession, setLastSession] = useState<HistorySession | null>(null);

  async function run(selected: TestType | 'all') {
    if (running) return;
    const target = host.trim();
    if (selected !== 'gateway' && !target) { setError('Enter a hostname before running this test.'); return; }
    const sequence = selected === 'all' ? tests.map(test => test.type) : [selected];
    setRunning(selected); setError(null); setProgress('Preparing diagnostic session…');
    const startedAt = new Date().toISOString(); const start = performance.now(); const resultsForSession: DiagnosticResult[] = [];
    try {
      for (let index = 0; index < sequence.length; index++) {
        const type = sequence[index];
        setProgress(selected === 'all' ? `Running ${index + 1} of ${sequence.length}: ${tests.find(item => item.type === type)?.title}` : `Running ${tests.find(item => item.type === type)?.title}…`);
        const result = await invoke<DiagnosticResult>('run_diagnostic_test', { request: { testType: type, targetHost: type === 'gateway' ? null : target } });
        resultsForSession.push(result); setResults(previous => ({ ...previous, [type]: result }));
      }
      const completedAt = new Date().toISOString(); const id = crypto.randomUUID();
      const session: HistorySession = { id, startedAt, completedAt, durationMs: Math.round(performance.now() - start), results: resultsForSession };
      await invoke('save_diagnostic_session', { request: session });
      setLastSession(session); setLastRun(completedAt); setProgress('');
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError('ORBIT could not complete or save this diagnostic run. Check the local app service and try again.');
      console.error('[ORBIT] Diagnostic operation failed.', { error: message });
      setProgress('');
    } finally { setRunning(null); }
  }

  async function exportReport() {
    if (!lastSession) return;
    try {
      const path = await save({ defaultPath: `ORBIT-diagnostics-${lastSession.startedAt.slice(0, 10)}.html`, filters: [{ name: 'HTML report', extensions: ['html'] }] });
      if (!path) return;
      await invoke('export_diagnostic_report', { sessionId: lastSession.id, outputPath: path });
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError('The report could not be exported to the selected destination.');
      console.error('[ORBIT] Report export failed.', { error: message });
    }
  }

  return <>
    <div className="page-heading"><div><div className="eyebrow">WORKSPACE / DIAGNOSTICS</div><h1>Network diagnostics</h1><p className="page-subtitle">Bounded, read-only checks. Results are evidence from a single route, not a guarantee about a remote service.</p></div><button className="refresh-button primary-action" onClick={() => void run('all')} disabled={running !== null}>{running === 'all' ? <><span className="spinner" /> Running tests…</> : 'Run all tests'}</button></div>
    {error && <div className="error-banner" role="alert"><span className="state-icon">!</span><div><strong>Diagnostic error</strong><span>{error}</span></div><button onClick={() => { setError(null); void run('all'); }} disabled={running !== null}>Try again</button></div>}
    <section className="diagnostic-controls" aria-label="Diagnostic target"><label htmlFor="diagnostic-host">Target hostname</label><input id="diagnostic-host" value={host} onChange={event => setHost(event.target.value)} maxLength={253} autoComplete="url" spellCheck={false} disabled={running !== null} aria-describedby="target-help"/><span id="target-help">Use a hostname such as example.com; do not include a scheme, path, or port.</span><button className="text-button" onClick={onNavigateHistory}>View history →</button></section>
    {running && <div className="run-progress" role="status" aria-live="polite"><span className="spinner" />{progress}<span className="progress-limit">Timeouts: DNS 4 s · TCP 3 s · Gateway 1.2 s</span></div>}
    <div className="diagnostic-list">
      {tests.map(test => {
        const result = results[test.type]; const isRunning = running === test.type || running === 'all' && progress.toLowerCase().includes(test.title.toLowerCase());
        return <article className="diagnostic-card" key={test.type}>
          <div className="diagnostic-main"><div className="diagnostic-title"><span className={`result-mark ${result?.status ?? (isRunning ? 'running' : 'idle')}`} aria-hidden="true">{isRunning ? <span className="spinner" /> : result?.status === 'success' ? '✓' : result?.status === 'failed' ? '×' : result?.status === 'inconclusive' ? '?' : '—'}</span><div><h2>{test.title}</h2><p>{test.description}</p></div></div><div className="diagnostic-actions"><span className={`result-status ${result?.status ?? (isRunning ? 'running' : 'idle')}`}>{isRunning ? 'Running' : statusLabel(result?.status)}</span><button className="small-action" onClick={() => void run(test.type)} disabled={running !== null} aria-label={`Run ${test.title} test`}>{isRunning ? 'Running…' : result ? 'Rerun' : 'Run test'}</button></div></div>
          {result && <details className="result-details"><summary>Result details</summary><div className="result-body"><div><span>Target</span><strong>{result.targetHost ?? 'Default gateway'}</strong></div><div><span>Duration</span><strong>{result.durationMs} ms</strong></div><div><span>Completed</span><strong>{result.completedAt ? new Date(result.completedAt).toLocaleString() : 'Unavailable'}</strong></div><pre>{formatDetails(result)}</pre>{result.errorMessage && <p className="result-guidance"><strong>{result.status === 'failed' ? 'Next step' : 'Interpretation'}:</strong> {result.errorMessage}</p>}</div></details>}
        </article>;
      })}
    </div>
    <div className="diagnostic-footer"><span>{lastRun ? `Last run ${new Date(lastRun).toLocaleString()} · ${lastSession?.durationMs ?? 0} ms` : 'Completed sessions are stored locally in diagnostic history.'}</span><div>{lastSession && <button className="text-button" onClick={() => void exportReport()}>Export last session</button>}<button className="text-button" onClick={onNavigateHistory}>Open history →</button></div></div>
    <p className="diagnostic-note">Gateway reachability uses ICMP. Firewalls and network policies can block echo requests, so a missing reply is reported as inconclusive.</p>
  </>;
}
