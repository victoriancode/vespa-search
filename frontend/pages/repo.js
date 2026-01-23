import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import Link from 'next/link';

const API_BASE =
  process.env.NEXT_PUBLIC_API_BASE || 'https://vespa-search.fly.dev';

export default function RepoWiki() {
  const router = useRouter();
  const { id } = router.query;
  const [summary, setSummary] = useState('');
  const [longSummary, setLongSummary] = useState('');
  const [summaryHistory, setSummaryHistory] = useState([]);
  const [summaryIndex, setSummaryIndex] = useState(0);
  const [summaryLoading, setSummaryLoading] = useState(false);
  const [summaryError, setSummaryError] = useState('');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState([]);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState('');
  const [searchMode, setSearchMode] = useState('semantic');

  useEffect(() => {
    if (!id) return;
    fetch(`${API_BASE}/repos/${id}/wiki`)
      .then((res) => res.json())
      .then((data) => {
        setSummary(data.summary || '');
        setLongSummary(data.long_summary || '');
        setSummaryHistory(Array.isArray(data.history) ? data.history : []);
        setSummaryIndex(0);
      })
      .catch(() => {
        setSummary('Unable to load CodeWiki summary.');
        setLongSummary('');
        setSummaryHistory([]);
      });
  }, [id]);

  const handleSearch = async (event) => {
    event.preventDefault();
    if (!id || !query.trim()) return;
    setSearching(true);
    setSearchError('');
    try {
      const res = await fetch(`${API_BASE}/search`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: query.trim(),
          repo_filter: id,
          search_mode: searchMode
        })
      });
      const data = await res.json();
      if (!res.ok) {
        throw new Error(data.error || 'Search failed');
      }
      setResults(data.results || []);
    } catch (err) {
      setSearchError(err.message);
    } finally {
      setSearching(false);
    }
  };

  const handleUpdateSummary = async () => {
    if (!id) return;
    setSummaryLoading(true);
    setSummaryError('');
    try {
      const res = await fetch(`${API_BASE}/repos/${id}/wiki/summary`, {
        method: 'POST'
      });
      const data = await res.json();
      if (!res.ok) {
        throw new Error(data.error || 'Summary update failed');
      }
      setSummary(data.summary || '');
      setLongSummary(data.long_summary || '');
      setSummaryHistory(Array.isArray(data.history) ? data.history : []);
      setSummaryIndex(0);
    } catch (err) {
      setSummaryError(err.message);
    } finally {
      setSummaryLoading(false);
    }
  };

  const activeHistory = summaryHistory[summaryIndex];
  const historyCount = summaryHistory.length;

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand">
          <span className="logo">V</span>
          <div>
            <strong>Vespa Search</strong>
            <span>CodeWiki</span>
          </div>
        </div>
        <div className="topbar-actions">
          <Link href="/" className="secondary">
            Back to repos
          </Link>
        </div>
      </header>

      <main className="content">
        <section className="repo-hero">
          <div>
            <h1>CodeWiki workspace</h1>
            <p>
              Explore the generated wiki and run deep semantic searches across this repository.
            </p>
          </div>
        </section>

        <section className="split-layout">
          <aside className="panel boxed wiki-panel">
            <div className="panel-header summary-header">
              <h2>Wiki overview</h2>
              <div className="history-controls">
                <button
                  type="button"
                  className="icon-button"
                  onClick={() =>
                    setSummaryIndex((index) => Math.min(index + 1, historyCount - 1))
                  }
                  disabled={historyCount === 0 || summaryIndex >= historyCount - 1}
                >
                  ←
                </button>
                <span className="subtle">
                  {historyCount > 0
                    ? `v${activeHistory?.version ?? ''} of ${historyCount}`
                    : 'No history'}
                </span>
                <button
                  type="button"
                  className="icon-button"
                  onClick={() => setSummaryIndex((index) => Math.max(index - 1, 0))}
                  disabled={historyCount === 0 || summaryIndex === 0}
                >
                  →
                </button>
              </div>
            </div>
            <section className="summary-card">
              <header>
                <h3>Auto summary</h3>
              </header>
              <div className="summary-text">
                {activeHistory?.summary || summary || 'Summary not available yet.'}
              </div>
              {(activeHistory?.long_summary || longSummary) && (
                <div className="summary-text long">
                  {activeHistory?.long_summary || longSummary}
                </div>
              )}
              {activeHistory && (
                <span className="subtle">
                  Generated {new Date(activeHistory.created_at).toLocaleString()}
                </span>
              )}
              <button
                type="button"
                className="secondary"
                onClick={handleUpdateSummary}
                disabled={summaryLoading}
              >
                {summaryLoading ? 'Updating...' : 'Update summary'}
              </button>
              {summaryError && <p className="error">{summaryError}</p>}
            </section>
          </aside>

          <section className="panel search-panel">
            <div className="panel-header search-header">
              <div className="search-controls">
                <div className="mode-toggle">
                  <button
                    className={`pill ${searchMode === 'bm25' ? 'active' : 'ghost'}`}
                    type="button"
                    onClick={() => setSearchMode('bm25')}
                  >
                    Fast
                  </button>
                  <button
                    className={`pill ${searchMode === 'semantic' ? 'active' : 'ghost'}`}
                    type="button"
                    onClick={() => setSearchMode('semantic')}
                  >
                    Deep
                  </button>
                </div>
                <form onSubmit={handleSearch} className="search-bar inline">
                  <input
                    type="text"
                    placeholder="Search for functions, files, or concepts"
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                  />
                  <button type="submit" disabled={searching || !query.trim()}>
                    {searching ? 'Searching...' : 'Search'}
                  </button>
                </form>
              </div>
            </div>
            <div className="search-panel-body">
              {searchError && <p className="error">{searchError}</p>}
              {!searchError && results.length === 0 && !searching && (
                <div className="empty-state">
                  <p className="status">No results yet. Try a different query.</p>
                </div>
              )}
              {results.length > 0 && (
                <div className="result-list">
                  {results.map((result, index) => (
                    <article className="result-card" key={`${result.file_path}-${index}`}>
                      <div className="result-header">
                        <div>
                          <strong className="result-path">{result.file_path}</strong>
                          <span className="result-meta">
                            Lines {result.line_start}-{result.line_end}
                          </span>
                        </div>
                        <span className="pill ghost">Code</span>
                      </div>
                      <pre className="result-snippet">
                        <code>{result.snippet}</code>
                      </pre>
                    </article>
                  ))}
                </div>
              )}
            </div>
          </section>
        </section>

        {results.length > 0 && (
          <footer className="page-footer">
            <button
              type="button"
              className="secondary"
              onClick={() => window.scrollTo({ top: 0, behavior: 'smooth' })}
            >
              Return to top
            </button>
          </footer>
        )}
      </main>
    </div>
  );
}
