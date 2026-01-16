# Vespa Code Search + CodeWiki Plan (Rust + NextJS)

See `HIGH_LEVEL_DESIGN.md` for system-level design and data flow context.

## Goals
- Dense embedding semantic search over code.
- Search a codebase with semantic results.
- Rust services for ingestion and query.
- NextJS frontend for repo management, ingestion progress, and CodeWiki UI.
- Accept any public GitHub URL, ingest it, and generate a CodeWiki using the MCP DeepWiki connector.
- Each repo contains a `vv/` folder with vectors and repo metadata.
- Lightning-fast search via ANN and caching.

## 1) Architecture Overview
- **Vespa**: primary vector search engine for code chunks and embeddings.
- **Rust Ingestion Service**: clones repos, chunks files, builds embeddings, manages `vv/`, and feeds Vespa.
- **Rust CodeWiki Service**: orchestrates MCP DeepWiki generation during ingestion.
- **Rust Query Service**: embeds queries, performs ANN search, and serves results.
- **NextJS Frontend**: ingestion progress + repo browse + CodeWiki UI + search UI.

## 2) Repository Layout
- All public repos are cloned into this project under: `repos/<repo_owner>/<repo_name>`.
- Each repo contains a `vv/` folder:
  - `vv/manifest.json`: repo metadata, commit, branch, index version.
  - `vv/chunks.jsonl`: chunk metadata (file path + line ranges).
  - `vv/vectors/`: cached embeddings (optional).
  - `vv/wiki/`: CodeWiki artifacts produced by MCP DeepWiki.

## 3) Vespa Schema (Conceptual)
- **Document fields**
  - Repo metadata: `repo_id`, `repo_url`, `repo_name`, `repo_owner`, `commit_sha`, `branch`
  - File metadata: `file_path`, `language`, `license_spdx`
  - Chunk metadata: `chunk_id`, `chunk_hash`, `line_start`, `line_end`, `symbol_names`
  - Content fields: `content` (string), `content_sha`
  - Vector: `embedding` (tensor<float>(d))
  - Timestamps: `last_indexed_at`
- **Indexing**
  - ANN index for embeddings.
  - Optional BM25 on `content` for hybrid retrieval.
- **Ranking**
  - Primary: ANN similarity.
  - Optional rerank: content relevance + metadata signals.

## 4) Ingestion Workflow (Rust)
1. **User submits GitHub URL** in the frontend.
2. **Rust ingestion service** validates URL, clones into `repos/<owner>/<name>`.
3. **Chunking**
   - Prefer AST-based chunking where possible; fallback to size-based.
4. **Embeddings**
   - Generate code embeddings via a hosted or local embedding model.
5. **Create `vv/` folder**
   - Write `manifest.json`, `chunks.jsonl`, and cache embeddings if needed.
6. **Generate CodeWiki (MCP DeepWiki)**
   - During ingestion, call MCP DeepWiki (OpenAI + GitHub connector) to build a CodeWiki.
   - Store artifacts under `vv/wiki/`.
7. **Index to Vespa**
   - Batch and stream documents for throughput.
8. **Enable Search**
   - Search is only exposed after CodeWiki generation completes.

## 5) CodeWiki Requirements
- Use MCP DeepWiki connector per OpenAI tools guide: https://platform.openai.com/docs/guides/tools-connectors-mcp
- CodeWiki features should match DeepWiki (https://deepwiki.com/) but with a different look and feel.
- CodeWiki content is generated during ingestion and stored under `vv/wiki/`.

## 6) Rust Services
### Ingestion Service
- `POST /repos` → register repo
- `POST /repos/{id}/index` → clone + chunk + embed + CodeWiki + Vespa feed
- Emits progress events for frontend (SSE/WebSocket).

### CodeWiki Service
- Wraps MCP DeepWiki calls.
- Writes artifacts to `vv/wiki/`.

### Query Service
- `POST /search`
  - Input: query + optional repo filter
  - Output: ranked snippets with file/line metadata

## 7) NextJS Frontend
- **Repo management**
  - Add GitHub URL, show ingestion status.
- **Ingestion progress**
  - Live progress bar.
- **Post-ingestion UI**
  - When ingestion completes, show a button to view the repo.
  - Clicking the button opens the CodeWiki view.
- **CodeWiki view**
  - Display wiki artifacts from `vv/wiki/`.
  - Provide a button to enable/search the repo (post-wiki).

## 8) Performance (Lightning Fast)
- Vespa HNSW ANN tuning (`efSearch`, `max-links-per-node`).
- Cache embeddings and top queries.
- Parallel chunking + batched Vespa feeds.
- Keep vectors in RAM; use SSDs for indexing.
- Defer search availability until CodeWiki is ready to avoid rework.
