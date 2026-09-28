# local-code — SPEC

Zero-config, offline-first AI coding assistant CLI that works with local models (Ollama, llama.cpp). No API keys, no subscriptions.

```
local-code "fix the auth bug"
```

## Goals

1. **Zero config**: works out of the box if Ollama is running on `localhost:11434`. Auto-detects available models.
2. **Offline-first**: no cloud calls ever. All inference local.
3. **Terminal-native**: fast, single binary, sensible defaults, scriptable.
4. **Codebase-aware**: gathers relevant file context from the current repo before prompting.

## Non-goals (v0)

- No GUI / IDE extension
- No cloud provider fallback
- No multi-user / team features
- No fine-tuning or model management (defer to `ollama` CLI)

## Feature List

### F1 — Prompt mode (MVP)
- `local-code "<instruction>"` sends the instruction + gathered context to the local model and streams the response to stdout.
- Model selection: `--model <name>` flag, `LOCAL_CODE_MODEL` env var, else auto-pick first available code-ish model from Ollama (`qwen2.5-coder`, `deepseek-coder`, `codellama`, `llama3`, fallback: first model listed).
- Stream tokens as they arrive (SSE from Ollama `/api/chat`).
- Exit non-zero with a clear error if Ollama is unreachable; print how to install/start it.

### F2 — Context gathering
- Default: include a compact repo map (file tree, respecting `.gitignore`, capped at N files/depth) + contents of files explicitly mentioned in the prompt (match by filename/substring).
- `--file <path>` (repeatable) to force-include files.
- `--context-tokens <n>` budget (default ~8000 chars of file content); truncate oldest/least relevant first.
- Never include binary files, files > 100KB, or anything matched by `.gitignore`.

### F3 — Apply mode
- `local-code --apply "<instruction>"`: asks the model for edits in a strict format (fenced blocks with `path` + `SEARCH`/`REPLACE` sections), parses them, shows a diff, and applies to disk.
- `--dry-run` prints diffs without writing.
- Refuse to apply malformed blocks; report which failed.

### F4 — Chat mode
- `local-code chat` (or `--chat`): REPL with multi-turn history kept in memory for the session; `/add <file>`, `/model <name>`, `/clear`, `/quit` commands.

### F5 — Session history
- Persist sessions (prompt, model, response, cwd, timestamp) to SQLite at `~/.local-code/history.db`.
- `local-code history` lists recent sessions; `local-code history --last` shows the most recent exchange.

### F6 — llama.cpp server support
- `--backend llama` targets an OpenAI-compatible llama.cpp server (`localhost:8080/v1/chat/completions` by default, override with `--server-url`).
- Backend auto-detection: probe Ollama first, then llama.cpp server.

## Tech Stack

- **Rust** (stable), single binary
- `reqwest` (HTTP + SSE streaming), `clap` (CLI), `serde`/`serde_json`, `rusqlite` (bundled SQLite), `ignore` (gitignore-respecting walk), `anyhow`/`thiserror`, `colored` or `owo-colors`, `tokio`
- No heavy frameworks

## Acceptance Criteria

- [ ] AC1: `cargo build --release` succeeds with no warnings-as-errors; binary runs on Windows and Linux.
- [ ] AC2: `local-code --help` documents all flags above.
- [ ] AC3: With Ollama running, `local-code "hello"` streams a response; exit code 0.
- [ ] AC4: With Ollama NOT running, `local-code "hello"` exits non-zero and prints an actionable install/start hint. (Testable without a model via unit/integration test against a closed port.)
- [ ] AC5: Context gathering respects `.gitignore`, skips binaries/>100KB files, includes explicitly mentioned files. Unit tests cover selection + truncation.
- [ ] AC6: `--apply` parses SEARCH/REPLACE blocks, applies exact matches, rejects ambiguous/missing matches. Unit tests with tempdirs cover apply, dry-run, and malformed input.
- [ ] AC7: Session history writes to SQLite and `history --last` round-trips. Unit test with temp DB.
- [ ] AC8: `--backend llama` sends OpenAI-compatible requests; covered by a mock-server integration test.
- [ ] AC9: `cargo test` passes; `cargo clippy -- -D warnings` clean.
- [ ] AC10: README.md with install (cargo install / binary releases), quickstart, examples, and configuration.

## Build Plan (cycles)

1. Cargo project scaffold + CLI parsing (clap) + config/model resolution + Ollama client with streaming (F1) → AC1–AC4
2. Context gathering (F2) + tests → AC5
3. Apply mode (F3) + tests → AC6
4. Session history (F5) + tests → AC7
5. llama.cpp backend (F6) + mock test → AC8
6. Chat REPL (F4)
7. Polish: clippy, README, error messages → AC9, AC10
