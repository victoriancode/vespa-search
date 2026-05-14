CREATE TABLE IF NOT EXISTS repos (
  id TEXT PRIMARY KEY,
  repo_url TEXT NOT NULL,
  owner TEXT NOT NULL,
  name TEXT NOT NULL,
  default_branch TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS statuses (
  repo_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  message TEXT,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY (repo_id) REFERENCES repos(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS wiki_summaries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  repo_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  summary TEXT NOT NULL,
  long_summary TEXT NOT NULL,
  FOREIGN KEY (repo_id) REFERENCES repos(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS chunks (
  vector_id TEXT PRIMARY KEY,
  repo_id TEXT NOT NULL,
  file_path TEXT NOT NULL,
  language TEXT NOT NULL,
  line_start INTEGER NOT NULL,
  line_end INTEGER NOT NULL,
  content TEXT NOT NULL,
  content_lc TEXT NOT NULL,
  content_sha TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY (repo_id) REFERENCES repos(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chunks_repo_id ON chunks(repo_id);
CREATE INDEX IF NOT EXISTS idx_chunks_content_sha ON chunks(content_sha);
CREATE INDEX IF NOT EXISTS idx_wiki_repo_id_version ON wiki_summaries(repo_id, version);
