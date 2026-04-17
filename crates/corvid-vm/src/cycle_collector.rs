//! Bacon-Rajan trial deletion for the interpreter-owned heap graph.
//!
//! This collector is VM-only. The native tier uses the C runtime's
//! mark-sweep collector over its own heap. Parity is behavioural, not
//! implementation-sharing.

use crate::value::{Color, ObjectRef, WeakObjectRef};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

struct CollectorState {
    roots: Vec<WeakObjectRef>,
    collecting: bool,
}

static STATE: OnceLock<Mutex<CollectorState>> = OnceLock::new();
static SUPPRESS_RELEASE_DEPTH: AtomicUsize = AtomicUsize::new(0);

fn state() -> &'static Mutex<CollectorState> {
    STATE.get_or_init(|| {
        Mutex::new(CollectorState {
            roots: Vec::new(),
            collecting: false,
        })
    })
}

fn auto_trigger() -> usize {
    std::env::var("CORVID_VM_GC_TRIGGER")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(10_000)
}

fn release_suppressed() -> bool {
    SUPPRESS_RELEASE_DEPTH.load(Ordering::Acquire) > 0
}

struct SuppressReleaseGuard;

impl SuppressReleaseGuard {
    fn new() -> Self {
        SUPPRESS_RELEASE_DEPTH.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for SuppressReleaseGuard {
    fn drop(&mut self) {
        SUPPRESS_RELEASE_DEPTH.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn release_object(object: ObjectRef) {
    if release_suppressed() {
        return;
    }

    let old = object.release_strong();
    if old == 1 {
        object.free_zero_path();
        return;
    }

    let should_collect = {
        let mut guard = state().lock().expect("cycle collector lock poisoned");
        if !object.buffered() {
            object.set_buffered(true);
            object.set_color(Color::Purple);
            guard.roots.push(object.downgrade());
        }
        let trigger = auto_trigger();
        !guard.collecting && trigger > 0 && guard.roots.len() >= trigger
    };

    if should_collect {
        let _ = collect_cycles();
    }
}

pub fn collect_cycles() -> usize {
    if release_suppressed() {
        return 0;
    }

    let roots = {
        let mut guard = state().lock().expect("cycle collector lock poisoned");
        if guard.collecting {
            return 0;
        }
        guard.collecting = true;
        guard.roots.clone()
    };

    for root in &roots {
        if let Some(object) = root.upgrade() {
            if object.buffered() && object.strong_count() > 0 {
                mark_gray(&object);
            }
        }
    }

    for root in &roots {
        if let Some(object) = root.upgrade() {
            scan(&object);
        }
    }

    let mut condemned = Vec::new();
    let mut seen = HashSet::new();
    for root in &roots {
        if let Some(object) = root.upgrade() {
            collect_white(&object, &mut seen, &mut condemned);
        }
    }
    let collected = condemned.len();

    if !condemned.is_empty() {
        let _suppress = SuppressReleaseGuard::new();
        for object in &condemned {
            object.prepare_collect();
        }
        for object in &condemned {
            object.clear_payload();
        }
    }

    let mut guard = state().lock().expect("cycle collector lock poisoned");
    guard.roots.clear();
    guard.collecting = false;
    collected
}

enum MarkTask {
    Visit(ObjectRef),
    DecShadow(ObjectRef),
}

fn mark_gray(object: &ObjectRef) {
    let mut stack = vec![MarkTask::Visit(object.clone())];
    while let Some(task) = stack.pop() {
        match task {
            MarkTask::Visit(object) => {
                if object.color() == Color::Gray {
                    continue;
                }
                object.set_color(Color::Gray);
                object.set_shadow(object.strong_count());
                let children = object.children();
                for child in children.into_iter().rev() {
                    stack.push(MarkTask::DecShadow(child.clone()));
                    stack.push(MarkTask::Visit(child));
                }
            }
            MarkTask::DecShadow(child) => child.dec_shadow(),
        }
    }
}

fn scan(object: &ObjectRef) {
    let mut stack = vec![ScanTask::Scan(object.clone())];
    while let Some(task) = stack.pop() {
        match task {
            ScanTask::Scan(object) => {
                if object.color() != Color::Gray {
                    continue;
                }

                if object.shadow_count() > 0 {
                    stack.push(ScanTask::ScanBlack(object));
                } else {
                    object.set_color(Color::White);
                    for child in object.children().into_iter().rev() {
                        stack.push(ScanTask::Scan(child));
                    }
                }
            }
            ScanTask::ScanBlack(object) => {
                if object.color() == Color::Black {
                    object.set_buffered(false);
                    continue;
                }

                object.set_color(Color::Black);
                object.set_buffered(false);
                let children = object.children();
                for child in children.into_iter().rev() {
                    child.inc_shadow();
                    stack.push(ScanTask::ScanBlack(child));
                }
            }
        }
    }
}

enum ScanTask {
    Scan(ObjectRef),
    ScanBlack(ObjectRef),
}

fn collect_white(
    object: &ObjectRef,
    seen: &mut HashSet<usize>,
    out: &mut Vec<ObjectRef>,
) {
    let mut stack = vec![object.clone()];
    while let Some(object) = stack.pop() {
        if !seen.insert(object.ptr_key()) {
            continue;
        }

        if object.color() == Color::White {
            out.push(object.clone());
            for child in object.children().into_iter().rev() {
                stack.push(child);
            }
        } else {
            object.set_color(Color::Black);
            object.set_buffered(false);
        }
    }
}
