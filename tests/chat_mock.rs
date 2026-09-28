//! F4 chat mode: piped-stdin REPL against a mock llama.cpp SSE server.
//! Verifies multi-turn message accumulation and slash commands.

use std::io::Write;
use std::process::{Command, Stdio};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread")]
async fn chat_repl_two_turns_and_quit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // Serve two chat completions; capture both request bodies.
    let server = tokio::spawn(async move {
        let mut reqs = Vec::new();
        for turn in 0..2 {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let n = sock.read(&mut buf).await.unwrap();
            reqs.push(String::from_utf8_lossy(&buf[..n]).to_string());
            let body = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"reply{}\"}}}}]}}\n\ndata: [DONE]\n\n",
                turn + 1
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        }
        reqs
    });

    let db = std::env::temp_dir().join(format!("lc-chat-test-{port}.db"));
    let url = format!("http://127.0.0.1:{port}/v1");
    let db2 = db.clone();
    let out = tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_local-code"))
            .args([
                "chat",
                "--backend",
                "llama",
                "--model",
                "chat-model",
                "--server-url",
                &url,
            ])
            .env("LOCAL_CODE_DB", &db2)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn local-code");
        {
            let mut stdin = child.stdin.take().unwrap();
            writeln!(stdin, "first question").unwrap();
            stdin.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            writeln!(stdin, "/model other-model").unwrap();
            writeln!(stdin, "second question").unwrap();
            stdin.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            writeln!(stdin, "/quit").unwrap();
            stdin.flush().unwrap();
        }
        child.wait_with_output().expect("failed to wait")
    })
    .await
    .unwrap();
    let _ = std::fs::remove_file(&db);

    let reqs = server.await.unwrap();
    assert_eq!(reqs.len(), 2, "expected two model calls");
    assert!(reqs[0].contains("first question"), "turn 1 body: {}", reqs[0]);
    assert!(reqs[0].contains("\"model\":\"chat-model\""), "turn 1 model");
    // Turn 2 must accumulate history and reflect /model switch.
    assert!(reqs[1].contains("second question"), "turn 2 body: {}", reqs[1]);
    assert!(reqs[1].contains("first question"), "turn 2 lost history: {}", reqs[1]);
    assert!(reqs[1].contains("reply1"), "turn 2 missing assistant message: {}", reqs[1]);
    assert!(reqs[1].contains("\"model\":\"other-model\""), "turn 2 model: {}", reqs[1]);

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reply1"), "missing streamed reply1: {stdout}");
    assert!(stdout.contains("reply2"), "missing streamed reply2: {stdout}");
    assert!(stdout.contains("(model: other-model)"), "missing /model ack: {stdout}");
}
