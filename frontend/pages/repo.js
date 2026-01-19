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
    <div className="page">
      <header className="header">
        <div>
          <h1>CodeWiki</h1>
          <p>A curated, ingestion-first wiki experience for your repository.</p>
        </div>
        <div className="header-actions">
          <Link href="/" className="secondary">
            Back to repos
          </Link>
        </div>
      </header>

      <main className="stack">
        <section className="card wiki">
          <pre>{wiki}</pre>
        </section>

        <section className="card search">
          <h2>Search this repo</h2>
          <form onSubmit={handleSearch} className="form search-form">
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
          {searchError && <p className="error">{searchError}</p>}
          {!searchError && results.length === 0 && !searching && (
            <p className="status">No results yet. Try a different query.</p>
          )}
          {results.length > 0 && (
            <div className="search-results">
              {results.map((result, index) => (
                <div className="search-result" key={`${result.file_path}-${index}`}>
                  <div className="search-meta">
                    <strong>{result.file_path}</strong>
                    <span>
                      Lines {result.line_start}-{result.line_end}
                    </span>
                  </div>
                  <pre>{result.snippet}</pre>
                </div>
              ))}
            </div>
          )}
        </section>
      </main>
    </div>
  );
}
