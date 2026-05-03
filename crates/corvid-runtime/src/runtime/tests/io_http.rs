use super::*;
#[tokio::test]
async fn http_request_emits_trace_events() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/status"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("http.jsonl");
    let r = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "http-run"))
        .build();

    let response = r
        .http_request(crate::http::HttpRequest::get(format!(
            "{}/status",
            server.uri()
        )))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "ok");

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.http.request" && payload["method"] == "GET"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.http.response" && payload["status"] == 200
    )));
}

#[tokio::test]
async fn file_io_emits_trace_events() {
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("io.jsonl");
    let file_path = dir.path().join("data").join("note.txt");
    let r = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "io-run"))
        .build();

    let write = r.write_text_file(&file_path, "hello").await.unwrap();
    assert_eq!(write.bytes, 5);
    let read = r.read_text_file(&file_path).await.unwrap();
    assert_eq!(read.contents, "hello");
    let entries = r.list_dir(file_path.parent().unwrap()).await.unwrap();
    assert_eq!(entries.len(), 1);

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.io.write.result" && payload["bytes"] == 5
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.io.read.result" && payload["bytes"] == 5
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.io.list.result" && payload["entries"] == 1
    )));
}
