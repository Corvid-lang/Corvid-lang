INSERT INTO knowledge_sources
    (id, tenant_id, root_id, path, media_type, content_hash, provenance_id)
VALUES
    ('source-1', 'tenant-1', 'notes', 'notes/strategy.md', 'text/markdown', 'sha256:source-demo', 'files:notes:notes/strategy.md:0:512');

INSERT INTO knowledge_documents
    (id, tenant_id, source_id, title_fingerprint, content_hash, version, provenance_id)
VALUES
    ('doc-1', 'tenant-1', 'source-1', 'sha256:title-demo', 'sha256:source-demo', 1, 'files:notes:notes/strategy.md:0:512');

INSERT INTO knowledge_chunks
    (id, tenant_id, document_id, source_id, byte_start, byte_end, text_fingerprint, provenance_id)
VALUES
    ('chunk-1', 'tenant-1', 'doc-1', 'source-1', 0, 512, 'sha256:chunk-text-demo', 'files:notes:notes/strategy.md:0:512');

INSERT INTO knowledge_embeddings
    (id, chunk_id, provider, model, vector_hash, local_only)
VALUES
    ('embedding-1', 'chunk-1', 'local', 'bge-small-en', 'sha256:vector-demo', 1);

INSERT INTO knowledge_ingestion_jobs
    (id, tenant_id, root_id, status, discovered_count, indexed_count, replay_key)
VALUES
    ('ingest-1', 'tenant-1', 'notes', 'indexed', 1, 1, 'knowledge:ingest:notes:strategy');
