//! Heap-cell metadata for the cycle-collector-tracked
//! refcounted value families (`Struct`, `List`, `Boxed`).
//!
//! Each `Arc<...Inner>` heap cell embeds a `HeapMeta` carrying:
//!
//! - `strong`: the user-visible refcount the language operates
//!   against (separate from the `Arc`'s internal one because the
//!   collector re-targets `strong` during cycle reclamation while
//!   the `Arc` keeps its own books).
//! - `shadow`: the temporary "buffered candidate" count Bacon &
//!   Rajan's deferred reference counting algorithm uses to
//!   distinguish suspected cycles from collected ones.
//! - `color`: the four-color trial-deletion state machine
//!   (`Black` = certainly live, `Gray` = under exam, `White` =
//!   tentatively dead, `Purple` = candidate root).
//! - `buffered`: a fast skip flag the collector consults to
//!   avoid double-buffering a candidate root.
//!
//! All four counters use atomic ordering chosen to pair with the
//! collector's traversal (Release on writes that publish
//! state to other threads, Acquire on reads that consume it).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Color {
    Black = 0,
    Gray = 1,
    White = 2,
    Purple = 3,
}

#[derive(Debug)]
pub(super) struct HeapMeta {
    strong: AtomicUsize,
    shadow: AtomicUsize,
    color: AtomicU8,
    buffered: AtomicBool,
}

impl HeapMeta {
    pub(super) fn new() -> Self {
        Self {
            strong: AtomicUsize::new(1),
            shadow: AtomicUsize::new(0),
            color: AtomicU8::new(Color::Black as u8),
            buffered: AtomicBool::new(false),
        }
    }

    pub(super) fn retain(&self) {
        self.strong.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn release(&self) -> usize {
        self.strong.fetch_sub(1, Ordering::AcqRel)
    }

    pub(super) fn strong_count(&self) -> usize {
        self.strong.load(Ordering::Acquire)
    }

    pub(super) fn set_strong(&self, value: usize) {
        self.strong.store(value, Ordering::Release);
    }

    pub(super) fn shadow_count(&self) -> usize {
        self.shadow.load(Ordering::Acquire)
    }

    pub(super) fn set_shadow(&self, value: usize) {
        self.shadow.store(value, Ordering::Release);
    }

    pub(super) fn inc_shadow(&self) {
        self.shadow.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn dec_shadow(&self) {
        let old = self.shadow.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(old > 0, "shadow count underflow");
    }

    pub(super) fn color(&self) -> Color {
        match self.color.load(Ordering::Acquire) {
            0 => Color::Black,
            1 => Color::Gray,
            2 => Color::White,
            3 => Color::Purple,
            other => panic!("invalid heap color {other}"),
        }
    }

    pub(super) fn set_color(&self, color: Color) {
        self.color.store(color as u8, Ordering::Release);
    }

    pub(super) fn buffered(&self) -> bool {
        self.buffered.load(Ordering::Acquire)
    }

    pub(super) fn set_buffered(&self, value: bool) {
        self.buffered.store(value, Ordering::Release);
    }
}
