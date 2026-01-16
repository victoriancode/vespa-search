# High Level Design: Vespa Code Search + CodeWiki (Rust + NextJS)

## 1) Goals
- Provide lightning-fast semantic code search using Vespa ANN indexing.
- Generate a CodeWiki for each ingested repository before enabling search.
- Support public GitHub repositories and store per-repo artifacts under `vv/`.
- Offer a Rust-based ingestion/query backend with a NextJS frontend.

## 2) Non-Goals
- Private repository authentication and enterprise SSO.
- Full IDE-level refactoring or code intelligence.
- Multi-tenant billing, quotas, or enterprise compliance features.

## 3) System Context
- **Users** submit public GitHub URLs and search queries via the NextJS UI.
- **Rust services** ingest repos, generate CodeWiki, and serve queries.
- **Vespa** stores and retrieves vectors plus metadata.
- **MCP DeepWiki** produces CodeWiki content during ingestion.

## 4) Core Components
1. **NextJS Frontend**
   - Repo submission, ingestion progress, CodeWiki view, search UI.
   - Shows a "View Repo" button once ingestion completes.
2. **Rust Ingestion Service**
   - Clones repos, chunks code, embeds, and writes `vv/` artifacts.
   - Streams ingestion progress to the frontend (SSE/WebSockets).
3. **Rust CodeWiki Service**
   - Calls MCP DeepWiki to generate wiki content.
   - Stores artifacts under `vv/wiki/`.
4. **Rust Query Service**
   - Embeds queries and performs ANN retrieval from Vespa.
5. **Vespa**
   - Stores code chunks, embeddings, and metadata for fast retrieval.

## 5) Data Flow (Ingestion)
1. User submits a public GitHub URL.
2. Ingestion service clones into `repos/<owner>/<name>`.
3. Chunking extracts code segments with file path + line ranges.
4. Embeddings are generated and cached under `vv/vectors/`.
5. CodeWiki service generates wiki artifacts via MCP DeepWiki → `vv/wiki/`.
6. Ingestion service feeds Vespa with chunk metadata + embeddings.
7. Frontend enables "View Repo" and "Search" once CodeWiki is complete.

## 6) Data Flow (Search)
1. User submits a query.
2. Query service embeds the query.
3. Vespa ANN retrieves top-k matches.
4. Results are returned with file paths, line ranges, and snippets.

## 7) Storage Layout
```
repos/
  <owner>/<repo>/
    vv/
      manifest.json
      chunks.jsonl
      vectors/
      wiki/
```

## 8) APIs (High Level)
- `POST /repos` → register GitHub URL
- `POST /repos/{id}/index` → ingest + CodeWiki + Vespa feed
- `GET /repos/{id}/status` → progress tracking
- `POST /search` → semantic search

## 9) Performance & Scaling
- Vespa HNSW tuning (`efSearch`, `max-links-per-node`).
- Batch ingestion and parallel chunking.
- Cache frequent query embeddings.
- Keep vectors in RAM; SSDs for index persistence.

## 10) Observability & Operations
- Structured logs for ingestion, CodeWiki, and search.
- Metrics: ingestion time, embedding throughput, query latency.
- Alerts on ingestion failures and MCP errors.

## 11) Risks & Mitigations
- **Large repos** → chunking limits and batching.
- **MCP downtime** → retry with backoff, mark CodeWiki status.
- **Embedding cost** → cache embeddings and dedupe content.

## 12) Security & Compliance
- Only public GitHub repos are supported.
- Rate limit ingestion and search endpoints.
- Sanitize inputs and strip secrets from stored artifacts.
