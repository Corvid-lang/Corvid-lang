//! Integration tests for `OpenAiAdapter` against a wiremock server.
//! Proves request shape (including `response_format` for structured output),
//! `Authorization: Bearer` header, and JSON content extraction.

use corvid_runtime::{LlmAdapter, LlmRequest, OpenAiAdapter};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn structured_call_sends_response_format_and_parses_json_content() {
    let server = MockServer::start().await;

    let response = ResponseTemplate::new(200).set_body_json(json!({
        "id": "chatcmpl_test",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"should_refund\": true, \"reason\": \"legit complaint\"}"
                },
                "finish_reason": "stop"
            }
        ]
    }));

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer sk-test-openai"))
        .respond_with(response)
        .mount(&server)
        .await;

    let adapter =
        OpenAiAdapter::new("sk-test-openai").with_base_url(server.uri());
    let req = LlmRequest {
        prompt: "decide".into(),
        model: "gpt-4o-mini".into(),
        rendered: "Decide.".into(),
        args: vec![],
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "should_refund": {"type": "boolean"},
                "reason": {"type": "string"}
            },
            "required": ["should_refund", "reason"],
            "additionalProperties": false
        })),
    };

    let resp = adapter.call(&req.as_ref()).await.expect("adapter call");
    assert_eq!(
        resp.value,
        json!({"should_refund": true, "reason": "legit complaint"})
    );

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("request body is JSON");
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["messages"][0]["content"], "Decide.");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "respond_with_decide"
    );
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
}

#[tokio::test]
async fn unstructured_call_returns_raw_string() {
    let server = MockServer::start().await;
    let response = ResponseTemplate::new(200).set_body_json(json!({
        "choices": [
            {"message": {"role": "assistant", "content": "hello world"}}
        ]
    }));
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(response)
        .mount(&server)
        .await;

    let adapter = OpenAiAdapter::new("k").with_base_url(server.uri());
    let req = LlmRequest {
        prompt: "chat".into(),
        model: "gpt-4o-mini".into(),
        rendered: "say hi".into(),
        args: vec![],
        output_schema: None,
    };
    let resp = adapter.call(&req.as_ref()).await.unwrap();
    assert_eq!(resp.value, json!("hello world"));
}

#[tokio::test]
async fn http_error_surfaces_as_adapter_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let adapter = OpenAiAdapter::new("k").with_base_url(server.uri());
    let req = LlmRequest {
        prompt: "x".into(),
        model: "gpt-4o-mini".into(),
        rendered: "".into(),
        args: vec![],
        output_schema: None,
    };
    let err = adapter.call(&req.as_ref()).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("openai"));
    assert!(msg.contains("429"));
}
