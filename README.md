# local-code

Zero-config, offline-first AI coding assistant CLI for **local models** (Ollama, llama.cpp). No API keys, no subscriptions, no cloud. Just:

```
local-code "fix the auth bug"
```

## Why

Cloud coding assistants are great but require subscriptions and send your code to remote servers. With capable local models (Qwen2.5-Coder, DeepSeek-Coder, CodeLlama) and NPUs becoming standard, a truly local coding CLI is now practical — private, free, and works offline.

## Install

From source (Rust stable):

```
cargo install --git https://github.com/fuleinist/local-code
```

Or build locally:

```
git clone https://github.com/fuleinist/local-code
cd local-code
cargo build --release
# binary: target/release/local-code
```

You need a local model server:

- **Ollama** (recommended): <https://ollama.com/download>, then `ollama pull qwen2.5-coder`
- **llama.cpp**: `llama-server -m model.gguf --port 8080` (OpenAI-compatible API)

## Quickstart

```
# Ask a question about your codebase (streams the answer)
local-code "explain what this project does"

# Auto-gathers context: repo file map + any files mentioned in your prompt
local-code "why does src/auth.rs return 401?"

# Force-include files
local-code "review this" --file src/main.rs --file src/lib.rs

# Pick a model
local-code --model qwen2.5-coder:7b "write a fizzbuzz in rust"

# Apply edits to disk (model returns SEARCH/REPLACE blocks)
local-code --apply "add input validation to the login form"
local-code --apply --dry-run "..."   # preview diffs without writing

# Multi-turn chat REPL
local-code chat
> /add src/db.rs        # include a file every turn
> /model deepseek-coder-v2:16b
> /clear                # reset conversation
> /quit

# Session history (SQLite at ~/.local-code/history.db)
local-code history
local-code history --last
```

## Backends & configuration

| Setting | Flag | Env var | Default |
|---|---|---|---|
| Backend | `--backend auto\|ollama\|llama` | — | `auto` (probes Ollama, then llama.cpp) |
| Server URL | `--server-url <url>` | `OLLAMA_HOST` | `http://127.0.0.1:11434` (Ollama), `http://127.0.0.1:8080/v1` (llama) |
| Model | `--model <name>` | `LOCAL_CODE_MODEL` | auto-pick first code model from Ollama tags |
| Context budget | `--context-tokens <chars>` | — | `8000` |
| History DB | — | `LOCAL_CODE_DB` | `~/.local-code/history.db` |

Model auto-detection prefers (in order): `qwen2.5-coder`, `qwen2-coder`, `deepseek-coder`, `codellama`, `codegemma`, `starcoder`, `llama3.x`, `mistral`, `phi3`, then whatever is installed first.

## How context gathering works

- Walks the repo respecting `.gitignore` (depth-capped, max 500 files) and sends a compact file map.
- Files **mentioned in your prompt** (by name or path) are included automatically.
- `--file` forces inclusion (works even for gitignored files).
- Skips binary files, non-UTF-8 files, and files > 100KB; truncates to the context budget.

## Apply mode format

The model is instructed to emit edits as:

```
FILE: src/main.rs
<<<<<<< SEARCH
fn main() {}
=======
fn main() { println!("hi"); }
>>>>>>> REPLACE
```

`local-code` verifies each SEARCH block matches the file exactly once (rejecting ambiguous or missing matches), prints a diff, and applies it. Empty SEARCH creates a new file. Malformed blocks are reported and skipped. Exits non-zero if any edit failed.

## Development

```
cargo build
cargo test            # unit + integration tests (mock servers; no model needed)
cargo clippy --all-targets -- -D warnings
```

All tests run without a local model: network paths are covered against closed ports and in-test mock HTTP servers.

## Roadmap

See [SPEC.md](SPEC.md). Ideas welcome — issues open.

## License

MIT
