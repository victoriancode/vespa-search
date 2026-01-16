import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import Link from 'next/link';

const API_BASE =
  process.env.NEXT_PUBLIC_API_BASE || 'https://vespa-search.fly.dev';

export default function RepoWiki() {
  const router = useRouter();
  const { id } = router.query;
  const [wiki, setWiki] = useState('');

  useEffect(() => {
    if (!id) return;
    fetch(`${API_BASE}/repos/${id}/wiki`)
      .then((res) => res.json())
      .then((data) => setWiki(data.content || ''))
      .catch(() => setWiki('Unable to load CodeWiki.'));
  }, [id]);

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
          <button className="primary">Enable search</button>
        </div>
      </header>

      <main className="card wiki">
        <pre>{wiki}</pre>
      </main>
    </div>
  );
}
