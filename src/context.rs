//! F2 — Context gathering: repo map + relevant file contents, respecting
//! .gitignore, skipping binaries and oversized files, under a char budget.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use ignore::WalkBuilder;

const MAX_FILE_BYTES: u64 = 100 * 1024; // 100KB
const MAX_MAP_ENTRIES: usize = 500;
const MAX_MAP_DEPTH: usize = 5;
const SNIFF_BYTES: usize = 8 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Gathered {
    /// Relative paths of all walked files (the repo map), in walk order.
    pub repo_files: Vec<String>,
    /// Files whose contents were included, with contents. Forced includes
    /// first, then prompt-mentioned files, in mention order.
    pub included: Vec<(String, String)>,
    /// Names of requested files that could not be included, with reasons.
    pub skipped: Vec<(String, String)>,
    /// True when the budget truncated some content.
    pub truncated: bool,
}

/// Walk the repo root respecting .gitignore; return relative file paths.
pub fn walk_repo(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in WalkBuilder::new(root)
        .max_depth(Some(MAX_MAP_DEPTH))
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
    {
        let entry = entry.context("walk error")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
        if files.len() >= MAX_MAP_ENTRIES {
            break;
        }
    }
    files.sort();
    Ok(files)
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(SNIFF_BYTES).any(|&b| b == 0)
}

/// Read a file for inclusion as context. Returns Err(reason) if it must be
/// skipped (missing, binary, or oversized).
pub fn read_context_file(root: &Path, rel: &str) -> std::result::Result<String, String> {
    let path: PathBuf = root.join(rel);
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => return Err(format!("not readable ({e})")),
    };
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "too large ({} KB > {} KB limit)",
            meta.len() / 1024,
            MAX_FILE_BYTES / 1024
        ));
    }
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => return Err(format!("read failed ({e})")),
    };
    if is_probably_binary(&bytes) {
        return Err("binary file".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "not valid UTF-8".to_string())
}

/// Find repo files mentioned in the prompt. A mention matches when the prompt
/// contains the file's basename (with extension) or its relative path.
pub fn find_mentioned(repo_files: &[String], prompt: &str) -> Vec<String> {
    let mut mentioned = Vec::new();
    for f in repo_files {
        let basename = f.rsplit('/').next().unwrap_or(f);
        if !basename.contains('.') {
            continue; // skip extensionless names: too many false positives
        }
        if prompt.contains(f.as_str()) || prompt.contains(basename) {
            mentioned.push(f.clone());
        }
    }
    mentioned
}

/// Gather context: forced files + mentioned files under `budget_chars`.
pub fn gather(
    root: &Path,
    prompt: &str,
    forced: &[String],
    budget_chars: usize,
) -> Result<Gathered> {
    let repo_files = walk_repo(root)?;
    let mut gathered = Gathered {
        repo_files,
        ..Default::default()
    };

    // Forced includes (validated against existence on disk, not the walk,
    // so explicitly ignored files can still be forced).
    let mut wanted: Vec<(String, bool)> = Vec::new(); // (path, forced)
    for f in forced {
        let norm = f.replace('\\', "/");
        wanted.push((norm, true));
    }
    for f in find_mentioned(&gathered.repo_files, prompt) {
        if !wanted.iter().any(|(w, _)| *w == f) {
            wanted.push((f, false));
        }
    }

    let mut used = 0usize;
    for (rel, forced) in wanted {
        match read_context_file(root, &rel) {
            Ok(content) => {
                let cost = content.len() + rel.len() + 12; // fences + header
                if used + cost > budget_chars && !gathered.included.is_empty() {
                    // Truncate this file's content to the remaining budget.
                    let remaining = budget_chars.saturating_sub(used);
                    if remaining > 256 {
                        let mut cut = content.chars().take(remaining - 64).collect::<String>();
                        cut.push_str("\n... [truncated to fit context budget] ...");
                        gathered.included.push((rel, cut));
                        used = budget_chars;
                        gathered.truncated = true;
                    } else {
                        gathered.skipped.push((rel, "context budget exhausted".to_string()));
                        gathered.truncated = true;
                    }
                    continue;
                }
                used += cost;
                let _ = forced;
                gathered.included.push((rel, content));
            }
            Err(reason) => gathered.skipped.push((rel, reason)),
        }
    }

    Ok(gathered)
}

