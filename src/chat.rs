//! F4 — Chat mode: interactive multi-turn REPL with in-session history.
//! Commands: /add <file>, /model <name>, /clear, /help, /quit.

use std::io::{self, BufRead, Write};

use anyhow::Result;
use serde_json::json;

use crate::cli::{Backend, Cli};
use crate::config::Resolved;
use crate::{context, history, llama, ollama, SYSTEM_PROMPT};

const HELP: &str = "commands:\n  /add <file>   include a file as context in every turn\n  /model <name> switch model for this session\n  /clear        reset conversation history\n  /help         show this help\n  /quit         exit (Ctrl-D also works)";

pub async fn run(cli: &Cli, client: &reqwest::Client, resolved: &mut Resolved) -> Result<()> {
    println!(
        "local-code chat — model: {} — server: {} — backend: {:?}",
        resolved.model, resolved.base_url, resolved.backend
    );
    println!("{HELP}");

    let mut messages: Vec<serde_json::Value> =
        vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
    let mut extra_files: Vec<String> = cli.files.clone();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let stdin = io::stdin();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            // EOF (Ctrl-D or piped input ended)
            println!();
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" {
            break;
        }
        if input == "/clear" {
            messages.truncate(1);
            println!("(conversation cleared)");
            continue;
        }
        if input == "/help" {
            println!("{HELP}");
            continue;
        }
        if let Some(f) = input.strip_prefix("/add ") {
            let f = f.trim().to_string();
            if f.is_empty() {
                println!("usage: /add <file>");
            } else {
                println!("(added {f} to every-turn context)");
                extra_files.push(f);
            }
            continue;
        }
        if let Some(m) = input.strip_prefix("/model ") {
            let m = m.trim().to_string();
            if m.is_empty() {
                println!("usage: /model <name>");
            } else {
                println!("(model: {m})");
                resolved.model = m;
            }
            continue;
        }

        let user_message = match context::gather(&root, input, &extra_files, cli.context_tokens) {
            Ok(gathered) => context::render(input, &gathered),
            Err(e) => {
                eprintln!("local-code: context gathering failed ({e}); sending prompt only");
                input.to_string()
            }
        };
        messages.push(json!({"role": "user", "content": user_message}));

        let stdout = io::stdout();
        let mut out = stdout.lock();
        let streamed = match resolved.backend {
            Backend::Llama => {
                llama::stream_chat_messages(
                    client,
                    &resolved.base_url,
                    &resolved.model,
                    &messages,
                    |tok| {
                        let _ = write!(out, "{tok}");
                        let _ = out.flush();
                    },
                )
                .await
            }
            _ => {
                ollama::stream_chat_messages(
                    client,
                    &resolved.base_url,
                    &resolved.model,
                    &messages,
                    |tok| {
                        let _ = write!(out, "{tok}");
                        let _ = out.flush();
                    },
                )
                .await
            }
        };
        match streamed {
            Ok(response) => {
                writeln!(out)?;
                drop(out);
                messages.push(json!({"role": "assistant", "content": response.clone()}));
                // Persist each turn (best effort).
                if let Ok(path) = history::db_path() {
                    if let Ok(conn) = history::open(&path) {
                        let cwd = root.to_string_lossy().to_string();
                        if let Err(e) = history::record(&conn, &cwd, &resolved.model, input, &response)
                        {
                            eprintln!("local-code: warning: history write failed: {e}");
                        }
                    }
                }
            }
            Err(e) => {
                drop(out);
                eprintln!("local-code: {e}");
                // Drop the failed user turn so history stays consistent.
                messages.pop();
            }
        }
    }
    Ok(())
}
