# Personal Knowledge Agent Backend

This Phase 42 reference app is a backend-only private knowledge agent. 42D1
ships the ingestion half: local file mock connector data, provenance-preserving
documents/chunks, local embeddings, SQLite migrations, seed data, and a Corvid
server route that proves ingestion works in private/local mode.

## Routes

- `GET /config`
- `GET /ingest/mock`
- `GET /search/mock`
- `GET /answer/mock`
- `GET /feedback/eval/mock`

## Ingestion Contract

- The default connector mode is `mock`.
- Embeddings are marked `local_only`.
- Sources, documents, chunks, and embeddings preserve a stable provenance ID.
- Committed fixtures contain hashes and fingerprints, not raw private text.

## Search And Answer Contract

42D2 adds grounded retrieval and feedback evals:

- Search hits must carry citations with byte ranges and provenance IDs.
- Answers must preserve the citation provenance ID and stay in local/private
  mode.
- Feedback records can be promoted into deterministic eval fixtures.
- `evals/search_answer_eval.cor` and `traces/demo.lineage.jsonl` prove the
  search/answer path without committing raw document text.
