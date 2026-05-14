const EMBEDDING_MODEL = '@cf/baai/bge-base-en-v1.5';
const MAX_FILE_BYTES = 200_000;
const DEFAULT_MAX_FILES = 80;
const DEFAULT_MAX_CHUNKS = 240;
const CHUNK_LINES = 120;
const CHUNK_OVERLAP = 20;
const VECTOR_BATCH_SIZE = 20;
const METADATA_CONTENT_LIMIT = 7000;

const CORS_HEADERS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET,POST,OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type,Authorization',
};

export default {
  async fetch(request, env, ctx) {
    if (request.method === 'OPTIONS') {
      return new Response(null, { headers: CORS_HEADERS });
    }

    try {
      const url = new URL(request.url);
      const route = matchRoute(url.pathname);

      if (request.method === 'GET' && route.name === 'health') {
        return json({ ok: true, backend: 'cloudflare-workers' });
      }

      if (request.method === 'GET' && route.name === 'repos') {
        return json(await listRepos(env));
      }

      if (request.method === 'POST' && route.name === 'repos') {
        return json(await createRepo(request, env), 201);
      }

      if (request.method === 'POST' && route.name === 'index') {
        const repo = await findRepo(env, route.params.id);
        await writeStatus(env, repo.id, 'in_progress', 'Ingestion queued');
        ctx.waitUntil(indexRepo(env, repo));
        return json({ status: 'in_progress', message: 'Ingestion started' });
      }

      if (request.method === 'GET' && route.name === 'status') {
        return json(await getStatus(env, route.params.id));
      }

      if (request.method === 'GET' && route.name === 'wiki') {
        return json(await getWiki(env, route.params.id));
      }

      if (request.method === 'POST' && route.name === 'summary') {
        const repo = await findRepo(env, route.params.id);
        return json(await writeGeneratedSummary(env, repo));
      }

      if (request.method === 'POST' && route.name === 'search') {
        return json(await search(request, env));
      }

      return json({ error: 'not found' }, 404);
    } catch (error) {
      const status = error.status || 500;
      return json({ error: error.message || 'internal error' }, status);
    }
  },
};

function matchRoute(pathname) {
  if (pathname === '/' || pathname === '/health') return { name: 'health', params: {} };
  if (pathname === '/repos') return { name: 'repos', params: {} };
  if (pathname === '/search') return { name: 'search', params: {} };

  const parts = pathname.split('/').filter(Boolean);
  if (parts[0] === 'repos' && parts.length === 3 && parts[2] === 'index') {
    return { name: 'index', params: { id: parts[1] } };
  }
  if (parts[0] === 'repos' && parts.length === 3 && parts[2] === 'status') {
    return { name: 'status', params: { id: parts[1] } };
  }
  if (parts[0] === 'repos' && parts.length === 3 && parts[2] === 'wiki') {
    return { name: 'wiki', params: { id: parts[1] } };
  }
  if (parts[0] === 'repos' && parts.length === 4 && parts[2] === 'wiki' && parts[3] === 'summary') {
    return { name: 'summary', params: { id: parts[1] } };
  }
  return { name: 'not_found', params: {} };
}

async function listRepos(env) {
  const { results } = await env.DB.prepare(
    'SELECT id, repo_url, owner, name FROM repos ORDER BY created_at DESC'
  ).all();
  return results || [];
}

async function createRepo(request, env) {
  const payload = await readJson(request);
  const { owner, name, repoUrl } = parseRepoUrl(payload.repo_url);
  const now = Date.now();
  const existing = await env.DB.prepare(
    'SELECT id, repo_url, owner, name FROM repos WHERE owner = ? AND name = ?'
  ).bind(owner, name).first();
  if (existing) return { ...existing, path: `cloudflare://${owner}/${name}` };

  const id = crypto.randomUUID();
  await env.DB.batch([
    env.DB.prepare(
      'INSERT INTO repos (id, repo_url, owner, name, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)'
    ).bind(id, repoUrl, owner, name, now, now),
    env.DB.prepare(
      'INSERT INTO statuses (repo_id, status, message, updated_at) VALUES (?, ?, ?, ?)'
    ).bind(id, 'registered', 'Repository registered. Start ingestion to index it.', now),
  ]);
  return { id, repo_url: repoUrl, owner, name, path: `cloudflare://${owner}/${name}` };
}

