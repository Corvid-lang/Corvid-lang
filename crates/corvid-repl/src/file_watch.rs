//! Hot-reload file watcher: watches imported `.cor` files for changes
//! and triggers automatic redefinition in the REPL session.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Manages watched file paths and delivers change notifications.
pub struct FileWatchManager {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<PathBuf>,
    watched: HashMap<PathBuf, WatchedFile>,
}

struct WatchedFile {
    last_notified: Instant,
}

/// A file change that needs processing.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
}

impl FileWatchManager {
    pub fn new() -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_)
                ) {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            }
        })
        .map_err(|e| format!("failed to create file watcher: {e}"))?;

        Ok(Self {
            _watcher: watcher,
            rx,
            watched: HashMap::new(),
        })
    }

    /// Start watching a file path for changes.
    pub fn watch(&mut self, path: &Path) -> Result<(), String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("cannot resolve `{}`: {e}", path.display()))?;

        if self.watched.contains_key(&canonical) {
            return Ok(());
        }

        // Watch the parent directory (file-level watching is unreliable
        // on some platforms because editors write to temp files then rename).
        let watch_dir = canonical
            .parent()
            .ok_or_else(|| format!("cannot determine parent of `{}`", canonical.display()))?;

        // Watcher is behind &mut — need to access it.
        // notify's RecommendedWatcher implements Watcher, so we call watch().
        Watcher::watch(&mut self._watcher, watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("failed to watch `{}`: {e}", watch_dir.display()))?;

        self.watched.insert(
            canonical,
            WatchedFile {
                last_notified: Instant::now() - Duration::from_secs(10),
            },
        );
        Ok(())
    }

    /// Check for pending file changes (non-blocking). Returns changed
    /// files that we're watching, debounced to avoid rapid-fire re-imports.
    pub fn poll_changes(&mut self) -> Vec<FileChange> {
        let mut changes = Vec::new();
        let now = Instant::now();
        let debounce = Duration::from_millis(500);

        while let Ok(path) = self.rx.try_recv() {
            let canonical = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Some(watched) = self.watched.get_mut(&canonical) {
                if now.duration_since(watched.last_notified) >= debounce {
                    watched.last_notified = now;
                    changes.push(FileChange {
                        path: canonical.clone(),
                    });
                }
            }
        }

        changes.dedup_by(|a, b| a.path == b.path);
        changes
    }

    pub fn is_watching(&self, path: &Path) -> bool {
        path.canonicalize()
            .map(|c| self.watched.contains_key(&c))
            .unwrap_or(false)
    }

    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched.keys().cloned().collect()
    }
}
