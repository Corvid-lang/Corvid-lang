# Shared App Template Security Model

- Default connector mode is mock.
- External writes must be approval-gated by app-specific slices.
- Trace fixtures must use fingerprints and redaction hashes.
- Runtime secrets belong in environment variables, not source files.
