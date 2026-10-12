export type TestType = 'connectivity' | 'dns' | 'gateway' | 'latency';
export type ResultStatus = 'success' | 'failed' | 'inconclusive';

export type DiagnosticResult = {
  testType: TestType;
  targetHost: string | null;
  status: ResultStatus;
  durationMs: number;
  details: Record<string, unknown>;
  errorMessage: string | null;
  completedAt: string | null;
};

export type HistorySession = {
  id: string;
  startedAt: string;
  completedAt: string;
  durationMs: number;
  results: DiagnosticResult[];
};
