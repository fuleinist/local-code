mod cli;
mod config;
mod context;
mod ollama;

use std::io::Write;

use anyhow::{bail, Result};
use clap::Parser;

use cli::{Backend, Cli};

const SYSTEM_PROMPT: &str = "You are local-code, an expert programming assistant running \
entirely on the user's machine via a local model. Be concise and practical. When showing code, \
use fenced code blocks with the language tag. Prefer minimal, working solutions over lengthy \
explanations.";

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("local-code: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    if cli.chat {
        bail!("chat mode lands in a later milestone; for now use: local-code \"<instruction>\"");
    }

    let prompt = match &cli.prompt {
        Some(p) if !p.trim().is_empty() => p.clone(),
        _ => {
            Cli::parse_from(["local-code", "--help"]);
            unreachable!()
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let resolved = config::resolve(&cli, &client).await?;
    if resolved.backend == Backend::Llama {
        bail!("the llama.cpp backend lands in a later milestone; use --backend ollama for now");
    }

    let user_message = {
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        match context::gather(&root, &prompt, &cli.files, cli.context_tokens) {
            Ok(gathered) => context::render(&prompt, &gathered),
            Err(e) => {
                eprintln!("local-code: context gathering failed ({e}); sending prompt only");
                prompt.clone()
            }
        }
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    ollama::stream_chat(&client, &resolved.base_url, &resolved.model, SYSTEM_PROMPT, &user_message, |tok| {
        let _ = write!(out, "{tok}");
        let _ = out.flush();
    })
    .await?;
    writeln!(out)?;
    Ok(())
}
