use crate::errors::RuntimeError;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<HttpHeader>,
    pub body: String,
    pub attempts: u32,
    pub elapsed_ms: u64,
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
}
