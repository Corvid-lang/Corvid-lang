# Personal Knowledge Agent Backend

This Phase 42 reference app is a backend-only private knowledge agent. 42D1
ships the ingestion half: local file mock connector data, provenance-preserving
documents/chunks, local embeddings, SQLite migrations, seed data, and a Corvid
server route that proves ingestion works in private/local mode.

## Routes

- `GET /config`
- `GET /ingest/mock`

## Ingestion Contract

- The default connector mode is `mock`.
- Embeddings are marked `local_only`.
- Sources, documents, chunks, and embeddings preserve a stable provenance ID.
- Committed fixtures contain hashes and fingerprints, not raw private text.
