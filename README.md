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

## Backend API (starter)
- `POST /repos` → register a repo URL.
- `POST /repos/{id}/index` → clone, generate `vv/` artifacts, and mark ingestion complete.
- `GET /repos/{id}/status` → ingestion status for progress UI.
- `GET /repos/{id}/wiki` → CodeWiki markdown content.
- `POST /search` → placeholder search endpoint (returns empty results for now).

## Deployment (GitHub Actions)
This repo includes a GitHub Actions workflow to deploy the Rust backend to Fly.io (free-tier friendly).

1. Create a Fly.io app (example: `fly apps create vespa-code-search`).
2. Add the Fly.io API token as a GitHub secret named `FLY_API_TOKEN`.
3. Update the `FLY_APP_NAME` value in `.github/workflows/deploy-backend.yml`.

The workflow will build and deploy the backend on pushes to `main`.

If you deploy manually with `flyctl`, run from the repo root:

```bash
flyctl deploy --config fly.toml --remote-only
```
