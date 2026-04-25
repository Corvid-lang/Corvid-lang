use crate::{LanguageServerState, ServerMessage};
use serde_json::Value;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

#[derive(Debug)]
pub enum LspTransportError {
    Io(io::Error),
    Json(serde_json::Error),
    MissingContentLength,
    InvalidHeader(String),
}

impl fmt::Display for LspTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LspTransportError::Io(error) => write!(f, "LSP I/O error: {error}"),
            LspTransportError::Json(error) => write!(f, "LSP JSON error: {error}"),
            LspTransportError::MissingContentLength => write!(f, "missing Content-Length header"),
            LspTransportError::InvalidHeader(header) => write!(f, "invalid LSP header `{header}`"),
        }
    }
}

impl std::error::Error for LspTransportError {}

impl From<io::Error> for LspTransportError {
    fn from(value: io::Error) -> Self {
        LspTransportError::Io(value)
    }
}

impl From<serde_json::Error> for LspTransportError {
    fn from(value: serde_json::Error) -> Self {
        LspTransportError::Json(value)
    }
}

pub fn run_stdio_server() -> Result<(), LspTransportError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_server(stdin.lock(), stdout.lock())
}

pub fn run_server<R, W>(reader: R, mut writer: W) -> Result<(), LspTransportError>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(reader);
    let mut state = LanguageServerState::new();
    while let Some(message) = read_message(&mut reader)? {
        let should_exit = message.get("method").and_then(Value::as_str) == Some("exit");
        for response in state.handle(message) {
            write_message(&mut writer, response)?;
        }
        if should_exit && state.shutdown_requested() {
            break;
        }
    }
    Ok(())
}

fn read_message<R>(reader: &mut BufReader<R>) -> Result<Option<Value>, LspTransportError>
where
    R: Read,
{
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(LspTransportError::InvalidHeader(trimmed.to_string()));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| LspTransportError::InvalidHeader(trimmed.to_string()))?,
            );
        }
    }

    let len = content_length.ok_or(LspTransportError::MissingContentLength)?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

fn write_message<W>(writer: &mut W, message: ServerMessage) -> Result<(), LspTransportError>
where
    W: Write,
{
    let body = serde_json::to_vec(&message.into_json())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn framed_initialize_request_produces_framed_response() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        })
        .to_string();
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut output = Vec::new();
        run_server(input.as_bytes(), &mut output).expect("run lsp server");
        let out = String::from_utf8(output).unwrap();
        assert!(out.starts_with("Content-Length: "));
        assert!(out.contains("\"id\":1"));
        assert!(out.contains("\"capabilities\""));
    }
}
