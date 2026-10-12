import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { HistorySession } from './diagnostics.types';

export default function HistoryPage({ focusSessionId = null }: { focusSessionId?: string | null }) {
  const [sessions, setSessions] = useState<HistorySession[]>([]);
  const [testType, setTestType] = useState(''); const [status, setStatus] = useState('');
  const [loading, setLoading] = useState(true); const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState<string | null>(null);
  async function load() {
    setLoading(true); setError(null);
    try { setSessions(await invoke<HistorySession[]>('get_diagnostic_history', { filter: { testType: testType || null, status: status || null } })); }
    catch (cause) {
      const message = cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : 'Diagnostic history could not be read from local storage.';
      setError(message);
      console.error('[ORBIT] History query failed.', { error: message });
    }
    finally { setLoading(false); }
  }
  async function exportSession(session: HistorySession) {
    if (exporting) return;
    setExporting(session.id); setError(null);
    try {
      const path = await save({ defaultPath: `ORBIT-diagnostics-${session.startedAt.slice(0, 10)}.html`, filters: [{ name: 'HTML report', extensions: ['html'] }] });
      if (path) await invoke('export_diagnostic_report', { sessionId: session.id, outputPath: path });
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : typeof cause === 'string' ? cause : 'The report could not be exported to the selected destination.';
      setError(message);
      console.error('[ORBIT] History report export failed.', { error: message });
    } finally { setExporting(null); }
  }
  useEffect(() => { void load(); }, [testType, status]);
  return <>
    <div className="page-heading"><div><div className="eyebrow">WORKSPACE / HISTORY</div><h1>Diagnostic history</h1><p className="page-subtitle">Locally stored network diagnostic sessions on this device.</p></div><button className="refresh-button" onClick={() => void load()} disabled={loading}>{loading ? 'Loading…' : 'Refresh'}</button></div>
    <div className="history-filters"><label>Test type<select value={testType} onChange={event => setTestType(event.target.value)}><option value="">All tests</option><option value="connectivity">Connectivity</option><option value="dns">DNS resolution</option><option value="gateway">Default gateway</option><option value="latency">Latency</option></select></label><label>Status<select value={status} onChange={event => setStatus(event.target.value)}><option value="">All statuses</option><option value="success">Passed</option><option value="failed">Failed</option><option value="inconclusive">Inconclusive</option></select></label><span>{sessions.length} {sessions.length === 1 ? 'session' : 'sessions'} (maximum 250)</span></div>
    {error && <div className="error-banner" role="alert"><span className="state-icon">!</span><div><strong>History or export error</strong><span>{error}</span></div><button onClick={() => void load()}>Try again</button></div>}
    {loading ? <div className="loading-panel"><span className="spinner large"/><div><strong>Reading diagnostic history</strong><span>Loading completed sessions from this device.</span></div></div> : !sessions.length ? <div className="empty-history"><strong>No matching diagnostic sessions</strong><span>Run one or more checks in Network Diagnostics to create local history.</span></div> : <div className="history-list">{sessions.map(session => <details className="history-session" key={session.id} open={session.id === focusSessionId}><summary><div><strong>{new Date(session.startedAt).toLocaleString()}</strong><span>{session.results.length} {session.results.length === 1 ? 'test' : 'tests'} · {session.durationMs} ms</span></div><span className="history-summary-status">{session.results.map(result => <i key={result.testType} className={`status-pill ${result.status}`} title={`${result.testType}: ${result.status}`}>{result.testType} · {result.status}</i>)}</span></summary><div className="history-detail"><div className="history-export-row"><span>Session {session.id}</span><button className="small-action" onClick={() => void exportSession(session)} disabled={exporting !== null}>{exporting === session.id ? 'Preparing…' : 'Export HTML report'}</button></div>{session.results.map(result => <div className="history-result" key={result.testType}><div><strong>{result.testType}</strong><span className={`result-status ${result.status}`}>{result.status}</span></div><p>Target: {result.targetHost ?? 'Default gateway'} · {result.durationMs} ms · {result.completedAt ? new Date(result.completedAt).toLocaleString() : 'Per-test completion time not recorded in this legacy session'}</p>{result.errorMessage && <p className="result-guidance">{result.errorMessage}</p>}<pre>{JSON.stringify(result.details, null, 2)}</pre></div>)}</div></details>)}</div>}
  </>;
}