/// Render gathered context into the user message for the model.
pub fn render(prompt: &str, gathered: &Gathered) -> String {
    let mut msg = String::new();

    if !gathered.repo_files.is_empty() {
        msg.push_str("Repository file map:\n");
        for f in gathered.repo_files.iter().take(MAX_MAP_ENTRIES) {
            msg.push_str("  ");
            msg.push_str(f);
            msg.push('\n');
        }
        msg.push('\n');
    }

    for (path, content) in &gathered.included {
        msg.push_str(&format!("File `{path}`:\n```\n{content}\n```\n\n"));
    }

    for (path, reason) in &gathered.skipped {
        msg.push_str(&format!("(Skipped `{path}`: {reason})\n"));
    }
    if !gathered.skipped.is_empty() {
        msg.push('\n');
    }

    msg.push_str("User instruction:\n");
    msg.push_str(prompt);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // The `ignore` crate only applies .gitignore inside a git repo.
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        fs::write(root.join("src/auth.rs"), "pub fn login() -> bool { true }\n").unwrap();
        fs::write(root.join("README.md"), "# demo\n").unwrap();
        // ignored file
        fs::write(root.join(".gitignore"), "secret.env\nbuild/\n").unwrap();
        fs::write(root.join("secret.env"), "KEY=hunter2\n").unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/out.js"), "console.log(1)\n").unwrap();
        // binary file (contains NUL)
        fs::write(root.join("data.bin"), b"abc\0def").unwrap();
        // oversized file
        fs::write(root.join("huge.txt"), "x".repeat(101 * 1024)).unwrap();
        (dir, root)
    }

    #[test]
    fn walk_respects_gitignore() {
        let (_d, root) = setup();
        let files = walk_repo(&root).unwrap();
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/auth.rs".to_string()));
        assert!(!files.contains(&"secret.env".to_string()), "ignored file leaked: {files:?}");
        assert!(!files.iter().any(|f| f.starts_with("build/")), "ignored dir leaked: {files:?}");
        assert!(files.contains(&"data.bin".to_string()), "binaries listed but skipped at read time");
    }

    #[test]
    fn mentioned_files_detected() {
        let (_d, root) = setup();
        let files = walk_repo(&root).unwrap();
        let m = find_mentioned(&files, "fix the auth bug in src/auth.rs");
        assert!(m.contains(&"src/auth.rs".to_string()));
        let m2 = find_mentioned(&files, "what does main.rs do?");
        assert!(m2.contains(&"src/main.rs".to_string()));
    }

    #[test]
    fn skips_binary_oversized_missing() {
        let (_d, root) = setup();
        assert_eq!(
            read_context_file(&root, "data.bin").unwrap_err(),
            "binary file"
        );
        assert!(read_context_file(&root, "huge.txt")
            .unwrap_err()
            .contains("too large"));
        assert!(read_context_file(&root, "nope.rs")
            .unwrap_err()
            .contains("not readable"));
        assert!(read_context_file(&root, "src/auth.rs").is_ok());
    }

    #[test]
    fn gather_forced_and_budget() {
        let (_d, root) = setup();
        // Forced include works even for an ignored file.
        let g = gather(&root, "hello", &["secret.env".to_string()], 8000).unwrap();
        assert_eq!(g.included.len(), 1);
        assert_eq!(g.included[0].0, "secret.env");

        // Mentioned file pulled in via prompt.
        let g = gather(&root, "look at auth.rs", &[], 8000).unwrap();
        assert!(g.included.iter().any(|(p, _)| p == "src/auth.rs"));

        // Tiny budget truncates.
        let g = gather(
            &root,
            "review main.rs and auth.rs and README.md",
            &[],
            100,
        )
        .unwrap();
        assert!(g.truncated || g.skipped.iter().any(|(_, r)| r.contains("budget")));
    }

    #[test]
    fn gather_missing_forced_reported() {
        let (_d, root) = setup();
        let g = gather(&root, "hi", &["does/not/exist.rs".to_string()], 8000).unwrap();
        assert_eq!(g.skipped.len(), 1);
        assert!(g.skipped[0].1.contains("not readable"));
    }

    #[test]
    fn render_includes_sections() {
        let (_d, root) = setup();
        let g = gather(&root, "explain auth.rs", &[], 8000).unwrap();
        let rendered = render("explain auth.rs", &g);
        assert!(rendered.contains("Repository file map:"));
        assert!(rendered.contains("User instruction:"));
        assert!(rendered.contains("explain auth.rs"));
        assert!(rendered.contains("src/auth.rs"));
    }
}
