import type { DiagnosticResult, HistorySession, TestType } from './diagnostics.types';
import { invoke } from '@tauri-apps/api/core';

export type ConfidenceCategory = 'confirmedObservation' | 'strongIndication' | 'possibleCause' | 'insufficientEvidence';
export type RiskLevel = 'informational' | 'low' | 'moderate';
export type EvidenceReference = {
  source: 'systemInventory' | 'diagnosticHistory';
  sessionId: string | null;
  testType: TestType | null;
  field: string;
  observed: string;
  recordedAt: string | null;
};
export type Finding = {
  ruleId: string;
  title: string;
  ruleCondition: string;
  evidenceRequired: string;
  observation: string;
  interpretation: string;
  evidence: EvidenceReference[];
  possibleCauses: string[];
  confidence: ConfidenceCategory;
  riskLevel: RiskLevel;
  recommendedNextSteps: string[];
  missingEvidence: string[];
  suggestedTests: TestType[];
  limitations: string[];
  weakeningEvidence: string[];
};
export type SuggestedQuestion = {
  prompt: string;
  reason: string;
  testType: TestType | null;
  targetHost: string | null;
  purpose: string | null;
};
export type IntelligenceResponse = {
  question: string;
  intent: string;
  supported: boolean;
  summary: string;
  inventoryCollectedAt: string | null;
  historyAvailable: boolean;
  findings: Finding[];
  suggestedQuestions: SuggestedQuestion[];
};

export type LocalAiStatus = { provider: string; available: boolean; models: string[]; message: string };
export type LocalAiEvidence = {
  id: string; source: 'systemInventory' | 'diagnosticHistory'; sessionId: string | null;
  testType: TestType | null; field: string; observed: string; recordedAt: string | null;
};
export type LocalAiExplanation = {
  model: string; answer: string; uncertainty: string; nextSteps: string[];
  evidence: LocalAiEvidence[]; actionIds: TestType[];
};
export type LocalAiAnalysis = { rules: IntelligenceResponse; explanation: LocalAiExplanation };

export type PreviewAction = { testType: TestType; targetHost: string | null; purpose: string };

export async function executeReadOnlyAction(action: PreviewAction): Promise<HistorySession> {
  const startedAt = new Date().toISOString();
  const start = performance.now();
  const result = await invoke<DiagnosticResult>('run_diagnostic_test', {
    request: { testType: action.testType, targetHost: action.targetHost },
  });
  const completedAt = new Date().toISOString();
  const session: HistorySession = {
    id: crypto.randomUUID(), startedAt, completedAt,
    durationMs: Math.round(performance.now() - start), results: [result],
  };
  await invoke('save_diagnostic_session', { request: session });
  return session;
}
