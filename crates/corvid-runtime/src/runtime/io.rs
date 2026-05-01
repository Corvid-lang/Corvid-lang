//! I/O dispatch methods on `Runtime` — HTTP requests, text-file
//! read/write/list, and env-secret reads. Each method emits paired
//! request/response/error host events so traces show the I/O
//! attempt regardless of outcome, and delegates the actual work to
//! the `HttpClient` / `IoRuntime` / `SecretRuntime` collaborators
//! held on `Runtime`.

use std::path::Path;

use crate::errors::RuntimeError;
use crate::http::{HttpRequest, HttpResponse};
use crate::io::{DirectoryEntry, FileRead, FileWrite};
use crate::secrets::SecretRead;

use super::Runtime;

impl Runtime {
    pub async fn http_request(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeError> {
        self.emit_host_event(
            "std.http.request",
            serde_json::json!({
                "method": request.method.clone(),
                "url": request.url.clone(),
                "timeout_ms": request.timeout_ms,
                "max_retries": request.retry.max_retries,
                "body_bytes": request.body.as_ref().map(|body| body.len()).unwrap_or(0),
            }),
        );
        match self.http.send(&request).await {
            Ok(response) => {
                self.emit_host_event(
                    "std.http.response",
                    serde_json::json!({
                        "method": request.method.clone(),
                        "url": request.url.clone(),
                        "status": response.status,
                        "attempts": response.attempts,
                        "elapsed_ms": response.elapsed_ms,
                        "body_bytes": response.body.len(),
                    }),
                );
                Ok(response)
            }
            Err(err) => {
                self.emit_host_event(
                    "std.http.error",
                    serde_json::json!({
                        "method": request.method.clone(),
                        "url": request.url.clone(),
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }

    pub async fn read_text_file(&self, path: impl AsRef<Path>) -> Result<FileRead, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        self.emit_host_event(
            "std.io.read",
            serde_json::json!({
                "path": path.display().to_string(),
            }),
        );
        match self.io.read_text(&path).await {
            Ok(read) => {
                self.emit_host_event(
                    "std.io.read.result",
                    serde_json::json!({
                        "path": read.path.display().to_string(),
                        "bytes": read.bytes,
                        "elapsed_ms": read.elapsed_ms,
                    }),
                );
                Ok(read)
            }
            Err(err) => {
                self.emit_host_event(
                    "std.io.error",
                    serde_json::json!({
                        "op": "read",
                        "path": path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }

    pub async fn write_text_file(
        &self,
        path: impl AsRef<Path>,
        contents: &str,
    ) -> Result<FileWrite, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        self.emit_host_event(
            "std.io.write",
            serde_json::json!({
                "path": path.display().to_string(),
                "bytes": contents.len(),
            }),
        );
        match self.io.write_text(&path, contents).await {
            Ok(write) => {
                self.emit_host_event(
                    "std.io.write.result",
                    serde_json::json!({
                        "path": write.path.display().to_string(),
                        "bytes": write.bytes,
                        "elapsed_ms": write.elapsed_ms,
                    }),
                );
                Ok(write)
            }
            Err(err) => {
                self.emit_host_event(
                    "std.io.error",
                    serde_json::json!({
                        "op": "write",
                        "path": path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }

    pub async fn list_dir(&self, path: impl AsRef<Path>) -> Result<Vec<DirectoryEntry>, RuntimeError> {
        let path = path.as_ref().to_path_buf();
        self.emit_host_event(
            "std.io.list",
            serde_json::json!({
                "path": path.display().to_string(),
            }),
        );
        match self.io.list_dir(&path).await {
            Ok(entries) => {
                self.emit_host_event(
                    "std.io.list.result",
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "entries": entries.len(),
                    }),
                );
                Ok(entries)
            }
            Err(err) => {
                self.emit_host_event(
                    "std.io.error",
                    serde_json::json!({
                        "op": "list",
                        "path": path.display().to_string(),
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }

    pub fn read_env_secret(&self, name: &str) -> Result<SecretRead, RuntimeError> {
        match self.secrets.read_env(name) {
            Ok(read) => {
                self.emit_host_event(
                    "std.secrets.read",
                    serde_json::json!({
                        "name": read.name.clone(),
                        "present": read.present,
                        "value_redacted": read.present,
                    }),
                );
                Ok(read)
            }
            Err(err) => {
                self.emit_host_event(
                    "std.secrets.error",
                    serde_json::json!({
                        "name": name,
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }
}
