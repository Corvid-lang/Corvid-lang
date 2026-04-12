//! Type system and effect checker.
//!
//! Enforces the core invariant: irreversible tools require a prior
//! `approve` in the same block. See `ARCHITECTURE.md` §5.

#![allow(dead_code)]
