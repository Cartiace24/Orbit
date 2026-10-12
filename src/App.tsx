import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import DiagnosticsPage from './DiagnosticsPage';
import HistoryPage from './HistoryPage';
import IntelligencePage from './IntelligencePage';
import SearchDialog from './SearchDialog';
import type { SearchResult } from './search.types';
import type { TestType } from './diagnostics.types';
import './App.css';

type Availability = 'available' | 'unavailable';
type InventoryField<T> = { value: T | null; availability: Availability; reason: string | null };
type SystemInventory = {
  collectedAt: string;
  status: 'complete' | 'partial';
  operatingSystem: {
    productName: InventoryField<string>;
    edition: InventoryField<string>;
    displayVersion: InventoryField<string>;
    build: InventoryField<string>;
    architecture: InventoryField<string>;
  };
  computer: {
    name: InventoryField<string>;
    manufacturer: InventoryField<string>;
    model: InventoryField<string>;
  };
  processor: { model: InventoryField<string>; logicalProcessors: InventoryField<number> };
  memory: { installedBytes: InventoryField<number> };
};

type Section = 'Overview' | 'System' | 'Diagnostics' | 'Intelligence' | 'Repair Cases' | 'History' | 'Settings';
type Theme = 'dark' | 'light' | 'system';

const sections: { label: Section; icon: string }[] = [
  { label: 'Overview', icon: '◫' },
  { label: 'System', icon: '▤' },
  { label: 'Diagnostics', icon: '⌁' },
  { label: 'Intelligence', icon: '✳' },
  { label: 'Repair Cases', icon: '▧' },
  { label: 'History', icon: '◷' },
  { label: 'Settings', icon: '⚙' },
];

function display(field: InventoryField<unknown>): string {
  if (field.availability === 'available' && field.value !== null && field.value !== '') return String(field.value);
  return 'Unavailable';
}

function formatBytes(bytes: number): string {
  const gib = bytes / (1024 ** 3);
  return `${gib.toLocaleString(undefined, { maximumFractionDigits: 1 })} GB`;
}

function ErrorIcon() {
  return <span className="state-icon" aria-hidden="true">!</span>;
}

