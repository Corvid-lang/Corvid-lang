//! Corvid runtime.
//!
//! Provides the glue that generated Python/WASM code calls into:
//! LLM abstraction, tool dispatch, approval flow, trace emission,
//! and memory primitives.
//!
//! See `ARCHITECTURE.md` §6.

#![allow(dead_code)]
