use corvid_trace_schema::{append_event, TraceEvent};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct JsonlTraceWriter {
    inner: Arc<JsonlTraceWriterInner>,
}

struct JsonlTraceWriterInner {
    path: PathBuf,
    file: Mutex<Option<BufWriter<std::fs::File>>>,
}

impl JsonlTraceWriter {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let file = (|| -> std::io::Result<std::fs::File> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        })()
        .ok()
        .map(BufWriter::new);
        Self {
            inner: Arc::new(JsonlTraceWriterInner {
                path,
                file: Mutex::new(file),
            }),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.inner.path
    }

    pub(crate) fn is_enabled(&self) -> bool {
        if let Ok(guard) = self.inner.file.lock() {
            guard.is_some()
        } else {
            false
        }
    }

    pub(crate) fn append(&self, event: &TraceEvent) {
        if let Ok(mut guard) = self.inner.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = append_event(file, event);
                let _ = file.flush();
            }
        }
    }
}
