use anyhow::{bail, Result};

use crate::cli::{Backend, Cli};
use crate::ollama;

/// Resolved runtime configuration for a single invocation.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Backend to use (Auto resolved to a concrete backend).
    pub backend: Backend,
    /// Base URL of the inference server (no trailing slash).
    pub base_url: String,
    /// Model name to use.
    pub model: String,
}

fn normalize_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/').to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        format!("http://{url}")
    }
}

/// Resolve the effective server URL for a backend.
/// Precedence: --server-url > OLLAMA_HOST (ollama backend) > defaults.
pub fn resolve_base_url(cli: &Cli, backend: Backend) -> String {
    if let Some(u) = &cli.server_url {
        return normalize_url(u);
    }
    match backend {
        Backend::Llama => "http://127.0.0.1:8080/v1".to_string(),
        _ => {
            if let Ok(host) = std::env::var("OLLAMA_HOST") {
                if !host.trim().is_empty() {
                    return normalize_url(&host);
                }
            }
            "http://127.0.0.1:11434".to_string()
        }
    }
}

/// Resolve backend, server URL, and model. Contacts the server for
/// auto-detection when needed (backend=auto, or no model configured).
pub async fn resolve(cli: &Cli, client: &reqwest::Client) -> Result<Resolved> {
    let backend = match cli.backend {
        Backend::Auto => {
            // Probe Ollama first, then a llama.cpp server.
            let ollama_url = resolve_base_url(
                &Cli {
                    server_url: cli.server_url.clone(),
                    ..Cli::default_probe()
                },
                Backend::Ollama,
            );
            if cli.server_url.is_some() {
                // Explicit URL: treat auto as Ollama unless it ends with /v1.
                if ollama_url.ends_with("/v1") {
                    Backend::Llama
                } else {
                    Backend::Ollama
                }
            } else if ollama::list_models(client, &ollama_url).await.is_ok() {
                Backend::Ollama
            } else {
                let llama_url = resolve_base_url(cli, Backend::Llama);
                let probe = client
                    .get(format!("{llama_url}/models"))
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                if probe {
                    Backend::Llama
                } else {
                    // Nothing reachable; report against the primary (Ollama) URL.
                    let err = ollama::list_models(client, &ollama_url)
                        .await
                        .unwrap_err();
                    bail!("{err}");
                }
            }
        }
        b => b,
    };

    let base_url = resolve_base_url(cli, backend);

    let model = match &cli.model {
        Some(m) => m.clone(),
        None => match std::env::var("LOCAL_CODE_MODEL") {
            Ok(m) if !m.trim().is_empty() => m,
            _ => {
                if backend == Backend::Llama {
                    llama_default_model(client, &base_url).await?
                } else {
                    let models = ollama::list_models(client, &base_url).await?;
                    ollama::pick_model(&models).ok_or_else(|| {
                        anyhow::anyhow!(
                            "no models found on the Ollama server.\n\
                             Pull one with:  ollama pull qwen2.5-coder"
                        )
                    })?
                }
            }
        },
    };

    Ok(Resolved {
        backend,
        base_url,
        model,
    })
}

async fn llama_default_model(client: &reqwest::Client, base_url: &str) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Models {
        data: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
    }
    let resp = client
        .get(format!("{base_url}/models"))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{}", ollama::conn_hint(&e, base_url)))?;
    let models: Models = resp
        .json()
        .await
        .context("failed to parse llama.cpp /models response")?;
    models
        .data
        .into_iter()
        .next()
        .map(|e| e.id)
        .ok_or_else(|| anyhow::anyhow!("llama.cpp server reported no models"))
}

use anyhow::Context as _;

impl Cli {
    /// Minimal Cli used for probing (no user flags relevant to URL resolution).
    pub fn default_probe() -> Cli {
        Cli {
            prompt: None,
            model: None,
            files: vec![],
            context_tokens: 8000,
            apply: false,
            dry_run: false,
            backend: Backend::Auto,
            server_url: None,
            chat: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_with(server_url: Option<&str>) -> Cli {
        let mut cli = Cli::default_probe();
        cli.server_url = server_url.map(|s| s.to_string());
        cli
    }

    // Env-var tests are merged into one test to avoid races between
    // parallel test threads mutating process-global state.
    #[test]
    fn url_resolution() {
        // --server-url wins and is normalized.
        let url = resolve_base_url(&cli_with(Some("http://box:1234/")), Backend::Ollama);
        assert_eq!(url, "http://box:1234");
        let url = resolve_base_url(&cli_with(Some("192.168.1.5:11434")), Backend::Ollama);
        assert_eq!(url, "http://192.168.1.5:11434");

        // llama default.
        let url = resolve_base_url(&cli_with(None), Backend::Llama);
        assert_eq!(url, "http://127.0.0.1:8080/v1");

        // OLLAMA_HOST env used for ollama backend.
        std::env::set_var("OLLAMA_HOST", "http://10.0.0.2:11434");
        let url = resolve_base_url(&cli_with(None), Backend::Ollama);
        assert_eq!(url, "http://10.0.0.2:11434");

        // Without env: default.
        std::env::remove_var("OLLAMA_HOST");
        let url = resolve_base_url(&cli_with(None), Backend::Ollama);
        assert_eq!(url, "http://127.0.0.1:11434");
    }
}
