use crate::errors::RuntimeError;
use sha2::{Digest, Sha256};
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRetryPolicy {
    pub max_retries: u32,
    pub retry_on_5xx: bool,
}

impl Default for HttpRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            retry_on_5xx: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<String>,
    pub timeout_ms: u64,
    pub retry: HttpRetryPolicy,
    pub effect_tag: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".to_string(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: 30_000,
            retry: HttpRetryPolicy::default(),
            effect_tag: None,
        }
    }

    pub fn post_json(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            method: "POST".to_string(),
            url: url.into(),
            headers: vec![HttpHeader {
                name: "content-type".to_string(),
                value: "application/json".to_string(),
            }],
            body: Some(body.into()),
            timeout_ms: 30_000,
            retry: HttpRetryPolicy::default(),
            effect_tag: None,
        }
    }

    pub fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    pub fn retry(mut self, retry: HttpRetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push(HttpHeader {
            name: name.into(),
            value: value.into(),
        });
        self
    }

    pub fn effect_tag(mut self, effect_tag: impl Into<String>) -> Self {
        self.effect_tag = Some(effect_tag.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: String,
    pub attempts: u32,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedHttpExchange {
    pub request_fingerprint: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub attempts: u32,
    pub effect_tag: Option<String>,
    pub response_body: String,
}

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn send(&self, request: &HttpRequest) -> Result<HttpResponse, RuntimeError> {
        let started = Instant::now();
        let attempts_allowed = request.retry.max_retries.saturating_add(1);
        let mut attempt = 0;
        let mut last_error = None;
        while attempt < attempts_allowed {
            attempt += 1;
            match self.send_once(request).await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let headers = response_headers(response.headers());
                    let body = response.text().await.map_err(|err| RuntimeError::ToolFailed {
                        tool: "std.http".to_string(),
                        message: format!("failed to read HTTP response body: {err}"),
                    })?;
                    let should_retry =
                        request.retry.retry_on_5xx && status >= 500 && attempt < attempts_allowed;
                    if should_retry {
                        last_error = Some(format!("HTTP {status}"));
                        continue;
                    }
                    return Ok(HttpResponse {
                        status,
                        headers,
                        body,
                        attempts: attempt,
                        elapsed_ms: elapsed_ms(started),
                    });
                }
                Err(err) if attempt < attempts_allowed => {
                    last_error = Some(err.to_string());
                }
                Err(err) => return Err(err),
            }
        }
        Err(RuntimeError::ToolFailed {
            tool: "std.http".to_string(),
            message: last_error.unwrap_or_else(|| "HTTP request failed".to_string()),
        })
    }

    pub async fn send_recorded(
        &self,
        request: &HttpRequest,
    ) -> Result<(HttpResponse, RecordedHttpExchange), RuntimeError> {
        let response = self.send(request).await?;
        let record = record_exchange(request, &response);
        Ok((response, record))
    }

    async fn send_once(
        &self,
        request: &HttpRequest,
    ) -> Result<reqwest::Response, RuntimeError> {
        let method = request.method.parse::<reqwest::Method>().map_err(|err| {
            RuntimeError::ToolFailed {
                tool: "std.http".to_string(),
                message: format!("invalid HTTP method `{}`: {err}", request.method),
            }
        })?;
        let mut builder = self
            .client
            .request(method, &request.url)
            .timeout(Duration::from_millis(request.timeout_ms.max(1)));
        for header in &request.headers {
            builder = builder.header(&header.name, &header.value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        builder.send().await.map_err(|err| RuntimeError::ToolFailed {
            tool: "std.http".to_string(),
            message: err.to_string(),
        })
    }
}

fn response_headers(headers: &reqwest::header::HeaderMap) -> Vec<HttpHeader> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|value| HttpHeader {
                name: name.as_str().to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

pub fn record_exchange(request: &HttpRequest, response: &HttpResponse) -> RecordedHttpExchange {
    RecordedHttpExchange {
        request_fingerprint: request_fingerprint(request),
        method: request.method.clone(),
        url: request.url.clone(),
        status: response.status,
        attempts: response.attempts,
        effect_tag: request.effect_tag.clone(),
        response_body: response.body.clone(),
    }
}

pub fn request_fingerprint(request: &HttpRequest) -> String {
    let mut headers = request.headers.clone();
    headers.sort_by(|left, right| left.name.cmp(&right.name).then(left.value.cmp(&right.value)));
    let canonical = serde_json::json!({
        "method": request.method,
        "url": request.url,
        "headers": headers,
        "body": request.body,
        "timeout_ms": request.timeout_ms,
        "retry": {
            "max_retries": request.retry.max_retries,
            "retry_on_5xx": request.retry.retry_on_5xx,
        },
        "effect_tag": request.effect_tag,
    });
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string().as_bytes());
    encode_hex(&hasher.finalize())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn http_client_gets_text_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/hello"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .mount(&server)
            .await;

        let response = HttpClient::new()
            .send(&HttpRequest::get(format!("{}/hello", server.uri())))
            .await
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello");
        assert_eq!(response.attempts, 1);
    }

    #[tokio::test]
    async fn http_client_retries_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/flaky"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/flaky"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let response = HttpClient::new()
            .send(
                &HttpRequest::get(format!("{}/flaky", server.uri())).retry(HttpRetryPolicy {
                    max_retries: 1,
                    retry_on_5xx: true,
                }),
            )
            .await
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert_eq!(response.attempts, 2);
    }

    #[tokio::test]
    async fn http_client_records_exchange_with_effect_tag() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/recorded"))
            .respond_with(ResponseTemplate::new(200).set_body_string("captured"))
            .mount(&server)
            .await;

        let request = HttpRequest::get(format!("{}/recorded", server.uri())).effect_tag("network:http");
        let (response, record) = HttpClient::new().send_recorded(&request).await.unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(record.status, 200);
        assert_eq!(record.effect_tag.as_deref(), Some("network:http"));
        assert_eq!(record.response_body, "captured");
        assert_eq!(record.request_fingerprint.len(), 64);
    }
}