async function findRepo(env, id) {
  const repo = await env.DB.prepare(
    'SELECT id, repo_url, owner, name, default_branch FROM repos WHERE id = ?'
  ).bind(id).first();
  if (!repo) throw httpError(404, 'repo not found');
  return repo;
}

async function getStatus(env, repoId) {
  await findRepo(env, repoId);
  const status = await env.DB.prepare(
    'SELECT status, message FROM statuses WHERE repo_id = ?'
  ).bind(repoId).first();
  return status || { status: 'unknown', message: 'Status unavailable.' };
}

async function getWiki(env, repoId) {
  await findRepo(env, repoId);
  const { results } = await env.DB.prepare(
    'SELECT version, created_at, summary, long_summary FROM wiki_summaries WHERE repo_id = ? ORDER BY version DESC'
  ).bind(repoId).all();
  const history = results || [];
  const latest = history[0];
  if (!latest) {
    return {
      summary: '# CodeWiki\n\nWiki content is not yet available.',
      long_summary: '# CodeWiki\n\nWiki content is not yet available.',
      history: [],
    };
  }
  return {
    summary: latest.summary,
    long_summary: latest.long_summary,
    history,
  };
}

async function indexRepo(env, repo) {
  try {
    await writeStatus(env, repo.id, 'indexing', 'Fetching repository metadata from GitHub');
    const githubRepo = await githubJson(env, `/repos/${repo.owner}/${repo.name}`);
    const branch = githubRepo.default_branch || repo.default_branch || 'main';
    await env.DB.prepare(
      'UPDATE repos SET default_branch = ?, updated_at = ? WHERE id = ?'
    ).bind(branch, Date.now(), repo.id).run();

    await writeStatus(env, repo.id, 'indexing', 'Reading repository tree');
    const tree = await githubJson(
      env,
      `/repos/${repo.owner}/${repo.name}/git/trees/${encodeURIComponent(branch)}?recursive=1`
    );
    const files = selectIndexableFiles(tree.tree || [], maxFiles(env));
    if (files.length === 0) {
      throw new Error('No indexable source files found.');
    }

    await deleteExistingChunks(env, repo.id);
    let indexed = 0;
    const fileSummaries = [];

    for (const file of files) {
      await writeStatus(env, repo.id, 'indexing', `Indexing ${file.path}`);
      const content = await githubRaw(env, repo.owner, repo.name, branch, file.path);
      if (!content || content.trim().length === 0) continue;

      fileSummaries.push({ path: file.path, language: guessLanguage(file.path) });
      const chunks = chunkFile(content, file.path, maxChunks(env) - indexed);
      if (chunks.length === 0) continue;

      for (const batch of batches(chunks, VECTOR_BATCH_SIZE)) {
        const texts = batch.map((chunk) => chunk.content);
        const embeddings = await embedTexts(env, texts);
        const now = Date.now();
        const vectors = [];
        const statements = [];

        batch.forEach((chunk, index) => {
          const vectorId = `${repo.id}:${sha256(`${chunk.file_path}:${chunk.line_start}:${chunk.content_sha}`)}`;
          vectors.push({
            id: vectorId,
            values: embeddings[index],
            metadata: {
              repo_id: repo.id,
              repo_owner: repo.owner,
              repo_name: repo.name,
              repo_url: repo.repo_url,
              file_path: chunk.file_path,
              language: chunk.language,
              line_start: chunk.line_start,
              line_end: chunk.line_end,
              content: truncate(chunk.content, METADATA_CONTENT_LIMIT),
            },
          });
          statements.push(
            env.DB.prepare(
              `INSERT OR REPLACE INTO chunks
                (vector_id, repo_id, file_path, language, line_start, line_end, content, content_lc, content_sha, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
            ).bind(
              vectorId,
              repo.id,
              chunk.file_path,
              chunk.language,
              chunk.line_start,
              chunk.line_end,
              chunk.content,
              chunk.content.toLowerCase(),
              chunk.content_sha,
              now
            )
          );
        });

        await env.VECTORIZE.upsert(vectors);
        await env.DB.batch(statements);
        indexed += batch.length;
      }

      if (indexed >= maxChunks(env)) break;
    }

    await writeStatus(env, repo.id, 'summarizing', 'Generating CodeWiki summary');
    await writeGeneratedSummary(env, { ...repo, default_branch: branch }, fileSummaries, indexed);
    await writeStatus(env, repo.id, 'complete', `Ingestion complete. Indexed ${indexed} chunks.`);
  } catch (error) {
    await writeStatus(env, repo.id, 'error', error.message || 'Ingestion failed');
    throw error;
  }
}

async function writeGeneratedSummary(env, repo, fileSummaries = null, indexed = null) {
  if (!fileSummaries) {
    const { results } = await env.DB.prepare(
      'SELECT DISTINCT file_path, language FROM chunks WHERE repo_id = ? ORDER BY file_path LIMIT 80'
    ).bind(repo.id).all();
    fileSummaries = results || [];
    indexed = await env.DB.prepare(
      'SELECT COUNT(*) AS count FROM chunks WHERE repo_id = ?'
    ).bind(repo.id).first().then((row) => row?.count || 0);
  }

  const languages = countBy(fileSummaries.map((file) => file.language || 'unknown'));
  const languageText = Object.entries(languages)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 8)
    .map(([language, count]) => `${language} (${count})`)
    .join(', ');
  const fileList = fileSummaries.slice(0, 30).map((file) => `- ${file.path || file.file_path}`).join('\n');
  const summary = `# CodeWiki for ${repo.owner}/${repo.name}\n\nIndexed on Cloudflare Workers with Workers AI embeddings and Vectorize search.\n\nLanguages: ${languageText || 'unknown'}\n\nIndexed chunks: ${indexed ?? 0}`;
  const longSummary = `${summary}\n\nRepresentative files:\n${fileList || '- No files indexed yet.'}`;
  const row = await env.DB.prepare(
    'SELECT COALESCE(MAX(version), 0) + 1 AS next_version FROM wiki_summaries WHERE repo_id = ?'
  ).bind(repo.id).first();
  const version = row?.next_version || 1;
  await env.DB.prepare(
    'INSERT INTO wiki_summaries (repo_id, version, created_at, summary, long_summary) VALUES (?, ?, ?, ?, ?)'
  ).bind(repo.id, version, Date.now(), summary, longSummary).run();
  return getWiki(env, repo.id);
}

async function search(request, env) {
  const payload = await readJson(request);
  const query = String(payload.query || '').trim();
  if (!query) return { results: [] };

  const mode = String(payload.search_mode || 'hybrid').toLowerCase();
  const repoFilter = String(payload.repo_filter || '').trim();
  const semanticResults = mode === 'bm25' ? [] : await semanticSearch(env, query, repoFilter);
  const keywordResults = mode === 'semantic' ? [] : await keywordSearch(env, query, repoFilter);
  const merged = mergeResults(semanticResults, keywordResults, mode);
  return { results: merged.slice(0, repoFilter ? 50 : 10) };
}

async function semanticSearch(env, query, repoFilter) {
  const [embedding] = await embedTexts(env, [query]);
  const options = {
    topK: 50,
    returnMetadata: 'all',
  };
  if (repoFilter) {
    options.filter = { repo_id: repoFilter };
  }
  const matches = await env.VECTORIZE.query(embedding, options);
  return (matches.matches || [])
    .map((match) => ({
      repo_id: match.metadata?.repo_id || '',
      file_path: match.metadata?.file_path || '',
      line_start: Number(match.metadata?.line_start || 1),
      line_end: Number(match.metadata?.line_end || 1),
      snippet: buildSnippet(match.metadata?.content || ''),
      vector_score: Number(match.score || 0),
      keyword_score: 0,
      match_type: 'semantic',
      key: match.id,
    }));
}

async function keywordSearch(env, query, repoFilter) {
  const terms = tokenize(query).slice(0, 6);
  if (terms.length === 0) return [];

  const where = [];
  const binds = [];
  if (repoFilter) {
    where.push('repo_id = ?');
    binds.push(repoFilter);
  }
  for (const term of terms) {
    where.push('content_lc LIKE ?');
    binds.push(`%${term}%`);
  }
  const sql = `
    SELECT vector_id, repo_id, file_path, line_start, line_end, content
    FROM chunks
    WHERE ${where.join(' AND ')}
    ORDER BY updated_at DESC
    LIMIT 50
  `;
  const { results } = await env.DB.prepare(sql).bind(...binds).all();
  return (results || []).map((row) => ({
    repo_id: row.repo_id,
    file_path: row.file_path,
    line_start: row.line_start,
    line_end: row.line_end,
    snippet: buildSnippet(row.content),
    vector_score: 0,
    keyword_score: scoreKeyword(row.content, terms),
    match_type: 'keyword',
    key: row.vector_id,
  }));
}

function mergeResults(semanticResults, keywordResults, mode) {
  const merged = new Map();
  for (const result of [...semanticResults, ...keywordResults]) {
    const existing = merged.get(result.key);
    if (!existing) {
      merged.set(result.key, result);
      continue;
    }
    existing.vector_score = Math.max(existing.vector_score, result.vector_score);
    existing.keyword_score = Math.max(existing.keyword_score, result.keyword_score);
    existing.match_type = mode === 'hybrid' ? 'hybrid' : existing.match_type;
  }
  return [...merged.values()].sort((a, b) => combinedScore(b, mode) - combinedScore(a, mode));
}

function combinedScore(result, mode) {
  if (mode === 'semantic') return result.vector_score;
  if (mode === 'bm25') return result.keyword_score;
  return result.vector_score * 0.65 + result.keyword_score * 0.35;
}

async function embedTexts(env, texts) {
  const response = await env.AI.run(EMBEDDING_MODEL, { text: texts.map((text) => truncate(text, 4000)) });
  const data = Array.isArray(response) ? response : response.data || response.embeddings;
  if (!Array.isArray(data) || data.length !== texts.length) {
    throw new Error('Workers AI returned an unexpected embedding response.');
  }
  return data.map((embedding) => normalizeEmbedding(embedding));
}

function normalizeEmbedding(embedding) {
  if (!Array.isArray(embedding)) throw new Error('Invalid embedding vector.');
  if (embedding.length === 768) return embedding;
  if (embedding.length > 768) return embedding.slice(0, 768);
  return embedding.concat(Array(768 - embedding.length).fill(0));
}

async function githubJson(env, path) {
  const response = await fetch(`https://api.github.com${path}`, {
    headers: githubHeaders(env),
  });
  if (!response.ok) {
    throw new Error(`GitHub request failed: ${response.status} ${await response.text()}`);
  }
  return response.json();
}

async function githubRaw(env, owner, name, branch, path) {
  const response = await fetch(
    `https://raw.githubusercontent.com/${owner}/${name}/${encodeURIComponent(branch)}/${path.split('/').map(encodeURIComponent).join('/')}`,
    { headers: githubHeaders(env) }
  );
  if (!response.ok) return '';
  const content = await response.text();
  return sanitizeContent(content);
}

function githubHeaders(env) {
  const headers = {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'vespa-search-cloudflare-worker',
  };
  if (env.GITHUB_TOKEN) headers.Authorization = `Bearer ${env.GITHUB_TOKEN}`;
  return headers;
}

function selectIndexableFiles(tree, limit) {
  return tree
    .filter((item) => item.type === 'blob')
    .filter((item) => item.size > 0 && item.size <= MAX_FILE_BYTES)
    .filter((item) => !shouldSkipPath(item.path))
    .filter((item) => guessLanguage(item.path) !== 'unknown')
    .slice(0, limit);
}

function chunkFile(content, filePath, remaining) {
  const lines = content.split(/\r?\n/);
  const chunks = [];
  for (let start = 0; start < lines.length && chunks.length < remaining; start += CHUNK_LINES - CHUNK_OVERLAP) {
    const end = Math.min(start + CHUNK_LINES, lines.length);
    const chunkContent = lines.slice(start, end).join('\n').trim();
    if (chunkContent) {
      chunks.push({
        file_path: filePath,
        language: guessLanguage(filePath),
        line_start: start + 1,
        line_end: end,
        content: chunkContent,
        content_sha: sha256(chunkContent),
      });
    }
    if (end >= lines.length) break;
  }
  return chunks;
}

async function deleteExistingChunks(env, repoId) {
  const { results } = await env.DB.prepare(
    'SELECT vector_id FROM chunks WHERE repo_id = ? LIMIT 1000'
  ).bind(repoId).all();
  const ids = (results || []).map((row) => row.vector_id);
  for (const batch of batches(ids, 100)) {
    if (batch.length > 0) await env.VECTORIZE.deleteByIds(batch);
  }
  await env.DB.prepare('DELETE FROM chunks WHERE repo_id = ?').bind(repoId).run();
}

async function writeStatus(env, repoId, status, message) {
  await env.DB.prepare(
    `INSERT INTO statuses (repo_id, status, message, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(repo_id) DO UPDATE SET status = excluded.status, message = excluded.message, updated_at = excluded.updated_at`
  ).bind(repoId, status, message, Date.now()).run();
}

async function readJson(request) {
  try {
    return await request.json();
  } catch {
    throw httpError(400, 'invalid json body');
  }
}

function parseRepoUrl(repoUrl) {
  const raw = String(repoUrl || '').trim().replace(/\/$/, '').replace(/\.git$/, '');
  const match = raw.match(/^(?:https?:\/\/github\.com\/|git@github\.com:)([^/]+)\/([^/]+)$/);
  if (!match) throw httpError(400, 'invalid repo url');
  return {
    owner: match[1],
    name: match[2],
    repoUrl: `https://github.com/${match[1]}/${match[2]}`,
  };
}

function shouldSkipPath(path) {
  return /(^|\/)(\.git|\.vv|vv|node_modules|target|dist|build|\.next|\.venv|venv|__pycache__)\//.test(path)
    || /\.(png|jpe?g|gif|webp|ico|pdf|zip|gz|tar|mp4|mov|woff2?|ttf|lock)$/i.test(path);
}

function guessLanguage(path) {
  const ext = path.split('.').pop().toLowerCase();
  const map = {
    rs: 'rust',
    ts: 'typescript',
    tsx: 'typescript',
    js: 'javascript',
    jsx: 'javascript',
    py: 'python',
    go: 'go',
    java: 'java',
    rb: 'ruby',
    md: 'markdown',
    json: 'json',
    yml: 'yaml',
    yaml: 'yaml',
    css: 'css',
    html: 'html',
    toml: 'toml',
    sql: 'sql',
  };
  return map[ext] || 'unknown';
}

function sanitizeContent(input) {
  return input.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '');
}

function buildSnippet(content) {
  return truncate(String(content || '').trim(), 400);
}

function tokenize(query) {
  return String(query || '').toLowerCase().match(/[a-z0-9_.$/-]{2,}/g) || [];
}

function scoreKeyword(content, terms) {
  const lower = String(content || '').toLowerCase();
  const score = terms.reduce((sum, term) => sum + (lower.includes(term) ? 1 : 0), 0);
  return terms.length === 0 ? 0 : score / terms.length;
}

function countBy(values) {
  return values.reduce((acc, value) => {
    acc[value] = (acc[value] || 0) + 1;
    return acc;
  }, {});
}

function maxFiles(env) {
  return Number(env.MAX_INDEX_FILES || DEFAULT_MAX_FILES);
}

function maxChunks(env) {
  return Number(env.MAX_INDEX_CHUNKS || DEFAULT_MAX_CHUNKS);
}

function truncate(value, maxLength) {
  const text = String(value || '');
  return text.length > maxLength ? `${text.slice(0, maxLength)}...` : text;
}

function batches(items, size) {
  const output = [];
  for (let index = 0; index < items.length; index += size) {
    output.push(items.slice(index, index + size));
  }
  return output;
}

async function sha256HexBuffer(value) {
  const data = new TextEncoder().encode(value);
  return crypto.subtle.digest('SHA-256', data);
}

function sha256(value) {
  let hash = 0;
  const text = String(value);
  for (let index = 0; index < text.length; index += 1) {
    hash = (Math.imul(31, hash) + text.charCodeAt(index)) | 0;
  }
  return Math.abs(hash).toString(16);
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      ...CORS_HEADERS,
      'Content-Type': 'application/json',
    },
  });
}

function httpError(status, message) {
  const error = new Error(message);
  error.status = status;
  return error;
}
