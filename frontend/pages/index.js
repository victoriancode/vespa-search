import { useEffect, useState } from 'react';
import Link from 'next/link';

const API_BASE =
  process.env.NEXT_PUBLIC_API_BASE || 'https://vespa-search.fly.dev';
const GITHUB_ORG = process.env.NEXT_PUBLIC_GITHUB_ORG || 'victoriancode';

export default function Home() {
  const [repoUrl, setRepoUrl] = useState('');
  const [repos, setRepos] = useState([]);
  const [selected, setSelected] = useState(null);
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const progressMap = {
    in_progress: 45,
    mirroring: 30,
    indexing: 70,
    summarizing: 85,
    complete: 100,
    error: 100
  };
  const progressValue = selected ? progressMap[status?.status] ?? 10 : 0;
  const progressLabel = selected ? `${progressValue}%` : '0%';

  const pastelFromString = (value) => {
    let hash = 0;
    for (let i = 0; i < value.length; i += 1) {
      hash = (hash << 5) - hash + value.charCodeAt(i);
      hash |= 0;
    }
    const hue = Math.abs(hash) % 360;
    return `hsl(${hue} 60% 88%)`;
  };

  const fetchRepos = async () => {
    const res = await fetch(
      `https://api.github.com/orgs/${GITHUB_ORG}/repos?per_page=100`
    );
    if (!res.ok) {
      throw new Error('Unable to load mirrored repositories');
    }
    const data = await res.json();
    const mirrors = data.filter((repo) => repo.name.endsWith('-vv-search'));
    const hydrated = await Promise.all(
      mirrors.map(async (repo) => {
        const branch = repo.default_branch || 'main';
        const stateUrl = `https://raw.githubusercontent.com/${GITHUB_ORG}/${repo.name}/${branch}/.vv/state.json`;
        const stateRes = await fetch(stateUrl);
        if (!stateRes.ok) {
          return null;
        }
        const state = await stateRes.json();
        if (!state.repo_id) {
          return null;
        }
        return {
          id: state.repo_id,
          repo_url: state.repo_url,
          owner: state.owner,
          name: state.name,
          mirror_repo: repo.name,
          mirror_url: repo.html_url
        };
      })
    );
    setRepos(hydrated.filter(Boolean));
  };

  const fetchStatus = async (repoId) => {
    if (!repoId) return;
    const res = await fetch(`${API_BASE}/repos/${repoId}/status`);
    const data = await res.json();
    setStatus(data);
  };

  useEffect(() => {
    fetchRepos().catch((err) => setError(err.message));
  }, []);

  useEffect(() => {
    if (!selected) return;
    fetchStatus(selected.id);
    const interval = setInterval(() => fetchStatus(selected.id), 3000);
    return () => clearInterval(interval);
  }, [selected]);

  const handleAddRepo = async (event) => {
    event.preventDefault();
    setError('');
    setLoading(true);
    try {
      const res = await fetch(`${API_BASE}/repos`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ repo_url: repoUrl })
      });
      if (!res.ok) {
        const data = await res.json();
        throw new Error(data.error || 'Unable to add repo');
      }
      const data = await res.json();
      setRepoUrl('');
      setRepos((prev) => {
        if (prev.some((repo) => repo.id === data.id)) {
          return prev;
        }
        return [
          {
            ...data,
            mirror_repo: `${data.name}-vv-search`,
            mirror_url: `https://github.com/${GITHUB_ORG}/${data.name}-vv-search`
          },
          ...prev
        ];
      });
      setSelected(data);
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  };

  const handleIngest = async () => {
    if (!selected) return;
    setStatus({ status: 'in_progress', message: 'Starting ingestion...' });
    const res = await fetch(`${API_BASE}/repos/${selected.id}/index`, { method: 'POST' });
    if (!res.ok) {
      const data = await res.json();
      setError(data.error || 'Ingestion failed');
      return;
    }
    const data = await res.json();
    setStatus(data);
  };

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand">
          <div>
            <strong>Vespa Vector Search</strong>
            <span>CodeWiki</span>
          </div>
        </div>
        <div className="topbar-actions">
          <span className="pill">Live indexing</span>
          <span className="pill ghost">Org: {GITHUB_ORG}</span>
        </div>
      </header>

      <main className="content">
        <section className="hero">
          <div className="hero-copy">
            <h1>Deep, fast code search for any repo.</h1>
            <p>
              Ingest public GitHub repositories, generate a living CodeWiki, and query
              across file paths, symbols, and natural language questions.
            </p>
            <div className="hero-meta">
              <span className="pill">Semantic + keyword</span>
              <span className="pill ghost">Auto-snippets</span>
              <span className="pill ghost">Vespa powered</span>
            </div>
          </div>
        </section>

        <section className="panel-grid">
          <section className="panel boxed">
            <div className="panel-header">
              <h2>Repositories</h2>
              <span className="subtle">Select one to see status</span>
            </div>
            <div className="repo-grid">
              <div className="repo-icon repo-icon-add" style={{ background: '#eef2f7' }}>
                <div className="repo-icon-title">
                  <span className="repo-icon-mark" aria-hidden="true">
                    <svg viewBox="0 0 24 24" role="img">
                      <path
                        d="M12 5v14M5 12h14"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.8"
                        strokeLinecap="round"
                      />
                    </svg>
                  </span>
                  <span className="repo-icon-label">Add repo</span>
                </div>
                <form onSubmit={handleAddRepo} className="form add-repo-form">
                  <input
                    type="url"
                    placeholder="https://github.com/owner/repo"
                    value={repoUrl}
                    onChange={(event) => setRepoUrl(event.target.value)}
                    required
                  />
                  <button type="submit" className="button-compact" disabled={loading}>
                    {loading ? 'Adding...' : 'Add'}
                  </button>
                </form>
                {error && <p className="error">{error}</p>}
              </div>
              {repos.map((repo) => (
                <button
                  key={repo.id}
                  className={`repo-icon ${selected?.id === repo.id ? 'active' : ''}`}
                  style={{ background: pastelFromString(`${repo.owner}/${repo.name}`) }}
                  onClick={() => setSelected(repo)}
                >
                  <span className="repo-icon-mark" aria-hidden="true">
                    <svg viewBox="0 0 24 24" role="img">
                      <path
                        d="M12 2.25c-5.38 0-9.75 4.37-9.75 9.75 0 4.3 2.79 7.94 6.66 9.22.49.09.67-.21.67-.47 0-.23-.01-1-.01-1.81-2.71.59-3.28-1.15-3.28-1.15-.44-1.12-1.08-1.42-1.08-1.42-.88-.6.07-.59.07-.59 1 .07 1.52 1.03 1.52 1.03.88 1.52 2.31 1.08 2.87.82.09-.64.35-1.08.63-1.33-2.16-.24-4.43-1.08-4.43-4.79 0-1.06.38-1.93 1.01-2.61-.1-.25-.44-1.25.1-2.61 0 0 .82-.26 2.7 1a9.33 9.33 0 0 1 4.92 0c1.88-1.26 2.7-1 2.7-1 .54 1.36.2 2.36.1 2.61.63.68 1.01 1.55 1.01 2.61 0 3.72-2.27 4.54-4.44 4.78.36.31.68.92.68 1.86 0 1.35-.01 2.44-.01 2.77 0 .26.18.57.67.47A9.76 9.76 0 0 0 21.75 12c0-5.38-4.37-9.75-9.75-9.75z"
                        fill="currentColor"
                      />
                    </svg>
                  </span>
                  <span className="repo-icon-label">{repo.owner}/{repo.name}</span>
                </button>
              ))}
            </div>
          </section>

          <section className="panel-stack">
            {repos.length > 0 && (
              <section className="panel highlight">
                <h2>Add repository</h2>
                <form onSubmit={handleAddRepo} className="form inline-form">
                  <input
                    type="url"
                    placeholder="https://github.com/owner/repo"
                    value={repoUrl}
                    onChange={(event) => setRepoUrl(event.target.value)}
                    required
                  />
                  <button type="submit" className="button-compact" disabled={loading}>
                    {loading ? 'Adding...' : 'Add Repo'}
                  </button>
                </form>
                {error && <p className="error">{error}</p>}
              </section>
            )}

            <section className="panel">
            <div className="panel-header">
              <h2>Ingestion Progress</h2>
              <span className="subtle">Queue status</span>
            </div>
            <div className="progress">
              <div
                className={`progress-bar ${status?.status === 'complete' ? 'complete' : ''}`}
                style={{
                  width: `${progressValue}%`
                }}
              />
            </div>
            <p className="status">
              {status?.message || 'Select a repo to check status.'} ({progressLabel})
            </p>
            <div className="actions">
              <button onClick={handleIngest} disabled={!selected}>
                Start ingestion
              </button>
              {status?.status === 'complete' && selected && (
                <Link
                  className="secondary"
                  href={{ pathname: '/repo', query: { id: selected.id } }}
                >
                  View repo
                </Link>
              )}
            </div>
            </section>
          </section>
        </section>
      </main>
    </div>
  );
}
