//! Integration tests for AC4: unreachable server must exit non-zero with an
//! actionable hint. Runs against a closed port; no model required.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_local-code"))
}

#[test]
fn unreachable_ollama_exits_nonzero_with_hint() {
    let out = bin()
        .arg("hello")
        .arg("--backend")
        .arg("ollama")
        .env("OLLAMA_HOST", "http://127.0.0.1:9")
        .output()
        .expect("failed to run local-code binary");
    assert!(
        !out.status.success(),
        "expected non-zero exit, got success. stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        stderr.contains("ollama"),
        "stderr should mention ollama: {stderr}"
    );
    assert!(
        stderr.contains("serve") || stderr.contains("install") || stderr.contains("download"),
        "stderr should contain an actionable hint: {stderr}"
    );
}

#[test]
fn unreachable_auto_backend_exits_nonzero_with_hint() {
    let out = bin()
        .arg("hello")
        .env("OLLAMA_HOST", "http://127.0.0.1:9")
        .output()
        .expect("failed to run local-code binary");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(stderr.contains("ollama"), "stderr: {stderr}");
}

#[test]
fn help_documents_flags() {
    let out = bin()
        .arg("--help")
        .output()
        .expect("failed to run local-code binary");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--model",
        "--file",
        "--context-tokens",
        "--apply",
        "--dry-run",
        "--backend",
        "--server-url",
        "--chat",
    ] {
        assert!(help.contains(flag), "--help missing {flag}");
    }
}
