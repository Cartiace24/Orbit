import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SearchResponse, SearchResult } from './search.types';

const suggestions = [
  'Why is my internet slow?',
  'Show my recent DNS failures',
  'Explain my last diagnostic result',
  'What can I check without changing Windows settings?',
  'Where can I see my system information?',
  'Does ORBIT test connectivity?',
];
const sourceLabels: Record<SearchResult['source'], string> = {
  page: 'ORBIT FEATURE', history: 'SAVED DIAGNOSTIC', finding: 'RULES ENGINE',
  action: 'READ-ONLY ACTION', intelligence: 'INTELLIGENCE',
};
const actionLabels: Record<SearchResult['destination'], string> = {
  section: 'Open page', history: 'View result', intelligence: 'Open in Intelligence', actionPreview: 'Review action',
};

type Props = {
  onClose: () => void;
  onSelect: (result: SearchResult, query: string) => void;
};

export default function SearchDialog({ onClose, onSelect }: Props) {
  const [query, setQuery] = useState('');
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => { inputRef.current?.focus(); }, []);
  useEffect(() => { const previous = document.body.style.overflow; document.body.style.overflow = 'hidden'; return () => { document.body.style.overflow = previous; }; }, []);
  useEffect(() => { document.getElementById(`orbit-search-result-${activeIndex}`)?.scrollIntoView({ block: 'nearest' }); }, [activeIndex, response]);
  useEffect(() => {
    if (!query.trim()) return;
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      try {
        const result = await invoke<SearchResponse>('universal_search', { query });
        if (!cancelled) { setResponse(result); setError(null); setLoading(false); }
      } catch (cause) {
        if (!cancelled) {
          setError(typeof cause === 'string' ? cause : 'Search could not read local ORBIT data. Try again.');
          setResponse(null); setLoading(false);
        }
      }
    }, 240);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [query]);

  function changeQuery(value: string) {
    setQuery(value); setActiveIndex(0); setError(null); setLoading(Boolean(value.trim()));
    if (!value.trim()) setResponse(null);
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLElement>) {
    if (event.key === 'Escape') { event.preventDefault(); onClose(); return; }
    if (event.key === 'Tab') {
      const focusable = [...(dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? [])];
      const first = focusable[0], last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first && last) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last && first) { event.preventDefault(); first.focus(); }
      return;
    }
    if (document.activeElement !== inputRef.current) return;
    const results = response?.results ?? [];
    if (event.key === 'ArrowDown' && results.length) { event.preventDefault(); setActiveIndex(index => (index + 1) % results.length); }
    if (event.key === 'ArrowUp' && results.length) { event.preventDefault(); setActiveIndex(index => (index - 1 + results.length) % results.length); }
    if (event.key === 'Enter' && !loading && results[activeIndex]) { event.preventDefault(); onSelect(results[activeIndex], query); }
  }

  const results = query.trim() && !loading ? response?.results ?? [] : [];
  return <div className="search-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="search-dialog" ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="search-title" onKeyDown={handleKeyDown}>
      <header className="search-header"><div><span className="eyebrow">WORKSPACE / UNIVERSAL SEARCH</span><h2 id="search-title">Search ORBIT</h2></div><button className="search-close" onClick={onClose} aria-label="Close search">Esc <span aria-hidden="true">×</span></button></header>
      <label className="search-input-label" htmlFor="orbit-search-input">Describe a problem, feature, or saved result</label>
      <div className="search-input-wrap"><span aria-hidden="true">⌕</span><input ref={inputRef} id="orbit-search-input" role="combobox" aria-autocomplete="list" aria-expanded={results.length > 0} aria-controls="orbit-search-results" aria-activedescendant={results.length ? `orbit-search-result-${activeIndex}` : undefined} value={query} onChange={event => changeQuery(event.target.value)} maxLength={200} placeholder="Try: Show my recent DNS failures"/><kbd>Ctrl K</kbd></div>
      {!query.trim() ? <div className="search-suggestions"><strong>TRY A SEARCH</strong>{suggestions.map(item => <button key={item} onClick={() => { changeQuery(item); inputRef.current?.focus(); }}>{item}</button>)}</div> : <div className="search-body" id="orbit-search-results" role="listbox" aria-label="Search results">
        {loading && <div className="search-state" role="status"><span className="spinner"/> Searching local ORBIT data…</div>}
        {error && <div className="search-state search-error" role="alert">{error}</div>}
        {!loading && !error && response?.historyNote && <div className="search-history-note" role="status">{response.historyNote}</div>}
        {!loading && !error && response && results.length === 0 && <div className="search-state"><strong>No matching ORBIT result</strong><span>Try a feature name, a recorded test type, or one of the suggested searches. Search does not infer unsupported Windows capabilities.</span></div>}
        {!loading && !error && results.length > 0 && <><div className="search-count">{results.length} {results.length === 1 ? 'result' : 'results'} · local deterministic search</div>{results.map((result, index) => <button id={`orbit-search-result-${index}`} role="option" aria-selected={index === activeIndex} className={`search-result ${index === activeIndex ? 'selected' : ''}`} key={result.id} onMouseEnter={() => setActiveIndex(index)} onClick={() => onSelect(result, query)}><div className="search-result-meta"><span>{sourceLabels[result.source]}</span>{result.status && <span>{result.status}</span>}{result.source === 'history' && <span>{result.recordedAt ? new Date(result.recordedAt).toLocaleString() : 'Per-test time unavailable'}</span>}</div><strong>{result.title}</strong><p>{result.description}</p><small>{actionLabels[result.destination]} →</small></button>)}</>}
      </div>}
      <footer className="search-footer"><span>Local pages, SQLite history, and verified rules</span><span>↑ ↓ select · Enter open · Esc close</span></footer>
    </section>
  </div>;
}
