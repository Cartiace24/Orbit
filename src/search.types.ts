import type { TestType } from './diagnostics.types';

export type SearchResult = {
  id: string;
  source: 'page' | 'history' | 'finding' | 'action' | 'intelligence';
  destination: 'section' | 'history' | 'intelligence' | 'actionPreview';
  title: string;
  description: string;
  section: string | null;
  sessionId: string | null;
  testType: TestType | null;
  recordedAt: string | null;
  status: string | null;
};

export type SearchResponse = {
  query: string;
  results: SearchResult[];
  historyAvailable: boolean;
  historyNote: string | null;
};
