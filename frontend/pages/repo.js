import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import Link from 'next/link';

const API_BASE =
  process.env.NEXT_PUBLIC_API_BASE || 'https://vespa-search.fly.dev';

export default function RepoWiki() {
  const router = useRouter();
  const { id } = router.query;
  const [wiki, setWiki] = useState('');
  const [query, setQuery] = useState('');
  const [results, setResults] = useState([]);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState('');

  useEffect(() => {
    if (!id) return;
    fetch(`${API_BASE}/repos/${id}/wiki`)
      .then((res) => res.json())
      .then((data) => setWiki(data.content || ''))
      .catch(() => setWiki('Unable to load CodeWiki.'));
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
          repo_filter: id
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
            <div className="panel-header">
              <h2>Wiki overview</h2>
              <span className="subtle">Auto-generated summary</span>
            </div>
            <pre>{wiki}</pre>
          </aside>

          <section className="panel search-panel">
            <div className="panel-header">
              <h2>Search</h2>
              <div className="mode-toggle">
                <button className="pill active" type="button">
                  Fast
                </button>
                <button className="pill ghost" type="button">
                  Deep
                </button>
              </div>
            </div>
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
          </section>
        </section>

        <section className="search-footer">
          <form onSubmit={handleSearch} className="search-bar bottom">
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
        </section>
      </main>
    </div>
  );
}
