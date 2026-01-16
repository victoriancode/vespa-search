# Vespa Code Search + CodeWiki (Rust + NextJS)

This repository documents a plan for building a Vespa-backed code search engine with Rust-based
services, a NextJS frontend, and an MCP-powered CodeWiki generated during ingestion.

## Contents
- `PLAN.md`: end-to-end architecture, workflow, schema, and UI requirements.
- `HIGH_LEVEL_DESIGN.md`: system-level design, data flows, and operational considerations.
- `docs/ARCHITECTURAL_REQUIREMENTS.md`: architectural requirements and constraints.
- `docs/ARCHITECTURAL_DOCUMENT.md`: architecture overview and component responsibilities.
- `docs/ARCHITECTURAL_SPECIFICATION.md`: API, schema, storage, and security specifics.
- `docs/DETAILED_DESIGN.md`: detailed ingestion/search workflows and tuning guidance.

## Highlights
- Accepts any public GitHub URL and clones it under `repos/<owner>/<name>`.
- Generates a CodeWiki using the MCP DeepWiki connector before enabling search.
- Stores vectors and repo metadata in a `vv/` folder inside each repo.

## Local development
### Backend (Rust)
```bash
cargo run
```

### Frontend (NextJS)
```bash
cd frontend
npm install
npm run dev
```

The frontend reads the backend base URL from `NEXT_PUBLIC_API_BASE` (defaults to `http://localhost:3001`).

### Combined build
The production Docker image builds the NextJS frontend as static assets and serves them from the Rust
backend, so the deployed app runs on a single port.

## Backend API (starter)
- `POST /repos` → register a repo URL.
- `POST /repos/{id}/index` → clone, generate `vv/` artifacts, and mark ingestion complete.
- `GET /repos/{id}/status` → ingestion status for progress UI.
- `GET /repos/{id}/wiki` → CodeWiki markdown content.
- `POST /search` → placeholder search endpoint (returns empty results for now).

## Deployment (Fly.io)
This repo includes a single Docker image that builds both the Rust backend and the NextJS frontend.

If you deploy manually with `flyctl`, run from the repo root:

```bash
flyctl deploy --config fly.toml --remote-only
```
