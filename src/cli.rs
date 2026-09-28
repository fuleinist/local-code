use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    /// Auto-detect: probe Ollama first, then a llama.cpp server.
    Auto,
    /// Ollama (http://localhost:11434 by default).
    Ollama,
    /// llama.cpp server with OpenAI-compatible API (http://localhost:8080/v1 by default).
    Llama,
}

#[derive(Parser, Debug)]
#[command(
    name = "local-code",
    version,
    about = "Zero-config, offline-first AI coding assistant CLI for local models (Ollama, llama.cpp). No API keys, no subscriptions."
)]
pub struct Cli {
    /// Instruction for the model, e.g. "fix the auth bug".
    pub prompt: Option<String>,

    /// Model name (overrides LOCAL_CODE_MODEL and auto-detection).
    #[arg(short, long)]
    pub model: Option<String>,

    /// Force-include a file as context (repeatable).
    #[arg(short = 'f', long = "file")]
    pub files: Vec<String>,

    /// Approximate character budget for gathered file context.
    #[arg(long, default_value_t = 8000)]
    pub context_tokens: usize,

    /// Ask the model for SEARCH/REPLACE edits and apply them to disk.
    #[arg(long)]
    pub apply: bool,

    /// With --apply: print diffs without writing files.
    #[arg(long)]
    pub dry_run: bool,

    /// Inference backend.
    #[arg(long, value_enum, default_value_t = Backend::Auto)]
    pub backend: Backend,

    /// Override the backend server URL (also honors OLLAMA_HOST for Ollama).
    #[arg(long)]
    pub server_url: Option<String>,

    /// Start an interactive multi-turn chat REPL.
    #[arg(long)]
    pub chat: bool,
}
