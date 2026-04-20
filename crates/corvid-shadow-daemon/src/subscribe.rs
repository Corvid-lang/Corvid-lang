use async_trait::async_trait;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[async_trait]
pub trait TraceSubscription: Send + Sync {
    async fn next(&mut self) -> Option<PathBuf>;
}

pub struct FileWatchSubscription {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<PathBuf>,
}

impl FileWatchSubscription {
    pub fn watch(dir: &Path, debounce_ms: u64) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel(128);
        let seen = Arc::new(Mutex::new(HashMap::<PathBuf, Instant>::new()));
        let debounce = Duration::from_millis(debounce_ms.max(1));
        let seen_handle = seen.clone();
        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    return;
                }
                for path in event.paths {
                    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let now = Instant::now();
                    let mut seen = seen_handle.lock().unwrap();
                    let is_debounced = seen
                        .get(&path)
                        .map(|last| now.duration_since(*last) < debounce)
                        .unwrap_or(false);
                    if is_debounced {
                        continue;
                    }
                    seen.insert(path.clone(), now);
                    let _ = tx.blocking_send(path);
                }
            }
        })
        .map_err(|err| format!("failed to create trace watcher: {err}"))?;

        let mut this = Self { _watcher: watcher, rx };
        Watcher::watch(&mut this._watcher, dir, RecursiveMode::NonRecursive)
            .map_err(|err| format!("failed to watch `{}`: {err}", dir.display()))?;
        Ok(this)
    }
}

#[async_trait]
impl TraceSubscription for FileWatchSubscription {
    async fn next(&mut self) -> Option<PathBuf> {
        self.rx.recv().await
    }
}

#[derive(Default)]
pub struct QueueSubscription {
    queue: VecDeque<PathBuf>,
}

impl QueueSubscription {
    pub fn push(&mut self, path: PathBuf) {
        self.queue.push_back(path);
    }
}

#[async_trait]
impl TraceSubscription for QueueSubscription {
    async fn next(&mut self) -> Option<PathBuf> {
        self.queue.pop_front()
    }
}