function App() {
  const [active, setActive] = useState<Section>('Overview');
  const [inventory, setInventory] = useState<SystemInventory | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [historyFocusSession, setHistoryFocusSession] = useState<string | null>(null);
  const navigateHistory = useCallback((sessionId?: string) => { setHistoryFocusSession(sessionId ?? null); setActive('History'); }, []);
  const [searchOpen, setSearchOpen] = useState(false);
  const searchButtonRef = useRef<HTMLButtonElement>(null);
  const [intelligenceLaunch, setIntelligenceLaunch] = useState<{ key: number; question?: string; action?: TestType }>({ key: 0 });
  const [theme, setTheme] = useState<Theme>(() => (localStorage.getItem('orbit-theme') as Theme | null) ?? 'dark');

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<SystemInventory>('get_system_inventory');
      setInventory(result);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError('The native inventory service could not be reached. Try again.');
      console.error('[ORBIT] System inventory collection failed.', { error: message });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    const onShortcut = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault(); setSearchOpen(true);
      }
    };
    window.addEventListener('keydown', onShortcut);
    return () => window.removeEventListener('keydown', onShortcut);
  }, []);
  useEffect(() => {
    localStorage.setItem('orbit-theme', theme);
    document.documentElement.dataset.theme = theme === 'system'
      ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
      : theme;
  }, [theme]);

  const timestamp = useMemo(() => inventory ? new Date(inventory.collectedAt).toLocaleString() : null, [inventory]);
  const isSystem = active === 'System';

  function closeSearch() { setSearchOpen(false); window.requestAnimationFrame(() => searchButtonRef.current?.focus()); }

  function selectSearchResult(result: SearchResult, query: string) {
    if (result.destination === 'section') {
      const section = sections.find(item => item.label === result.section)?.label;
      if (section) setActive(section);
    } else if (result.destination === 'history' && result.sessionId) {
      navigateHistory(result.sessionId);
    } else if (result.destination === 'intelligence') {
      setIntelligenceLaunch(previous => ({ key: previous.key + 1, question: query }));
      setActive('Intelligence');
    } else if (result.destination === 'actionPreview' && result.testType) {
      setIntelligenceLaunch(previous => ({ key: previous.key + 1, action: result.testType! }));
      setActive('Intelligence');
    }
    setSearchOpen(false);
    window.setTimeout(() => { const heading = document.querySelector<HTMLElement>('.content-scroll h1'); heading?.setAttribute('tabindex', '-1'); heading?.focus(); }, 0);
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand"><div className="brand-mark"><span /></div><div><strong>ORBIT</strong><small>PC WORKBENCH</small></div></div>
        <div className="nav-label">WORKSPACE</div>
        <nav aria-label="Main navigation">
          {sections.map((section) => (
            <button key={section.label} className={`nav-item ${active === section.label ? 'active' : ''}`} aria-current={active === section.label ? 'page' : undefined} onClick={() => { if (section.label === 'Intelligence') setIntelligenceLaunch(previous => ({ key: previous.key + 1 })); setActive(section.label); }}>
              <span className="nav-icon" aria-hidden="true">{section.icon}</span><span>{section.label}</span>
              {section.label === 'Repair Cases' && <span className="planned-dot" title="Planned" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="local-status"><span className="status-dot" /><div><strong>LOCAL SESSION</strong><small>Data stays on this device</small></div></div>
          <div className="sidebar-version"><span>ORBIT</span><span>v0.1.0</span></div>
        </div>
      </aside>

      <main className="main-area">
        <header className="topbar">
          <div className="breadcrumb"><span>Workspace</span><span className="crumb-sep">/</span><strong>{active}</strong></div>
          <div className="top-actions">
            <button ref={searchButtonRef} className="top-search" onClick={() => setSearchOpen(true)} aria-label="Search ORBIT" aria-keyshortcuts="Control+K"><span aria-hidden="true">⌕</span><span>Search ORBIT</span><kbd>Ctrl K</kbd></button>
            <span className="read-only"><span className="status-dot" /> READ-ONLY</span>
            <span className="top-divider" />
            <div className="top-mark">O</div>
          </div>
        </header>

        <div className="content-scroll">
          {active === 'Overview' && (
            <>
              <div className="page-heading">
                <div><div className="eyebrow">WORKSPACE / OVERVIEW</div><h1>System overview</h1><p className="page-subtitle">A clear view of this machine’s current baseline.</p></div>
                <button className="refresh-button" onClick={() => void refresh()} disabled={loading}><span className={loading ? 'refresh-glyph spinning' : 'refresh-glyph'}>↻</span>{loading ? 'Collecting…' : 'Refresh'}</button>
              </div>
              {error && <div className="error-banner"><ErrorIcon /><div><strong>Collection failed</strong><span>{error}</span></div><button onClick={() => void refresh()}>Try again</button></div>}
              {loading && !inventory ? <LoadingPanel /> : !inventory && error ? <EmptyError onRetry={() => void refresh()} /> : inventory && (
                <>
                  {inventory.status === 'partial' && <div className="notice-banner"><span className="notice-mark">i</span><span>Some system values could not be read. Unavailable fields are identified below.</span></div>}
                  <section className="summary-strip" aria-label="System summary">
                    <div className="summary-cell"><span className="summary-label">OPERATING SYSTEM</span><strong>{display(inventory.operatingSystem.productName)}</strong><span className="summary-meta">{display(inventory.operatingSystem.displayVersion)} · build {display(inventory.operatingSystem.build)}</span></div>
                    <div className="summary-cell"><span className="summary-label">COMPUTER NAME</span><strong>{display(inventory.computer.name)}</strong><span className="summary-meta">{display(inventory.operatingSystem.architecture)} architecture</span></div>
                    <div className="summary-cell"><span className="summary-label">PROCESSOR</span><strong>{display(inventory.processor.model)}</strong><span className="summary-meta">{display(inventory.processor.logicalProcessors)} logical processors</span></div>
                    <div className="summary-cell"><span className="summary-label">INSTALLED MEMORY</span><strong>{inventory.memory.installedBytes.value === null ? 'Unavailable' : formatBytes(inventory.memory.installedBytes.value)}</strong><span className="summary-meta">Physical memory capacity</span></div>
                  </section>
                  <div className="section-title-row"><div><h2>Machine details</h2><p>Collected directly from this Windows device.</p></div><span className={`collection-status ${error ? 'failed' : ''}`}><span className="status-dot" /> {error ? 'Refresh failed · showing last result' : inventory.status === 'complete' ? 'Collection complete' : 'Partial collection'}</span></div>
                  <div className="details-grid">
                    <DetailCard title="Operating system" icon="▤" rows={[
                      ['Product', inventory.operatingSystem.productName], ['Edition', inventory.operatingSystem.edition], ['Version', inventory.operatingSystem.displayVersion], ['Build', inventory.operatingSystem.build], ['Architecture', inventory.operatingSystem.architecture],
                    ]} />
                    <DetailCard title="Hardware" icon="⌘" rows={[
                      ['Computer', inventory.computer.name], ['Manufacturer', inventory.computer.manufacturer], ['Model', inventory.computer.model], ['Processor', inventory.processor.model], ['Logical processors', inventory.processor.logicalProcessors], ['Installed memory', inventory.memory.installedBytes.value === null ? inventory.memory.installedBytes : { ...inventory.memory.installedBytes, value: formatBytes(inventory.memory.installedBytes.value) }],
                    ]} />
                  </div>
                  <div className="footer-meta"><span>LAST UPDATED <strong>{timestamp}</strong></span><span>Read-only system inventory</span></div>
                </>
              )}
              {loading && inventory && <div className="refreshing-note"><span className="spinner" /> Refreshing system information…</div>}
            </>
          )}

          {isSystem && (
            <>
              <div className="page-heading"><div><div className="eyebrow">WORKSPACE / SYSTEM</div><h1>System inventory</h1><p className="page-subtitle">Hardware and operating system details reported by Windows.</p></div><button className="refresh-button" onClick={() => void refresh()} disabled={loading}><span className={loading ? 'refresh-glyph spinning' : 'refresh-glyph'}>↻</span>{loading ? 'Collecting…' : 'Refresh'}</button></div>
              {error && <div className="error-banner"><ErrorIcon /><div><strong>Collection failed</strong><span>{error}</span></div><button onClick={() => void refresh()}>Try again</button></div>}
              {!inventory && loading ? <LoadingPanel /> : !inventory && error ? <EmptyError onRetry={() => void refresh()} /> : inventory && <>
                {inventory.status === 'partial' && <div className="notice-banner"><span className="notice-mark">i</span><span>Some values are unavailable. This inventory does not include thermal or drive-health telemetry.</span></div>}
                <div className="inventory-meta"><span className="collection-status"><span className="status-dot" /> {inventory.status === 'complete' ? 'All requested values available' : 'Some values unavailable'}</span><span>Collected {timestamp}</span></div>
                <DetailCard wide title="Operating system" icon="▤" rows={[
                  ['Product', inventory.operatingSystem.productName], ['Edition', inventory.operatingSystem.edition], ['Version', inventory.operatingSystem.displayVersion], ['Build', inventory.operatingSystem.build], ['Architecture', inventory.operatingSystem.architecture],
                ]} />
                <DetailCard wide title="Processor & memory" icon="⌘" rows={[
                  ['Processor model', inventory.processor.model], ['Logical processors', inventory.processor.logicalProcessors], ['Installed memory', inventory.memory.installedBytes.value === null ? inventory.memory.installedBytes : { ...inventory.memory.installedBytes, value: formatBytes(inventory.memory.installedBytes.value) }],
                ]} />
                <DetailCard wide title="Computer" icon="▣" rows={[
                  ['Computer name', inventory.computer.name], ['Manufacturer', inventory.computer.manufacturer], ['Model', inventory.computer.model],
                ]} />
                <div className="footer-meta"><span>LAST UPDATED <strong>{timestamp}</strong></span><span>Read-only system inventory</span></div>
              </>}
              {loading && inventory && <div className="refreshing-note"><span className="spinner" /> Refreshing system information…</div>}
            </>
          )}

          {active === 'Settings' && <Settings theme={theme} setTheme={setTheme} />}
          {active === 'Diagnostics' && <DiagnosticsPage onNavigateHistory={() => navigateHistory()} />}
          {active === 'Intelligence' && <IntelligencePage key={intelligenceLaunch.key} onNavigateHistory={navigateHistory} initialQuestion={intelligenceLaunch.question} initialAction={intelligenceLaunch.action} />}
          {active === 'History' && <HistoryPage focusSessionId={historyFocusSession} />}
          {active === 'Repair Cases' && <Planned section={active} />}
        </div>
      </main>
      {searchOpen && <SearchDialog onClose={closeSearch} onSelect={selectSearchResult} />}
    </div>
  );
}

function DetailCard({ title, icon, rows, wide = false }: { title: string; icon: string; rows: [string, InventoryField<unknown>][]; wide?: boolean }) {
  return <section className={`detail-card ${wide ? 'wide' : ''}`}><header><span className="card-icon">{icon}</span><h3>{title}</h3></header><div className="detail-rows">{rows.map(([label, field]) => <div className="detail-row" key={label}><span>{label}</span><strong className={field.availability === 'unavailable' ? 'unavailable' : ''} title={field.reason ?? undefined}>{display(field)}</strong></div>)}</div></section>;
}

function LoadingPanel() {
  return <div className="loading-panel"><span className="spinner large" /><div><strong>Reading system information</strong><span>Querying Windows for this device’s inventory.</span></div></div>;
}

function EmptyError({ onRetry }: { onRetry: () => void }) {
  return <div className="loading-panel error-empty"><div><strong>No system inventory available</strong><span>ORBIT could not retrieve system information. Try the collection again.</span></div><button className="refresh-button" onClick={onRetry}>Try again</button></div>;
}

function Planned({ section }: { section: string }) {
  return <div className="page-heading"><div><div className="eyebrow">WORKSPACE / {section.toUpperCase()}</div><h1>{section}</h1><p className="page-subtitle">This area is planned for a later milestone.</p></div><span className="planned-badge">PLANNED</span></div>;
}

function Settings({ theme, setTheme }: { theme: Theme; setTheme: (theme: Theme) => void }) {
  return <><div className="page-heading"><div><div className="eyebrow">WORKSPACE / PREFERENCES</div><h1>Settings</h1><p className="page-subtitle">Configure how ORBIT appears on this device.</p></div></div><section className="settings-section"><div><h2>Appearance</h2><p>Choose the interface theme.</p></div><div className="theme-options" role="group" aria-label="Theme preference">{(['dark', 'light', 'system'] as Theme[]).map((option) => <button key={option} className={theme === option ? 'selected' : ''} onClick={() => setTheme(option)}>{option[0].toUpperCase() + option.slice(1)}</button>)}</div></section><section className="settings-section about-section"><div><h2>About ORBIT</h2><p>A local-first Windows troubleshooting workbench.</p></div><div className="about-meta"><span>Application</span><strong>ORBIT</strong><span>Version</span><strong>0.1.0</strong><span>Data handling</span><strong>Local only · no telemetry</strong></div></section></>;
}

export default App;
