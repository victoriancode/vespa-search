import { useEffect, useState } from 'react';
import Link from 'next/link';

const API_BASE = process.env.NEXT_PUBLIC_API_BASE || 'http://localhost:3001';

export default function Home() {
  const [repoUrl, setRepoUrl] = useState('');
  const [repos, setRepos] = useState([]);
  const [selected, setSelected] = useState(null);
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const fetchRepos = async () => {
    const res = await fetch(`${API_BASE}/repos`);
    const data = await res.json();
    setRepos(data);
  };

  const fetchStatus = async (repoId) => {
    if (!repoId) return;
    const res = await fetch(`${API_BASE}/repos/${repoId}/status`);
    const data = await res.json();
    setStatus(data);
  };

  useEffect(() => {
    fetchRepos();
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
      await fetchRepos();
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
    <div className="page">
      <header className="header">
        <div>
          <h1>Vespa Code Search + CodeWiki</h1>
          <p>Ingest any public GitHub repository, generate a CodeWiki, then enable semantic search.</p>
        </div>
      </header>

      <main className="grid">
        <section className="card">
          <h2>Add repository</h2>
          <form onSubmit={handleAddRepo} className="form">
            <input
              type="url"
              placeholder="https://github.com/owner/repo"
              value={repoUrl}
              onChange={(event) => setRepoUrl(event.target.value)}
              required
            />
            <button type="submit" disabled={loading}>
              {loading ? 'Adding...' : 'Add Repo'}
            </button>
          </form>
          {error && <p className="error">{error}</p>}
        </section>

        <section className="card">
          <h2>Repositories</h2>
          <div className="repo-list">
            {repos.length === 0 && <p>No repositories registered yet.</p>}
            {repos.map((repo) => (
              <button
                key={repo.id}
                className={`repo-item ${selected?.id === repo.id ? 'active' : ''}`}
                onClick={() => setSelected(repo)}
              >
                <strong>{repo.owner}/{repo.name}</strong>
                <span>{repo.repo_url}</span>
              </button>
            ))}
          </div>
        </section>

        <section className="card">
          <h2>Ingestion Progress</h2>
          <div className="progress">
            <div
              className={`progress-bar ${status?.status === 'complete' ? 'complete' : ''}`}
              style={{
                width:
                  status?.status === 'complete'
                    ? '100%'
                    : status?.status === 'in_progress'
                    ? '60%'
                    : '10%'
              }}
            />
          </div>
          <p className="status">
            {status?.message || 'Select a repo to check status.'}
          </p>
          <div className="actions">
            <button onClick={handleIngest} disabled={!selected}>
              Start ingestion
            </button>
            {status?.status === 'complete' && selected && (
              <Link className="secondary" href={`/repos/${selected.id}`}>
                View repo
              </Link>
            )}
          </div>
        </section>
      </main>
    </div>
  );
}
