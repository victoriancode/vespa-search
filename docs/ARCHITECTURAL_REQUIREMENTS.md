# Architectural Requirements

## 1) Objectives
- Deliver lightning-fast semantic code search using Vespa ANN indexing.
- Generate a CodeWiki for each repository before enabling search.
- Support public GitHub repositories and store per-repo artifacts under `vv/`.
- Provide a Rust backend and NextJS frontend.

## 2) Functional Requirements
- **Repo ingestion**: accept public GitHub URLs and clone into `repos/<owner>/<name>`.
- **Chunking**: extract code chunks with file paths and line ranges.
- **Embeddings**: generate code embeddings and cache under `vv/vectors/`.
- **CodeWiki generation**: call MCP DeepWiki during ingestion and store results in `vv/wiki/`.
- **Search**: enable search only after CodeWiki completes.
- **UI**: show ingestion progress, show "View Repo" button after ingestion, and display CodeWiki.

## 3) Non-Functional Requirements
- **Performance**: low-latency search with Vespa HNSW ANN tuning.
- **Scalability**: parallel ingestion and batch Vespa feeds.
- **Reliability**: retries and clear status for MCP failures.
- **Security**: public-only repo ingestion; sanitize inputs; rate limiting.
- **Maintainability**: modular Rust services with clear API boundaries.

## 4) Constraints
- Use Rust for backend services.
- Use NextJS for the frontend.
- Use MCP DeepWiki for CodeWiki generation.
- Store all repo-specific artifacts under `vv/`.

## 5) Assumptions
- Repositories are public and cloneable.
- Embedding model is accessible via a hosted or local endpoint.
- Vespa is deployed with enough memory for vector indexes.
