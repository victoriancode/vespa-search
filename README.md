# Cloudflare Code Search + CodeWiki (Workers + NextJS)

This repository now includes a Cloudflare-native code search backend alongside the original Rust/Vespa prototype.
The Cloudflare path uses Workers, D1, Workers AI embeddings, and Vectorize to provide vector-based hybrid search
for public GitHub repositories while preserving the existing frontend API contract.

## Contents
- `PLAN.md`: end-to-end architecture, workflow, schema, and UI requirements.
- `HIGH_LEVEL_DESIGN.md`: system-level design, data flows, and operational considerations.
- `docs/ARCHITECTURAL_REQUIREMENTS.md`: architectural requirements and constraints.
- `docs/ARCHITECTURAL_DOCUMENT.md`: architecture overview and component responsibilities.
- `docs/ARCHITECTURAL_SPECIFICATION.md`: API, schema, storage, and security specifics.
- `docs/DETAILED_DESIGN.md`: detailed ingestion/search workflows and tuning guidance.

## Highlights
- Accepts any public GitHub URL.
- Ingests source files through the GitHub API, which is compatible with Cloudflare Workers.
- Stores repository, status, chunk, and CodeWiki metadata in Cloudflare D1.
- Generates 768-dimensional embeddings with Workers AI (`@cf/baai/bge-base-en-v1.5`).
- Stores vectors and searchable metadata in Cloudflare Vectorize.
- Serves `bm25`, `semantic`, and `hybrid` search modes from the same `/search` endpoint used by the UI.

## Local development
### Backend (Cloudflare Worker)
Requires Node.js 22+ for current Wrangler.

```bash
npm install
npm run cf:dev
```

The local Worker API defaults to `http://localhost:8787`.

### Backend (legacy Rust/Vespa prototype)
```bash
cargo run
```

### Frontend (NextJS)
```bash
cd frontend
npm install
npm run dev
```

The frontend reads the backend base URL from `NEXT_PUBLIC_API_BASE` (defaults to `http://localhost:8787`).

## Backend API
- `POST /repos` → register a repo URL.
- `POST /repos/{id}/index` → fetch, chunk, embed, and index repository files on Cloudflare.
- `GET /repos/{id}/status` → ingestion status for progress UI.
- `GET /repos/{id}/wiki` → CodeWiki summary content.
- `POST /search` → vector, keyword, or hybrid search depending on `search_mode`.

## Cloudflare Resources
Create the Cloudflare resources once before deploying. The Workers AI model outputs 768 dimensions, so the
Vectorize index must match that dimension.

```bash
npx wrangler vectorize create vespa-search-code --dimensions=768 --metric=cosine
npx wrangler vectorize create-metadata-index vespa-search-code --propertyName=repo_id --type=string
npx wrangler d1 create vespa-search-db
```

Copy the D1 `database_id` into `wrangler.worker.jsonc`.

Optional for higher GitHub API limits:

```bash
npx wrangler secret put GITHUB_TOKEN
```

## Deployment
GitHub Actions are configured for Cloudflare:

- `.github/workflows/deploy-backend.yml` applies D1 migrations and deploys the Worker.
- `.github/workflows/deploy-frontend.yml` builds the static NextJS export and deploys it to Cloudflare Pages.

Set these GitHub secrets:

- `CLOUDFLARE_ACCOUNT_ID`
- `CLOUDFLARE_API_TOKEN`

The API token should be scoped to the target account and include these account permissions:

- `Cloudflare Pages:Edit`
- `Workers Scripts:Edit`
- `D1:Edit`
- `Vectorize:Edit`
- `Workers AI:Edit`

If Wrangler still logs `/memberships` authentication failures while diagnosing a failed deploy, add the user
permission `Memberships:Read` or use Cloudflare's broader Workers/Pages deployment token template.

Set this GitHub Actions variable for the frontend:

- `NEXT_PUBLIC_API_BASE`, for example `https://vespa-search-api.<your-subdomain>.workers.dev`

Manual deploy:

```bash
npx wrangler pages project create vespa-search --production-branch main
npm run cf:d1:migrate
npm run cf:deploy
npm --prefix frontend install
NEXT_PUBLIC_API_BASE=https://vespa-search-api.<your-subdomain>.workers.dev npm --prefix frontend run build
npm run cf:pages:deploy
```
