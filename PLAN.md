# Vespa Code Search Plan (Rust + NextJS)

## Goals
- Dense embedding semantic search over code.
- Search across multiple public GitHub repositories.
- Rust services for ingestion and query.
- NextJS frontend for repo management and search UX.
- Each repo includes a `vv/` folder containing vectors + repo metadata.
- Lightning-fast search via ANN and caching.

## 1) Architecture Overview
- **Vespa**: primary search engine storing code chunks + embeddings + metadata.
- **Rust Ingestion Service**: clones repos, chunks files, builds embeddings, writes documents to Vespa.
- **Rust Query Service**: embeds queries, performs Vespa ANN search, optional rerank, returns results.
- **NextJS Frontend**: repo intake + search UI.

## 2) Vespa Schema (Conceptual)
- **Document fields**
  - `repo_id`, `repo_url`, `file_path`, `language`, `chunk_id`
  - `content` (string)
  - `embedding` (tensor<float>(d))
  - `commit_sha`, `last_indexed_at`
- **Indexing**
  - BM25 on `content` for hybrid retrieval (optional).
  - ANN index on `embedding`.
- **Ranking**
  - Primary: ANN similarity.
  - Optional rerank: content relevance + metadata signals.

## 3) Repo Ingestion Workflow (Rust)
1. **User submits GitHub URL** via frontend.
2. **Rust ingestion service** validates URL and clones repo.
3. **Chunking**
   - Prefer AST-based chunking (functions/classes), fallback to size-based.
4. **Embeddings**
   - Use code-optimized embedding model via hosted API or local service.
5. **Persist `vv/` folder**
   - `vv/manifest.json`: repo metadata, commit, index version.
   - `vv/chunks.jsonl`: chunk metadata (file path + line ranges).
   - `vv/vectors/`: optional cached embeddings.
6. **Index to Vespa**
   - Batch and stream documents for throughput.

## 4) Rust Services
### Ingestion Service
- `POST /repos` → register repo
- `POST /repos/{id}/index` → index/reindex
- Components:
  - Git client (e.g., `git2`)
  - Chunker module (language-aware)
  - Embedding client
  - Vespa feed client

### Query Service
- `POST /search`
  - Input: query + optional repo filter
  - Output: ranked code snippets with file/line metadata
- Flow:
  - Embed query → Vespa ANN search → optional rerank

## 5) NextJS Frontend
- Repo management: add URL, track indexing status.
- Search page: query, filters (repo/language), results list.
- Code snippet preview with file path + line numbers.

## 6) Performance (Lightning Fast)
- Vespa HNSW ANN index tuned (links, efSearch).
- Cache query embeddings and top results.
- Batch ingestion and parallel chunking.
- Keep vector memory footprint in RAM.
- Use SSDs for feed + reindex speed.

