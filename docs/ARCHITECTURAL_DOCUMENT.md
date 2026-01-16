# Architectural Document

## Overview
The system provides semantic code search and a CodeWiki experience for public GitHub repositories. Ingestion first generates a CodeWiki, then indexes code into Vespa for lightning-fast search. A Rust backend orchestrates ingestion, CodeWiki generation, and queries; a NextJS frontend provides repository management, progress tracking, and search UI.

## System Context
- **User**: submits a public GitHub URL and queries.
- **Frontend**: NextJS UI for ingestion progress, CodeWiki view, and search.
- **Backend**: Rust services for ingestion, CodeWiki generation, and query handling.
- **Vespa**: vector database for ANN search.
- **MCP DeepWiki**: generates wiki artifacts during ingestion.

## Component Responsibilities
1. **Ingestion Service (Rust)**
   - Clone repo, chunk code, generate embeddings, and manage `vv/` artifacts.
   - Feed Vespa with chunk metadata and embeddings.
   - Emit progress events (SSE/WebSockets).
2. **CodeWiki Service (Rust)**
   - Call MCP DeepWiki and write artifacts under `vv/wiki/`.
3. **Query Service (Rust)**
   - Embed queries and perform ANN search against Vespa.
4. **Vespa**
   - Store documents containing code chunks, metadata, and embeddings.
5. **NextJS Frontend**
   - Repo submission, progress bar, CodeWiki rendering, search UI.

## Data Flow Summary
- **Ingestion**: GitHub URL → clone → chunk → embed → CodeWiki → Vespa feed → enable search.
- **Search**: query → embed → Vespa ANN → results with file/line metadata.

## Storage Layout
```
repos/
  <owner>/<repo>/
    vv/
      manifest.json
      chunks.jsonl
      vectors/
      wiki/
```

## External Integrations
- MCP DeepWiki (OpenAI + GitHub connector).
- Embedding model service.
