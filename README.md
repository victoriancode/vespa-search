# Vespa Code Search (Rust + NextJS)

This repository contains a high-level plan for building a Vespa-backed code search engine with
Rust-based ingestion/query services and a NextJS frontend.

## Contents
- `PLAN.md`: architecture, workflow, and performance plan.

## Quick Start
- Read `PLAN.md` for the full design and implementation plan.

## Notes
- Each indexed repository should include a `vv/` folder that stores vectors and repo metadata.
- Vespa is the primary search engine for ANN-based dense embedding search.
