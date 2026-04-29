# Local Files Connector

The local files connector provides indexed folder metadata and provenance
snippets. It is designed for personal knowledge agents where file reads must be
auditable and replayable.

## Configuration

```sh
CORVID_FILES_ROOTS=docs=./docs,notes=./notes
CORVID_CONNECTOR_MODE=mock|replay|real
```

Read scopes:

```text
files.read
```

## Mock Mode

Mock operations:

- `index`
- `read`

Read responses include `provenance_id` with root, path, content hash, and byte
range.

## Replay Keys

- Index: `files:index:<root_id>:<stable-glob>`
- Read: `files:read:<root_id>:<stable-path>`

Writes land in 41H2 and require approval.
