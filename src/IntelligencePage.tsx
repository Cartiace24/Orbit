import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { HistorySession, TestType } from './diagnostics.types';
import { executeReadOnlyAction } from './intelligence.types';
import type { ConfidenceCategory, Finding, IntelligenceResponse, LocalAiAnalysis, LocalAiExplanation, LocalAiStatus, PreviewAction, RiskLevel, SuggestedQuestion } from './intelligence.types';

type Props = { onNavigateHistory: (sessionId?: string) => void; initialQuestion?: string; initialAction?: TestType };
const examples = ['Review my network results', 'Could DNS resolution be the issue?', 'Are the network latency samples consistent?', 'Are failures repeating?', 'What system information is unavailable?'];
const confidenceLabels: Record<ConfidenceCategory, string> = { confirmedObservation: 'Confirmed observation', strongIndication: 'Strong indication', possibleCause: 'Possible cause', insufficientEvidence: 'Insufficient evidence' };
const riskLabels: Record<RiskLevel, string> = { informational: 'Informational', low: 'Low risk', moderate: 'Moderate risk' };
const testLabels: Record<TestType, string> = { connectivity: 'Connectivity', dns: 'DNS resolution', gateway: 'Gateway reachability', latency: 'Latency samples' };

export default function IntelligencePage({ onNavigateHistory, initialQuestion, initialAction }: Props) {
  const [question, setQuestion] = useState(initialQuestion ?? '');
  const [answer, setAnswer] = useState<IntelligenceResponse | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<PreviewAction | null>(() => initialAction ? {
    testType: initialAction, targetHost: initialAction === 'gateway' ? null : '',
    purpose: `Run the existing ${testLabels[initialAction].toLowerCase()} check and add its result to local diagnostic history.`,
  } : null);
  const [previewHost, setPreviewHost] = useState('');
  const [actionRunning, setActionRunning] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionSession, setActionSession] = useState<HistorySession | null>(null);
  const [aiEnabled, setAiEnabled] = useState(() => localStorage.getItem('orbit.localAiEnabled') === 'true');
  const [selectedModel, setSelectedModel] = useState(() => localStorage.getItem('orbit.localAiModel') ?? '');
  const [providerStatus, setProviderStatus] = useState<LocalAiStatus | null>(null);
  const [checkingProvider, setCheckingProvider] = useState(false);
  const [aiExplanation, setAiExplanation] = useState<LocalAiExplanation | null>(null);
  const [aiError, setAiError] = useState<string | null>(null);

  const refreshProvider = useCallback(async () => {
    setCheckingProvider(true);
    try { setProviderStatus(await invoke<LocalAiStatus>('get_local_ai_status')); }
    catch { setProviderStatus({ provider: 'Ollama', available: false, models: [], message: 'ORBIT could not check the local Ollama service.' }); }
    finally { setCheckingProvider(false); }
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void invoke('set_local_ai_enabled', { enabled: aiEnabled }).then(() => {
        if (aiEnabled) void refreshProvider();
      }).catch(() => setProviderStatus({ provider: 'Ollama', available: false, models: [], message: 'ORBIT could not update the local AI setting.' }));
    }, 0);
    return () => window.clearTimeout(timer);
  }, [aiEnabled, refreshProvider]);

  function toggleAi(enabled: boolean) {
    setAiEnabled(enabled); localStorage.setItem('orbit.localAiEnabled', String(enabled));
    setAiExplanation(null); setAiError(null);
  }

  function chooseModel(model: string) { setSelectedModel(model); localStorage.setItem('orbit.localAiModel', model); }

  async function ask(value = question) {
    if (busy) return;
    setBusy(true); setError(null); setAiError(null); setAiExplanation(null); setAnswer(null); setActionError(null);
    try {
      if (aiEnabled && selectedModel && providerStatus?.models.includes(selectedModel)) {
        try {
          const result = await invoke<LocalAiAnalysis>('explain_with_local_ai', { question: value, model: selectedModel });
          setAnswer(result.rules); setAiExplanation(result.explanation);
        } catch (cause) {
          setAiError(typeof cause === 'string' ? cause : 'The local model could not answer. The deterministic rules are still available.');
          setAnswer(await invoke<IntelligenceResponse>('analyze_intelligence', { question: value }));
        }
      } else {
        if (aiEnabled) setAiError('Select a downloaded local model to use AI. Showing deterministic rules.');
        setAnswer(await invoke<IntelligenceResponse>('analyze_intelligence', { question: value }));
      }
      setQuestion(value);
    }
    catch (cause) { const message = cause instanceof Error ? cause.message : String(cause); setError('ORBIT could not analyze local evidence. Check the application service and try again.'); console.error('[ORBIT] Intelligence analysis failed.', { error: message }); }
    finally { setBusy(false); }
  }

  function prepareAction(suggestion: SuggestedQuestion | null, testType?: TestType) {
    const type = suggestion?.testType ?? testType;
    if (!type) return;
    const purpose = suggestion?.purpose ?? `Run the existing ${testLabels[type].toLowerCase()} check and add its result to local diagnostic history.`;
    const host = type === 'gateway' ? null : suggestion?.targetHost ?? '';
    setPreview({ testType: type, targetHost: host, purpose });
    setPreviewHost(host ?? ''); setActionError(null); setActionSession(null);
  }

  async function confirmAction() {
    if (!preview || actionRunning) return;
    const targetHost = preview.testType === 'gateway' ? null : previewHost.trim();
    if (preview.testType !== 'gateway' && !validHostname(targetHost ?? '')) { setActionError('Enter a valid hostname without a scheme, path, or port.'); return; }
    setActionRunning(true); setActionError(null);
    try { const session = await executeReadOnlyAction({ ...preview, targetHost }); setActionSession(session); }
    catch (cause) { const message = cause instanceof Error ? cause.message : String(cause); setActionError('The check could not be completed or saved. Open Network Diagnostics for more detail.'); console.error('[ORBIT] Suggested diagnostic action failed.', { error: message }); }
    finally { setActionRunning(false); }
  }

  return <>
    <div className="page-heading"><div><div className="eyebrow">WORKSPACE / INTELLIGENCE</div><h1>Troubleshooting assistant</h1><p className="page-subtitle">Deterministic local rules interpret verified inventory and saved diagnostic evidence.</p></div><span className="read-only"><span className="status-dot"/> LOCAL RULES</span></div>
    <section className="intel-provider" aria-label="Optional local AI provider"><div className="intel-provider-heading"><div><strong>Optional local AI · Ollama</strong><span>{aiEnabled ? 'Enabled for questions on this device' : 'Off — deterministic rules remain available'}</span></div><label className="intel-toggle"><input type="checkbox" checked={aiEnabled} onChange={event => toggleAi(event.target.checked)} disabled={busy}/><span>Enable local AI</span></label></div>
      {aiEnabled && <><p>When you ask, ORBIT sends your question and a bounded set of system inventory fields, rule findings, and recent diagnostic summaries to Ollama at 127.0.0.1. Computer name and full history are excluded. Choose a downloaded local model; model speed and memory use depend on your hardware.</p><div className="intel-provider-controls"><label htmlFor="intel-model">Installed model</label><select id="intel-model" value={providerStatus?.models.includes(selectedModel) ? selectedModel : ''} onChange={event => chooseModel(event.target.value)} disabled={busy || checkingProvider || !providerStatus?.available}><option value="">Select a local model</option>{providerStatus?.models.map(model => <option key={model} value={model}>{model}</option>)}</select><button className="small-action" onClick={() => void refreshProvider()} disabled={checkingProvider || busy}>{checkingProvider ? 'Checking…' : 'Refresh models'}</button></div><span className="intel-provider-status" role="status">{checkingProvider ? 'Checking the local Ollama service…' : providerStatus?.message ?? 'Provider status has not been checked.'} {providerStatus && !providerStatus.available ? 'Install or start Ollama yourself, then refresh.' : ''}</span></>}
    </section>
    <section className="intel-question" aria-label="Ask a troubleshooting question">
      <label htmlFor="intel-input">Ask about this device or its recorded network tests</label>
      <div className="intel-input-row"><input id="intel-input" value={question} onChange={event => setQuestion(event.target.value)} maxLength={500} placeholder="For example: Could DNS resolution be the issue?" disabled={busy} onKeyDown={event => { if (event.key === 'Enter') void ask(); }} /><button className="refresh-button" onClick={() => void ask()} disabled={busy || !question.trim()}>{busy ? <><span className="spinner"/> Reviewing…</> : 'Analyze evidence'}</button></div>
      <div className="intel-examples"><span>TRY</span>{examples.map(example => <button className="text-button" key={example} onClick={() => void ask(example)} disabled={busy}>{example}</button>)}</div>
    </section>
    {error && <div className="error-banner" role="alert"><span className="state-icon">!</span><div><strong>Analysis unavailable</strong><span>{error}</span></div><button onClick={() => void ask()}>Try again</button></div>}
    {busy && <div className="loading-panel intel-loading" role="status"><span className="spinner large"/><div><strong>{aiEnabled && selectedModel ? `Local model ${selectedModel} is processing` : 'Reviewing local evidence'}</strong><span>Reading current system inventory and recent diagnostic history.</span></div></div>}
    {!answer && !busy && !error && <div className="empty-history intel-welcome"><strong>Ask a troubleshooting question</strong><span>ORBIT always keeps its deterministic findings available. Local AI explanations are optional.</span></div>}
    {aiError && !busy && <div className="intel-ai-error" role="alert"><strong>Local AI unavailable</strong><span>{aiError}</span><button className="text-button" onClick={() => toggleAi(false)}>Use rules only</button></div>}
    {answer && !busy && <>
      {aiExplanation && <section className="intel-ai-answer" aria-label="Local model explanation"><div className="intel-ai-answer-head"><div className="eyebrow">LOCAL MODEL EXPLANATION · {aiExplanation.model}</div><span>Model text is untrusted; verified findings are shown below.</span></div><p>{aiExplanation.answer}</p><div className="intel-ai-uncertainty"><strong>Uncertainty and limits</strong><span>{aiExplanation.uncertainty}</span></div>{aiExplanation.evidence.length > 0 && <div className="intel-ai-references"><strong>Verified references</strong>{aiExplanation.evidence.map(item => <div key={item.id}><span>{item.id} · {item.field}: {item.observed}</span>{item.sessionId && <button className="text-button" onClick={() => onNavigateHistory(item.sessionId!)}>Open result →</button>}</div>)}</div>}{aiExplanation.nextSteps.length > 0 && <TextList title="Suggested next steps from the model" items={aiExplanation.nextSteps}/>}<div className="intel-inline-actions">{aiExplanation.actionIds.map(type => <button className="small-action" key={type} onClick={() => prepareAction(null, type)}>Review {testLabels[type]} action</button>)}</div></section>}
      <div className={`intel-summary ${answer.supported ? '' : 'unsupported'}`} role="status"><span className="notice-mark">{answer.supported ? 'i' : '?'}</span><div><strong>{answer.supported ? 'Evidence review' : 'Outside the supported rules'}</strong><span>{answer.summary}</span></div><small>Inventory {answer.inventoryCollectedAt ? new Date(answer.inventoryCollectedAt).toLocaleString() : 'unavailable'} · History {answer.historyAvailable ? 'available' : 'unavailable'}</small></div>
      {!answer.supported && <div className="intel-limit"><strong>Current scope</strong><p>ORBIT supports questions about DNS results, gateway reachability, connectivity, latency samples, missing inventory fields, and repeated recorded failures. Try a suggested prompt or run a read-only check.</p></div>}
      <div className="intel-results-heading"><div><h2>Findings</h2><p>Observations come from recorded results. Explanations are labeled separately as interpretations.</p></div><button className="text-button" onClick={() => onNavigateHistory()}>Open diagnostic history →</button></div>
      {answer.findings.length === 0 ? <div className="empty-history"><strong>{answer.supported ? 'No matching findings' : 'No findings generated'}</strong><span>{answer.supported ? 'No current evidence matched this rule set.' : 'Select a supported example question to review available evidence.'}</span></div> : <div className="intel-findings">{answer.findings.map((finding, index) => <FindingCard key={`${finding.ruleId}-${index}`} finding={finding} onNavigateHistory={onNavigateHistory} onPreview={type => prepareAction(null, type)}/>)}</div>}
      {answer.suggestedQuestions.length > 0 && <section className="intel-suggestions"><div className="intel-results-heading"><div><h2>Suggested next checks</h2><p>These recommendations do not run until you review and confirm the action.</p></div></div><div className="intel-suggestion-list">{answer.suggestedQuestions.map((suggestion, index) => <article className="intel-suggestion" key={`${suggestion.prompt}-${index}`}><div><strong>{suggestion.prompt}</strong><span>{suggestion.reason}</span></div><button className="small-action" onClick={() => suggestion.testType ? prepareAction(suggestion) : void ask(suggestion.prompt)}>{suggestion.testType ? 'Review action' : 'Ask'}</button></article>)}</div></section>}
    </>}
    {preview && <div className="intel-modal-backdrop" role="presentation"><section className="intel-preview" role="dialog" aria-modal="true" aria-labelledby="action-preview-title"><div className="eyebrow">READ-ONLY ACTION PREVIEW</div><h2 id="action-preview-title">Run {testLabels[preview.testType]}</h2><p>{preview.purpose}</p><dl><div><dt>Check</dt><dd>{testLabels[preview.testType]}</dd></div><div><dt>Target</dt><dd>{preview.testType === 'gateway' ? 'Windows-reported default gateway' : <input aria-label="Hostname to test" maxLength={253} value={previewHost} onChange={event => setPreviewHost(event.target.value)} placeholder="Enter a hostname"/>}</dd></div><div><dt>Effect</dt><dd>One bounded diagnostic request. Result is saved to local History.</dd></div></dl>{actionError && <div className="intel-action-error" role="alert">{actionError}</div>}{actionSession && <div className="intel-action-complete" role="status">Saved: {actionSession.results[0].status} · {actionSession.results[0].durationMs} ms <button className="text-button" onClick={() => onNavigateHistory(actionSession.id)}>Open result</button></div>}<div className="intel-preview-actions"><button className="small-action" onClick={() => setPreview(null)} disabled={actionRunning}>Cancel</button><button className="refresh-button" onClick={() => void confirmAction()} disabled={actionRunning}>{actionRunning ? <><span className="spinner"/> Running…</> : 'Run and save test'}</button></div></section></div>}
  </>;
}

