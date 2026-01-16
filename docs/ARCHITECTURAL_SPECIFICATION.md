# Architectural Specification

## 1) Services
### Ingestion Service
- **Endpoint**: `POST /repos`
  - Body: `{ "repo_url": "https://github.com/<owner>/<repo>" }`
- **Endpoint**: `POST /repos/{id}/index`
  - Behavior: clone repo, chunk files, generate embeddings, trigger CodeWiki, feed Vespa.
- **Endpoint**: `GET /repos/{id}/status`
  - Returns ingestion status and progress.

### CodeWiki Service
- **Endpoint**: internal call from ingestion service.
- **Behavior**: invoke MCP DeepWiki and write artifacts to `vv/wiki/`.

### Query Service
- **Endpoint**: `POST /search`
  - Body: `{ "query": "...", "repo_filter": "optional" }`
  - Returns ranked results with file path, line range, and snippet.

## 2) Document Schema (Vespa)
- `repo_id`, `repo_url`, `repo_name`, `repo_owner`
- `commit_sha`, `branch`
- `file_path`, `language`, `license_spdx`
- `chunk_id`, `chunk_hash`, `line_start`, `line_end`, `symbol_names`
- `content`, `content_sha`
- `embedding` (tensor<float>(d))
- `last_indexed_at`

## 3) Ranking
- Primary ANN similarity on `embedding`.
- Optional hybrid scoring with BM25 on `content`.

## 4) Storage Requirements
- Per-repo storage must be under `repos/<owner>/<name>/vv/`.
- Artifacts:
  - `manifest.json` with repo metadata and versioning.
  - `chunks.jsonl` for chunk metadata.
  - `vectors/` for embedding caches.
  - `wiki/` for CodeWiki artifacts.

## 5) Progress Events
- Emit ingestion progress stages: cloning, chunking, embedding, CodeWiki, indexing.
- Expose status updates for frontend progress bar.

## 6) Security
- Accept public repo URLs only.
- Validate repo URL format and sanitize user input.
- Rate limit ingestion and search endpoints.
