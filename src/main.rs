mod apply;
mod cli;
mod config;
mod context;
mod history;
mod llama;
mod ollama;

use std::io::Write;

use anyhow::{bail, Result};
use clap::Parser;

use cli::{Backend, Cli};

const SYSTEM_PROMPT: &str = "You are local-code, an expert programming assistant running \
entirely on the user's machine via a local model. Be concise and practical. When showing code, \
use fenced code blocks with the language tag. Prefer minimal, working solutions over lengthy \
explanations.";

const APPLY_SYSTEM_PROMPT: &str = "You are local-code running in --apply mode on the user's \
machine. Respond with code edits using EXACTLY this format, one block per edit:\n\
FILE: <relative/path>\n\
<<<<<<< SEARCH\n\
<exact existing content to replace; leave empty to create a new file>\n\
=======\n\
<new content>\n\
>>>>>>> REPLACE\n\
SEARCH text must match the file byte-for-byte and be unique within the file. Keep prose outside \
the blocks minimal.";

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
        Some(p) if p == "history" => {
            return run_history(&cli);
        }
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

    let system_prompt = if cli.apply { APPLY_SYSTEM_PROMPT } else { SYSTEM_PROMPT };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let response = match resolved.backend {
        Backend::Llama => {
            llama::stream_chat(
                &client,
                &resolved.base_url,
                &resolved.model,
                system_prompt,
                &user_message,
                |tok| {
                    let _ = write!(out, "{tok}");
                    let _ = out.flush();
                },
            )
            .await?
        }
        _ => {
            ollama::stream_chat(
                &client,
                &resolved.base_url,
                &resolved.model,
                system_prompt,
                &user_message,
                |tok| {
                    let _ = write!(out, "{tok}");
                    let _ = out.flush();
                },
            )
            .await?
        }
    };
    writeln!(out)?;

    if cli.apply {
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let outcome = apply::parse_edits(&response);
        for err in &outcome.errors {
            eprintln!("local-code: malformed edit block skipped: {err}");
        }
        if outcome.edits.is_empty() && outcome.errors.is_empty() {
            eprintln!("local-code: model returned no edit blocks; nothing to apply");
        }
        let mut failures = outcome.errors.len();
        for edit in &outcome.edits {
            eprintln!("\n--- {} ---", edit.path);
            eprint!("{}", apply::render_diff(edit));
            match apply::apply_edit(&root, edit, cli.dry_run) {
                Ok(apply::ApplyResult::Applied) => eprintln!("applied: {}", edit.path),
                Ok(apply::ApplyResult::Created) => eprintln!("created: {}", edit.path),
                Ok(apply::ApplyResult::DryRun) => eprintln!("dry-run (not written): {}", edit.path),
                Ok(apply::ApplyResult::Failed(reason)) => {
                    eprintln!("FAILED: {}: {reason}", edit.path);
                    failures += 1;
                }
                Err(e) => {
                    eprintln!("FAILED: {}: {e}", edit.path);
                    failures += 1;
                }
            }
        }
        if failures > 0 {
            std::process::exit(1);
        }
    }

    // F5: persist the exchange (best effort; never fail the run over it).
    match history::db_path().and_then(|p| history::open(&p)) {
        Ok(conn) => {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
            if let Err(e) = history::record(&conn, &cwd, &resolved.model, &prompt, &response) {
                eprintln!("local-code: warning: failed to record session history: {e}");
            }
        }
        Err(e) => eprintln!("local-code: warning: history unavailable: {e}"),
    }
    Ok(())
}

fn run_history(cli: &Cli) -> Result<()> {
    let path = history::db_path()?;
    let conn = history::open(&path)?;
    if cli.last {
        match history::last(&conn)? {
            Some(row) => {
                println!("#{} {} (model: {}, cwd: {})", row.id, history::format_unix_ts(&row.ts), row.model, row.cwd);
                println!("--- prompt ---\n{}", row.prompt);
                println!("--- response ---\n{}", row.response);
            }
            None => println!("no sessions recorded yet"),
        }
        return Ok(());
    }
    let rows = history::recent(&conn, 20)?;
    if rows.is_empty() {
        println!("no sessions recorded yet");
        return Ok(());
    }
    println!("recent sessions (newest first, db: {}):", path.display());
    for row in rows {
        let short: String = row.prompt.chars().take(70).collect();
        println!("  #{}  {}  [{}]  {}", row.id, history::format_unix_ts(&row.ts), row.model, short);
    }
    println!("tip: local-code history --last  (show the most recent exchange in full)");
    Ok(())
}