function validHostname(value: string): boolean {
  if (value.length < 1 || value.length > 253 || value.startsWith('.') || value.endsWith('.')) return false;
  return value.split('.').every(label => label.length >= 1 && label.length <= 63 && /^[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(label));
}

function FindingCard({ finding, onNavigateHistory, onPreview }: { finding: Finding; onNavigateHistory: (sessionId?: string) => void; onPreview: (type: TestType) => void }) {
  return <article className="intel-finding"><header><div><span className="intel-rule-id">{finding.ruleId}</span><h3>{finding.title}</h3></div><div className="intel-badges"><span className={`confidence-badge ${finding.confidence}`}>{confidenceLabels[finding.confidence]}</span><span className={`risk-badge ${finding.riskLevel}`}>{riskLabels[finding.riskLevel]}</span></div></header><div className="intel-finding-body"><section><h4>Observed</h4><p>{finding.observation}</p><h4>Interpretation</h4><p>{finding.interpretation}</p></section><section><h4>Rule condition</h4><p>{finding.ruleCondition}</p><h4>Evidence required</h4><p>{finding.evidenceRequired}</p></section>
    {finding.evidence.length > 0 && <section><h4>Evidence</h4><ul className="intel-evidence">{finding.evidence.map((item, index) => <li key={`${item.sessionId}-${item.field}-${index}`}><span><strong>{item.field}</strong><small>{item.observed}{item.recordedAt ? ` · ${new Date(item.recordedAt).toLocaleString()}` : ''}</small></span>{item.sessionId && <button className="text-button" onClick={() => onNavigateHistory(item.sessionId!)}>Open result →</button>}</li>)}</ul></section>}
    {finding.possibleCauses.length > 0 && <TextList title="Possible causes" items={finding.possibleCauses}/>}<TextList title="Recommended next steps" items={finding.recommendedNextSteps}/>{finding.missingEvidence.length > 0 && <TextList title="Missing evidence" items={finding.missingEvidence}/>}<details className="intel-limitations"><summary>Limitations and evidence that would weaken this finding</summary><TextList title="Limitations" items={finding.limitations}/><TextList title="Would weaken this finding" items={finding.weakeningEvidence}/></details>{finding.suggestedTests.length > 0 && <div className="intel-inline-actions">{finding.suggestedTests.map(type => <button className="text-button" key={type} onClick={() => onPreview(type)}>Review {testLabels[type]} action →</button>)}</div>}</div></article>;
}
function TextList({ title, items }: { title: string; items: string[] }) { return items.length ? <section><h4>{title}</h4><ul>{items.map((item, index) => <li key={`${title}-${index}`}>{item}</li>)}</ul></section> : null; }
