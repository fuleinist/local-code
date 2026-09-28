//! F6 — llama.cpp server support: OpenAI-compatible /chat/completions with
//! SSE streaming.

use anyhow::{bail, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Option<Delta>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(default)]
    content: String,
}

fn conn_hint(e: &reqwest::Error, base_url: &str) -> String {
    if e.is_connect() || e.is_request() {
        format!(
            "could not reach the llama.cpp server at {base_url}.\n\
             \n\
             Start one with, e.g.:\n  \
             llama-server -m model.gguf --port 8080\n\
             \n\
             Or point local-code at it with --server-url (expects an OpenAI-compatible /v1 API)."
        )
    } else {
        format!("request to {base_url} failed: {e}")
    }
}

/// Send an OpenAI-compatible chat completion request and stream tokens.
pub async fn stream_chat<F>(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    system: &str,
    user: &str,
    on_token: F,
) -> Result<String>
where
    F: FnMut(&str),
{
    let messages = vec![
        serde_json::json!({"role": "system", "content": system}),
        serde_json::json!({"role": "user", "content": user}),
    ];
    stream_chat_messages(client, base_url, model, &messages, on_token).await
}

/// Send a multi-turn OpenAI-compatible chat completion request and stream tokens.
pub async fn stream_chat_messages<F>(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    messages: &[serde_json::Value],
    mut on_token: F,
) -> Result<String>
where
    F: FnMut(&str),
{
    let url = format!("{base_url}/chat/completions");
    let body = json!({
        "model": model,
        "stream": true,
        "messages": messages,
    });
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(conn_hint(&e, base_url)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("llama.cpp server returned HTTP {status}: {text}");
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| anyhow::anyhow!(conn_hint(&e, base_url)))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let data = match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => continue, // ignore comments/keepalives
            };
            if data == "[DONE]" {
                return Ok(full);
            }
            let parsed: ChatCompletionChunk = serde_json::from_str(data)
                .with_context(data)?;
            if let Some(choice) = parsed.choices.first() {
                if let Some(delta) = &choice.delta {
                    if !delta.content.is_empty() {
                        on_token(&delta.content);
                        full.push_str(&delta.content);
                    }
                }
            }
        }
    }
    Ok(full)
}

trait WithContext<T> {
    fn with_context(self, data: &str) -> Result<T>;
}

impl<T> WithContext<T> for std::result::Result<T, serde_json::Error> {
    fn with_context(self, data: &str) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("failed to parse SSE chunk {data:?}: {e}"))
    }
}
