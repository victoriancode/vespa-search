# Detailed Design

## 1) Ingestion Pipeline
1. **Validate URL**
   - Ensure GitHub URL is public and uses allowed format.
2. **Clone Repository**
   - Clone into `repos/<owner>/<name>`.
3. **Chunking**
   - Prefer AST-based chunking for supported languages.
   - Fallback to fixed-size chunks with overlap.
4. **Embedding Generation**
   - Call embedding service and cache vectors in `vv/vectors/`.
5. **CodeWiki Generation**
   - Invoke MCP DeepWiki with repo context.
   - Store artifacts in `vv/wiki/`.
6. **Vespa Feed**
   - Batch feed documents to Vespa.
7. **Finalize**
   - Write `vv/manifest.json` and mark ingestion complete.

## 2) CodeWiki View
- Fetch and render wiki artifacts from `vv/wiki/`.
- Ensure different look and feel from deepwiki.com while retaining feature parity.
- Include a button to enable search after wiki generation.

## 3) Search Flow
- Embed user query.
- Execute Vespa ANN search with optional repo filter.
- Return ranked results with file paths, line ranges, and snippets.

## 4) Error Handling
- Retry MCP DeepWiki failures with exponential backoff.
- Mark ingestion status with error details for the frontend.
- Skip binary files and known vendor directories.

## 5) Performance Tuning
- Vespa HNSW params tuned for recall/latency balance.
- Parallel chunking and embedding generation.
- Cache common query embeddings.

## 6) Observability
- Structured logs per ingestion stage.
- Metrics for latency, throughput, and error rates.
- Tracing around CodeWiki generation and Vespa feed.
