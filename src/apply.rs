//! F3 — Apply mode: parse SEARCH/REPLACE edit blocks from model output,
//! show diffs, and apply them to disk (or dry-run).
//!
//! Wire format (documented to the model in the apply system prompt):
//!
//! ```text
//! FILE: <relative/path>
//! <<<<<<< SEARCH
//! <exact existing content; empty when creating a new file>
//! =======
//! <new content>
//! >>>>>>> REPLACE
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub path: String,
    pub search: String,
    pub replace: String,
}

#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub edits: Vec<Edit>,
    /// Human-readable problems for malformed blocks.
    pub errors: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyResult {
    Applied,
    DryRun,
    Created,
    Failed(String),
}

/// Parse edit blocks from arbitrary model output. Text outside blocks is
/// ignored. Malformed blocks are reported in `errors` (never silently dropped
/// once a `FILE:` header was seen).
pub fn parse_edits(text: &str) -> ParseOutcome {
    let mut out = ParseOutcome::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(path) = line
            .strip_prefix("FILE:")
            .map(|p| p.trim().trim_matches('`').to_string())
        {
            if path.is_empty() {
                out.errors.push(format!("line {}: FILE: header with empty path", i + 1));
                i += 1;
                continue;
            }
            // Expect <<<<<<< SEARCH on a following line (allowing a code fence).
            let mut j = i + 1;
            while j < lines.len() {
                let l = lines[j].trim();
                if l.starts_with("```") || l.is_empty() {
                    j += 1;
                    continue;
                }
                break;
            }
            if j >= lines.len() || !lines[j].trim().starts_with("<<<<<<<") {
                out.errors.push(format!(
                    "line {}: FILE: {path} not followed by a <<<<<<< SEARCH block",
                    i + 1
                ));
                i = j.max(i + 1);
                continue;
            }
            // Collect search section until =======
            let mut search = Vec::new();
            j += 1;
            let mut found_mid = false;
            while j < lines.len() {
                if lines[j].trim() == "=======" {
                    found_mid = true;
                    j += 1;
                    break;
                }
                if lines[j].trim().starts_with(">>>>>>>") {
                    break;
                }
                search.push(lines[j]);
                j += 1;
            }
            if !found_mid {
                out.errors.push(format!(
                    "line {}: block for {path} missing ======= separator",
                    i + 1
                ));
                i = j.max(i + 1);
                continue;
            }
            // Collect replace section until >>>>>>> REPLACE
            let mut replace = Vec::new();
            let mut found_end = false;
            while j < lines.len() {
                if lines[j].trim().starts_with(">>>>>>>") {
                    found_end = true;
                    j += 1;
                    break;
                }
                replace.push(lines[j]);
                j += 1;
            }
            if !found_end {
                out.errors.push(format!(
                    "line {}: block for {path} missing >>>>>>> REPLACE terminator",
                    i + 1
                ));
                i = j.max(i + 1);
                continue;
            }
            out.edits.push(Edit {
                path,
                search: search.join("\n"),
                replace: replace.join("\n"),
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Naive line diff for display: removals from `search`, additions from `replace`.
pub fn render_diff(edit: &Edit) -> String {
    let mut s = String::new();
    for l in edit.search.lines() {
        s.push('-');
        s.push_str(l);
        s.push('\n');
    }
    for l in edit.replace.lines() {
        s.push('+');
        s.push_str(l);
        s.push('\n');
    }
    if s.is_empty() {
        s.push_str("(no change)\n");
    }
    s
}

/// Apply a single edit under `root`.
pub fn apply_edit(root: &Path, edit: &Edit, dry_run: bool) -> Result<ApplyResult> {
    let path: PathBuf = root.join(&edit.path);
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Ok(ApplyResult::Failed(
            "path escapes the repository (.. not allowed)".to_string(),
        ));
    }

    // New-file creation: empty search block.
    if edit.search.is_empty() && !path.exists() {
        if dry_run {
            return Ok(ApplyResult::DryRun);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, edit.replace.as_bytes())?;
        return Ok(ApplyResult::Created);
    }

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(ApplyResult::Failed(format!(
                "cannot read {}: {e}",
                edit.path
            )))
        }
    };

    let matches = content.match_indices(&edit.search).count();
    if matches == 0 {
        // Retry ignoring line-ending differences (models often emit \n).
        let norm_search = edit.search.replace('\r', "");
        let norm_content = content.replace('\r', "");
        let norm_matches = norm_content.match_indices(&norm_search).count();
        if norm_matches == 0 {
            return Ok(ApplyResult::Failed(
                "SEARCH text not found in file".to_string(),
            ));
        }
        if norm_matches > 1 {
            return Ok(ApplyResult::Failed(format!(
                "SEARCH text is ambiguous ({norm_matches} matches)"
            )));
        }
        let idx = norm_content.find(&norm_search).unwrap();
        // Map normalized offset back: only safe when counts are equal-length;
        // fall back to replacing in normalized content and writing it out.
        let _ = idx;
        let new_content = norm_content.replacen(&norm_search, &edit.replace.replace('\r', ""), 1);
        if dry_run {
            return Ok(ApplyResult::DryRun);
        }
        fs::write(&path, new_content)?;
        return Ok(ApplyResult::Applied);
    }
    if matches > 1 {
        return Ok(ApplyResult::Failed(format!(
            "SEARCH text is ambiguous ({matches} matches)"
        )));
    }

    let new_content = content.replacen(&edit.search, &edit.replace, 1);
    if dry_run {
        return Ok(ApplyResult::DryRun);
    }
    fs::write(&path, new_content)?;
    Ok(ApplyResult::Applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const GOOD: &str = "Here are the edits:\n\n```\nFILE: src/main.rs\n<<<<<<< SEARCH\nfn main() {}\n=======\nfn main() { println!(\"hi\"); }\n>>>>>>> REPLACE\n```\n\nDone.";

    #[test]
    fn parse_single_edit() {
        let out = parse_edits(GOOD);
        assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
        assert_eq!(out.edits.len(), 1);
        assert_eq!(out.edits[0].path, "src/main.rs");
        assert_eq!(out.edits[0].search, "fn main() {}");
        assert_eq!(out.edits[0].replace, "fn main() { println!(\"hi\"); }");
    }

    #[test]
    fn parse_multiple_edits() {
        let text = "FILE: a.txt\n<<<<<<< SEARCH\nold a\n=======\nnew a\n>>>>>>> REPLACE\nsome prose\nFILE: b.txt\n<<<<<<< SEARCH\nold b\n=======\nnew b\n>>>>>>> REPLACE\n";
        let out = parse_edits(text);
        assert!(out.errors.is_empty());
        assert_eq!(out.edits.len(), 2);
        assert_eq!(out.edits[1].path, "b.txt");
    }

    #[test]
    fn parse_malformed_missing_separator() {
        let text = "FILE: a.txt\n<<<<<<< SEARCH\nold\n>>>>>>> REPLACE\n";
        let out = parse_edits(text);
        assert_eq!(out.edits.len(), 0);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].contains("======="));
    }

    #[test]
    fn parse_malformed_missing_terminator() {
        let text = "FILE: a.txt\n<<<<<<< SEARCH\nold\n=======\nnew\n";
        let out = parse_edits(text);
        assert_eq!(out.edits.len(), 0);
        assert_eq!(out.errors.len(), 1);
        assert!(out.errors[0].contains(">>>>>>>"));
    }

    #[test]
    fn parse_header_without_block() {
        let text = "FILE: a.txt\nno block here\n";
        let out = parse_edits(text);
        assert_eq!(out.edits.len(), 0);
        assert_eq!(out.errors.len(), 1);
    }

    #[test]
    fn apply_exact_match() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let out = parse_edits(GOOD);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert_eq!(r, ApplyResult::Applied);
        let content = fs::read_to_string(root.join("src/main.rs")).unwrap();
        assert!(content.contains("println!"));
    }

    #[test]
    fn apply_dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let out = parse_edits(GOOD);
        let r = apply_edit(root, &out.edits[0], true).unwrap();
        assert_eq!(r, ApplyResult::DryRun);
        let content = fs::read_to_string(root.join("src/main.rs")).unwrap();
        assert_eq!(content, "fn main() {}\n");
    }

    #[test]
    fn apply_no_match_fails() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "completely different\n").unwrap();
        let out = parse_edits(GOOD);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert!(matches!(r, ApplyResult::Failed(m) if m.contains("not found")));
    }

    #[test]
    fn apply_ambiguous_fails() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "TODO\nTODO\n").unwrap();
        let text = "FILE: src/main.rs\n<<<<<<< SEARCH\nTODO\n=======\nDONE\n>>>>>>> REPLACE\n";
        let out = parse_edits(text);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert!(matches!(r, ApplyResult::Failed(m) if m.contains("ambiguous")));
        // File untouched.
        assert_eq!(fs::read_to_string(root.join("src/main.rs")).unwrap(), "TODO\nTODO\n");
    }

    #[test]
    fn apply_creates_new_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let text = "FILE: new/mod.rs\n<<<<<<< SEARCH\n=======\npub fn new() {}\n>>>>>>> REPLACE\n";
        let out = parse_edits(text);
        assert_eq!(out.edits.len(), 1);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert_eq!(r, ApplyResult::Created);
        assert_eq!(
            fs::read_to_string(root.join("new/mod.rs")).unwrap(),
            "pub fn new() {}"
        );
    }

    #[test]
    fn apply_rejects_path_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.txt"), "x").unwrap();
        let text = "FILE: ../evil.txt\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n";
        let out = parse_edits(text);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert!(matches!(r, ApplyResult::Failed(_)));
    }

    #[test]
    fn apply_missing_file_fails() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let out = parse_edits(GOOD);
        let r = apply_edit(root, &out.edits[0], false).unwrap();
        assert!(matches!(r, ApplyResult::Failed(m) if m.contains("cannot read")));
    }

    #[test]
    fn diff_rendering() {
        let out = parse_edits(GOOD);
        let d = render_diff(&out.edits[0]);
        assert!(d.contains("-fn main() {}"));
        assert!(d.contains("+fn main() { println!"));
    }
}
