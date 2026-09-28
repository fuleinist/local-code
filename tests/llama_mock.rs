//! AC8: --backend llama sends OpenAI-compatible requests. A minimal in-test
//! HTTP server speaks SSE like llama-server / vLLM and we assert both the
//! request shape and the streamed output.

use std::process::Command;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread")]
async fn llama_backend_streams_openai_sse() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let n = sock.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();

        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" from\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" llama\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
        req
    });

    let db = std::env::temp_dir().join(format!("lc-llama-test-{port}.db"));
    let db2 = db.clone();
    let url = format!("http://127.0.0.1:{port}/v1");
    let out = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_local-code"))
            .args([
                "say hello",
                "--backend",
                "llama",
                "--model",
                "test-model",
                "--server-url",
                &url,
            ])
            .env("LOCAL_CODE_DB", &db2)
            .output()
            .expect("failed to run local-code binary")
    })
    .await
    .unwrap();
    let _ = std::fs::remove_file(&db);

    let req = server.await.unwrap();

    // Request shape: OpenAI-compatible chat completion.
    assert!(
        req.starts_with("POST /v1/chat/completions"),
        "unexpected request line: {}",
        req.lines().next().unwrap_or("")
    );
    assert!(req.contains("\"stream\":true"), "body missing stream flag: {req}");
    assert!(req.contains("\"model\":\"test-model\""), "body missing model: {req}");
    assert!(req.contains("\"role\":\"system\""), "body missing system message: {req}");
    assert!(req.contains("say hello"), "body missing user prompt: {req}");

    // Response streamed to stdout.
    assert!(
        out.status.success(),
        "exit failed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Hello from llama"),
        "streamed output missing: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn llama_backend_http_error_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16384];
        let _ = sock.read(&mut buf).await.unwrap();
        let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 9\r\nConnection: close\r\n\r\nboom fail";
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
    });

    let db = std::env::temp_dir().join(format!("lc-llama-err-{port}.db"));
    let db2 = db.clone();
    let url = format!("http://127.0.0.1:{port}/v1");
    let out = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_local-code"))
            .args([
                "hi",
                "--backend",
                "llama",
                "--model",
                "m",
                "--server-url",
                &url,
            ])
            .env("LOCAL_CODE_DB", &db2)
            .output()
            .expect("failed to run local-code binary")
    })
    .await
    .unwrap();
    let _ = std::fs::remove_file(&db);
    server.await.unwrap();

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("500"), "stderr should mention HTTP status: {stderr}");
}
