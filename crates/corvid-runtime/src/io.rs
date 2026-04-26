use crate::errors::RuntimeError;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: PathBuf,
    pub contents: String,
    pub bytes: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: PathBuf,
    pub bytes: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Default)]
pub struct IoRuntime;

impl IoRuntime {
    pub fn new() -> Self {
        Self
    }

    pub async fn read_text(&self, path: impl AsRef<Path>) -> Result<FileRead, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        let started = Instant::now();
        let contents = tokio::fs::read_to_string(&path).await.map_err(|err| {
            RuntimeError::ToolFailed {
                tool: "std.io".to_string(),
                message: format!("failed to read `{}`: {err}", path.display()),
            }
        })?;
        Ok(FileRead {
            bytes: contents.len() as u64,
            contents,
            path,
            elapsed_ms: elapsed_ms(started),
        })
    }

    pub async fn write_text(
        &self,
        path: impl AsRef<Path>,
        contents: &str,
    ) -> Result<FileWrite, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        let started = Instant::now();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                RuntimeError::ToolFailed {
                    tool: "std.io".to_string(),
                    message: format!("failed to create `{}`: {err}", parent.display()),
                }
            })?;
        }
        tokio::fs::write(&path, contents).await.map_err(|err| {
            RuntimeError::ToolFailed {
                tool: "std.io".to_string(),
                message: format!("failed to write `{}`: {err}", path.display()),
            }
        })?;
        Ok(FileWrite {
            path,
            bytes: contents.len() as u64,
            elapsed_ms: elapsed_ms(started),
        })
    }

    pub async fn list_dir(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Vec<DirectoryEntry>, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        let mut entries = tokio::fs::read_dir(&path).await.map_err(|err| {
            RuntimeError::ToolFailed {
                tool: "std.io".to_string(),
                message: format!("failed to list `{}`: {err}", path.display()),
            }
        })?;
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|err| {
            RuntimeError::ToolFailed {
                tool: "std.io".to_string(),
                message: format!("failed to read directory entry in `{}`: {err}", path.display()),
            }
        })? {
            let file_type = entry.file_type().await.map_err(|err| RuntimeError::ToolFailed {
                tool: "std.io".to_string(),
                message: format!("failed to stat `{}`: {err}", entry.path().display()),
            })?;
            out.push(DirectoryEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                is_dir: file_type.is_dir(),
            });
        }
        out.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(out)
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn io_runtime_writes_reads_and_lists_text_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("note.txt");
        let io = IoRuntime::new();

        let write = io.write_text(&path, "hello").await.unwrap();
        assert_eq!(write.bytes, 5);

        let read = io.read_text(&path).await.unwrap();
        assert_eq!(read.contents, "hello");
        assert_eq!(read.bytes, 5);

        let entries = io.list_dir(path.parent().unwrap()).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "note.txt");
        assert!(!entries[0].is_dir);
    }
}
