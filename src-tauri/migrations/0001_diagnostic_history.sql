CREATE TABLE diagnostic_sessions (
  id TEXT PRIMARY KEY NOT NULL,
  started_at TEXT NOT NULL,
  completed_at TEXT NOT NULL,
  duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0)
);

CREATE TABLE diagnostic_results (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES diagnostic_sessions(id) ON DELETE CASCADE,
  test_type TEXT NOT NULL CHECK (test_type IN ('connectivity', 'dns', 'gateway', 'latency')),
  target_host TEXT,
  status TEXT NOT NULL CHECK (status IN ('success', 'failed', 'inconclusive')),
  duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
  details_json TEXT NOT NULL,
  error_message TEXT
);

CREATE INDEX diagnostic_results_session_idx ON diagnostic_results(session_id);
CREATE INDEX diagnostic_results_filter_idx ON diagnostic_results(test_type, status);
