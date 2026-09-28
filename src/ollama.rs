use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
}

/// Models we prefer when auto-detecting, in priority order.
const PREFERRED: &[&str] = &[
    "qwen2.5-coder",
    "qwen2-coder",
    "deepseek-coder",
    "codellama",
    "codegemma",
    "starcoder",
    "llama3",
    "llama3.1",
    "llama3.2",
    "mistral",
    "phi3",
];

/// Pick the best model from an Ollama tag list, per SPEC F1 auto-detection.
pub fn pick_model(models: &[String]) -> Option<String> {
    for pref in PREFERRED {
        for m in models {
            if m.to_lowercase().contains(pref) {
                return Some(m.clone());
            }
        }
    }
    models.first().cloned()
}

pub fn conn_hint(e: &reqwest::Error, base_url: &str) -> String {
    if e.is_connect() || e.is_request() {
        format!(
            "could not reach the local model server at {base_url}.\n\
             \n\
             local-code needs Ollama running locally:\n  \
             1. Install:  https://ollama.com/download  (or: winget install Ollama.Ollama / brew install ollama)\n  \
             2. Start:    ollama serve\n  \
             3. Pull a model:  ollama pull qwen2.5-coder\n\
             \n\
             If your server runs elsewhere, set OLLAMA_HOST or pass --server-url."
        )
    } else {
        format!("request to {base_url} failed: {e}")
    }
}

/// List model names available on the Ollama server.
pub async fn list_models(client: &reqwest::Client, base_url: &str) -> Result<Vec<String>> {
    let url = format!("{base_url}/api/tags");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(conn_hint(&e, base_url)))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("Ollama returned HTTP {status} from {url}");
    }
    let tags: TagsResponse = resp
        .json()
        .await
        .context("failed to parse Ollama /api/tags response")?;
    Ok(tags.models.into_iter().map(|m| m.name).collect())
}

/// Send a chat request to Ollama and stream tokens to `on_token`.
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

/// Send a multi-turn chat request to Ollama and stream tokens to `on_token`.
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
    let url = format!("{base_url}/api/chat");
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
        bail!("Ollama returned HTTP {status}: {text}");
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| anyhow::anyhow!(conn_hint(&e, base_url)))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        // Ollama streams NDJSON: one JSON object per line.
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parsed: ChatChunk = serde_json::from_str(line)
                .with_context(|| format!("failed to parse Ollama stream line: {line}"))?;
            if let Some(err) = parsed.error {
                bail!("Ollama error: {err}");
            }
            if let Some(msg) = parsed.message {
                if !msg.content.is_empty() {
                    on_token(&msg.content);
                    full.push_str(&msg.content);
                }
            }
            if parsed.done {
                return Ok(full);
            }
        }
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_code_models() {
        let models: Vec<String> = ["llama3:latest", "qwen2.5-coder:7b", "mistral:latest"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_model(&models).as_deref(), Some("qwen2.5-coder:7b"));
    }

    #[test]
    fn pick_prefers_deepseek_over_llama() {
        let models: Vec<String> = ["llama3:latest", "deepseek-coder-v2:16b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            pick_model(&models).as_deref(),
            Some("deepseek-coder-v2:16b")
        );
    }

    #[test]
    fn pick_falls_back_to_first() {
        let models: Vec<String> = ["mystery-model:latest", "other:1.0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_model(&models).as_deref(), Some("mystery-model:latest"));
    }

    #[test]
    fn pick_empty_list() {
        assert_eq!(pick_model(&[]), None);
    }

    #[tokio::test]
    async fn unreachable_server_gives_hint() {
        let client = reqwest::Client::new();
        let err = list_models(&client, "http://127.0.0.1:9").await.unwrap_err();
        let msg = format!("{err:#}").to_lowercase();
        assert!(msg.contains("ollama"), "hint missing: {msg}");
        assert!(msg.contains("serve") || msg.contains("install"), "hint missing: {msg}");
    }
}
