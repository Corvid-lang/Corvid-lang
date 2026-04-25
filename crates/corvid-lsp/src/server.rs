use crate::{analyze_document, DocumentSnapshot};
use lsp_types::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, InitializeResult,
    PublishDiagnosticsParams, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, Url,
};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Response { id: Value, result: Value },
    Error { id: Option<Value>, code: i64, message: String },
    Notification { method: String, params: Value },
}

#[derive(Debug, Default)]
pub struct LanguageServerState {
    documents: HashMap<Url, String>,
    shutdown_requested: bool,
}

impl LanguageServerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, request: Value) -> Vec<ServerMessage> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = request.get("id").cloned();
        match method {
            "initialize" => self.initialize(id),
            "initialized" => Vec::new(),
            "shutdown" => {
                self.shutdown_requested = true;
                vec![ServerMessage::Response {
                    id: id.unwrap_or(Value::Null),
                    result: Value::Null,
                }]
            }
            "exit" => Vec::new(),
            "textDocument/didOpen" => self.did_open(request.get("params").cloned()),
            "textDocument/didChange" => self.did_change(request.get("params").cloned()),
            "textDocument/didSave" => self.did_save(request.get("params").cloned()),
            _ if id.is_some() => vec![ServerMessage::Error {
                id,
                code: -32601,
                message: format!("method `{method}` is not supported by corvid-lsp yet"),
            }],
            _ => Vec::new(),
        }
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    fn initialize(&self, id: Option<Value>) -> Vec<ServerMessage> {
        let result = InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
            server_info: Some(lsp_types::ServerInfo {
                name: "corvid-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        };
        vec![ServerMessage::Response {
            id: id.unwrap_or(Value::Null),
            result: serde_json::to_value(result).expect("initialize result serializes"),
        }]
    }

    fn did_open(&mut self, params: Option<Value>) -> Vec<ServerMessage> {
        let params = match parse_params::<DidOpenTextDocumentParams>(params) {
            Ok(params) => params,
            Err(error) => return vec![error],
        };
        let uri = params.text_document.uri;
        self.documents
            .insert(uri.clone(), params.text_document.text);
        self.publish_diagnostics(uri)
    }

    fn did_change(&mut self, params: Option<Value>) -> Vec<ServerMessage> {
        let params = match parse_params::<DidChangeTextDocumentParams>(params) {
            Ok(params) => params,
            Err(error) => return vec![error],
        };
        let Some(change) = params.content_changes.into_iter().last() else {
            return Vec::new();
        };
        let uri = params.text_document.uri;
        self.documents.insert(uri.clone(), change.text);
        self.publish_diagnostics(uri)
    }

    fn did_save(&mut self, params: Option<Value>) -> Vec<ServerMessage> {
        let params = match params {
            Some(params) => params,
            None => return Vec::new(),
        };
        let Some(uri) = params
            .get("textDocument")
            .and_then(|doc| doc.get("uri"))
            .and_then(Value::as_str)
            .and_then(|uri| Url::parse(uri).ok())
        else {
            return Vec::new();
        };
        self.publish_diagnostics(uri)
    }

    fn publish_diagnostics(&self, uri: Url) -> Vec<ServerMessage> {
        let Some(text) = self.documents.get(&uri) else {
            return Vec::new();
        };
        let analysis = analyze_document(&DocumentSnapshot {
            uri: uri.clone(),
            text: text.clone(),
        });
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics: analysis.diagnostics,
            version: None,
        };
        vec![ServerMessage::Notification {
            method: "textDocument/publishDiagnostics".to_string(),
            params: serde_json::to_value(params).expect("diagnostics params serialize"),
        }]
    }
}

impl ServerMessage {
    pub fn into_json(self) -> Value {
        match self {
            ServerMessage::Response { id, result } => {
                json!({ "jsonrpc": "2.0", "id": id, "result": result })
            }
            ServerMessage::Error { id, code, message } => {
                json!({
                    "jsonrpc": "2.0",
                    "id": id.unwrap_or(Value::Null),
                    "error": { "code": code, "message": message }
                })
            }
            ServerMessage::Notification { method, params } => {
                json!({ "jsonrpc": "2.0", "method": method, "params": params })
            }
        }
    }
}

fn parse_params<T>(params: Option<Value>) -> Result<T, ServerMessage>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(params.unwrap_or(Value::Null)).map_err(|error| ServerMessage::Error {
        id: None,
        code: -32602,
        message: format!("invalid LSP params: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did_open(uri: &str, text: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "corvid",
                    "version": 1,
                    "text": text
                }
            }
        })
    }

    #[test]
    fn initialize_advertises_full_text_sync() {
        let mut server = LanguageServerState::new();
        let messages = server.handle(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }));
        assert_eq!(messages.len(), 1);
        let json = messages.into_iter().next().unwrap().into_json();
        assert_eq!(json["id"], 1);
        assert_eq!(
            json["result"]["capabilities"]["textDocumentSync"].as_i64(),
            Some(1)
        );
    }

    #[test]
    fn did_open_publishes_compiler_diagnostics() {
        let mut server = LanguageServerState::new();
        let messages = server.handle(did_open(
            "file:///workspace/main.cor",
            "agent answer() -> Int:\n    return missing_name\n",
        ));
        assert_eq!(messages.len(), 1);
        let json = messages.into_iter().next().unwrap().into_json();
        assert_eq!(json["method"], "textDocument/publishDiagnostics");
        let diagnostics = json["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("missing_name"));
    }

    #[test]
    fn did_change_replaces_document_and_clears_diagnostics() {
        let mut server = LanguageServerState::new();
        server.handle(did_open(
            "file:///workspace/main.cor",
            "agent answer() -> Int:\n    return missing_name\n",
        ));
        let messages = server.handle(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": "file:///workspace/main.cor", "version": 2 },
                "contentChanges": [{ "text": "agent answer() -> Int:\n    return 42\n" }]
            }
        }));
        let json = messages.into_iter().next().unwrap().into_json();
        assert_eq!(
            json["params"]["diagnostics"].as_array().unwrap().len(),
            0
        );
    }
}
