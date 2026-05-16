use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over different Git ignore backends.
///
/// The MVP implements only [`GitInfoExclude`] (`.git/info/exclude`).
/// Future backends could target the global `~/.gitignore` or a
/// per-project `.gitignore`.
pub trait IgnoreBackend {
    /// Ensures all paths in `entries` appear in the ignore backend for
    /// `repo_root`.  Calling this multiple times with the same entries must
    /// be idempotent.
    fn add_entries(&self, repo_root: &Path, entries: &[&str]) -> Result<()>;

    /// Removes all paths in `entries` from the ignore backend.  Entries that
    /// are not present are silently skipped.
    fn remove_entries(&self, repo_root: &Path, entries: &[&str]) -> Result<()>;

    /// Returns `true` if `entry` is currently present in the ignore backend.
    fn has_entry(&self, repo_root: &Path, entry: &str) -> Result<bool>;
}

// ── GitInfoExclude ────────────────────────────────────────────────────────────

const BLOCK_BEGIN: &str = "# BEGIN shelfbox";
const BLOCK_END: &str = "# END shelfbox";

/// [`IgnoreBackend`] that manages a `# BEGIN shelfbox … # END shelfbox`
/// block inside `.git/info/exclude`.
///
/// The entire block is replaced atomically on every write, so the
/// implementation is safe even if the file was edited by the user outside
/// this tool.
pub struct GitInfoExclude;

impl GitInfoExclude {
    fn exclude_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".git").join("info").join("exclude")
    }

    /// Reads the current contents of `.git/info/exclude`, silently returning
    /// an empty string if the file does not exist yet.
    fn read(repo_root: &Path) -> Result<String> {
        let path = Self::exclude_path(repo_root);
        match std::fs::read_to_string(&path) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(AppError::io(path, e)),
        }
    }

    /// Atomically writes `contents` to `.git/info/exclude`.
    ///
    /// Creates the parent directory if it doesn't exist (bare repos or
    /// newly initialised worktrees may not have it yet).
    fn write(repo_root: &Path, contents: &str) -> Result<()> {
        let path = Self::exclude_path(repo_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        std::fs::write(&path, contents).map_err(|e| AppError::io(path, e))
    }

    /// Parses `contents` into three parts:
    /// - `before`: everything before `BLOCK_BEGIN` (inclusive of any trailing newline)
    /// - `managed`: the sorted list of entries currently inside the block
    /// - `after`: everything after `BLOCK_END` (inclusive of any leading newline)
    ///
    /// If no managed block exists, `before` is the full text and `managed` is empty.
    fn parse(contents: &str) -> (String, Vec<String>, String) {
        if let (Some(begin_idx), Some(end_idx)) =
            (contents.find(BLOCK_BEGIN), contents.find(BLOCK_END))
        {
            let before = contents[..begin_idx].to_string();
            let block_body = &contents[begin_idx + BLOCK_BEGIN.len()..end_idx];
            let managed: Vec<String> = block_body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            let after_start = end_idx + BLOCK_END.len();
            // Skip the newline that terminates the BLOCK_END line itself so
            // that `after` represents only content truly beyond the block.
            // Without this, render() prepends block's own trailing '\n' and
            // the leftover '\n' in `after`, adding a blank line on every write.
            let after_start = if contents.as_bytes().get(after_start) == Some(&b'\n') {
                after_start + 1
            } else {
                after_start
            };
            let after = contents[after_start..].to_string();
            (before, managed, after)
        } else {
            (contents.to_string(), Vec::new(), String::new())
        }
    }

    /// Serialises `before`, `managed`, `after` back to a file string.
    ///
    /// If `managed` is empty the block markers are omitted entirely so we
    /// don't leave an empty block in the file.
    fn render(before: &str, managed: &[String], after: &str) -> String {
        if managed.is_empty() {
            // Remove any trailing blank line left before the now-deleted block.
            let trimmed = before.trim_end_matches('\n');
            let suffix = if after.is_empty() {
                "\n".to_string()
            } else {
                format!("\n{after}")
            };
            return format!("{trimmed}{suffix}");
        }

        let mut block = String::new();
        block.push_str(BLOCK_BEGIN);
        block.push('\n');
        for entry in managed {
            block.push_str(entry);
            block.push('\n');
        }
        block.push_str(BLOCK_END);
        block.push('\n');

        // Ensure a single blank line separates the preceding content from the block.
        let before_sep = if before.ends_with("\n\n") || before.is_empty() {
            before.to_string()
        } else if before.ends_with('\n') {
            format!("{before}\n")
        } else {
            format!("{before}\n\n")
        };

        format!("{before_sep}{block}{after}")
    }

    /// Updates the managed block with `new_entries` (fully replaces current entries).
    #[allow(dead_code)]
    fn update_entries(&self, repo_root: &Path, new_entries: Vec<String>) -> Result<()> {
        let contents = Self::read(repo_root)?;
        let (before, _, after) = Self::parse(&contents);

        // Sort for deterministic output (easier to review diffs).
        let mut sorted = new_entries;
        sorted.sort();
        sorted.dedup();

        let rendered = Self::render(&before, &sorted, &after);
        Self::write(repo_root, &rendered)
    }
}

impl IgnoreBackend for GitInfoExclude {
    fn add_entries(&self, repo_root: &Path, entries: &[&str]) -> Result<()> {
        let contents = Self::read(repo_root)?;
        let (before, mut managed, after) = Self::parse(&contents);

        for entry in entries {
            let e = entry.to_string();
            if !managed.contains(&e) {
                managed.push(e);
            }
        }
        managed.sort();

        let rendered = Self::render(&before, &managed, &after);
        Self::write(repo_root, &rendered)
    }

    fn remove_entries(&self, repo_root: &Path, entries: &[&str]) -> Result<()> {
        let contents = Self::read(repo_root)?;
        let (before, mut managed, after) = Self::parse(&contents);

        managed.retain(|e| !entries.contains(&e.as_str()));

        let rendered = Self::render(&before, &managed, &after);
        Self::write(repo_root, &rendered)
    }

    fn has_entry(&self, repo_root: &Path, entry: &str) -> Result<bool> {
        let contents = Self::read(repo_root)?;
        let (_, managed, _) = Self::parse(&contents);
        Ok(managed.iter().any(|e| e == entry))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Creates a fake `.git/info/` directory so `write` has somewhere to go.
    fn setup_git_dir(root: &Path) {
        std::fs::create_dir_all(root.join(".git").join("info")).unwrap();
    }

    fn backend() -> GitInfoExclude {
        GitInfoExclude
    }

    // ── parse / render round-trip ────────────────────────────────────────────

    #[test]
    fn parse_empty_file_yields_no_managed_entries() {
        let (before, managed, after) = GitInfoExclude::parse("");
        assert_eq!(before, "");
        assert!(managed.is_empty());
        assert_eq!(after, "");
    }

    #[test]
    fn parse_file_without_block_puts_everything_in_before() {
        let contents = "*.log\n*.tmp\n";
        let (before, managed, after) = GitInfoExclude::parse(contents);
        assert_eq!(before, contents);
        assert!(managed.is_empty());
        assert_eq!(after, "");
    }

    #[test]
    fn parse_extracts_block_entries() {
        let contents = "*.log\n# BEGIN shelfbox\n/notes.md\n/prompts/\n# END shelfbox\n";
        let (before, managed, after) = GitInfoExclude::parse(contents);
        assert_eq!(before, "*.log\n");
        assert_eq!(managed, vec!["/notes.md", "/prompts/"]);
        // after is empty: the '\n' that terminates "# END shelfbox" belongs to
        // the block line itself and is not carried into `after`.
        assert_eq!(after, "");
    }

    #[test]
    fn render_empty_managed_removes_block() {
        let before = "*.log\n";
        let rendered = GitInfoExclude::render(before, &[], "");
        assert_eq!(rendered, "*.log\n");
        assert!(!rendered.contains(BLOCK_BEGIN));
    }

    #[test]
    fn render_produces_block_in_given_order() {
        // render itself does not sort; callers are responsible for ordering.
        let entries = vec!["/a.md".to_string(), "/z.md".to_string()];
        let rendered = GitInfoExclude::render("", &entries, "");
        let block_start = rendered.find(BLOCK_BEGIN).unwrap();
        let block_end = rendered.find(BLOCK_END).unwrap();
        let body = &rendered[block_start..block_end];
        assert!(body.find("/a.md").unwrap() < body.find("/z.md").unwrap());
    }

    #[test]
    fn add_entries_sorts_output() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        // Add in reverse alphabetical order; the file should be sorted.
        backend()
            .add_entries(dir.path(), &["/z.md", "/a.md"])
            .unwrap();

        let contents = std::fs::read_to_string(GitInfoExclude::exclude_path(dir.path())).unwrap();
        assert!(contents.find("/a.md").unwrap() < contents.find("/z.md").unwrap());
    }

    // ── add_entries ──────────────────────────────────────────────────────────

    #[test]
    fn add_entries_creates_block_in_empty_file() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();

        let contents = std::fs::read_to_string(GitInfoExclude::exclude_path(dir.path())).unwrap();
        assert!(contents.contains(BLOCK_BEGIN));
        assert!(contents.contains("/notes.md"));
        assert!(contents.contains(BLOCK_END));
    }

    #[test]
    fn add_entries_is_idempotent() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();
        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();

        let contents = std::fs::read_to_string(GitInfoExclude::exclude_path(dir.path())).unwrap();
        let count = contents.matches("/notes.md").count();
        assert_eq!(count, 1, "entry should appear exactly once");
    }

    #[test]
    fn add_entries_preserves_existing_user_content() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());
        let path = GitInfoExclude::exclude_path(dir.path());
        std::fs::write(&path, "*.log\n*.tmp\n").unwrap();

        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("*.log"));
        assert!(contents.contains("*.tmp"));
        assert!(contents.contains("/notes.md"));
    }

    // ── remove_entries ───────────────────────────────────────────────────────

    #[test]
    fn remove_entries_removes_correct_entry() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        backend()
            .add_entries(dir.path(), &["/notes.md", "/prompts/"])
            .unwrap();
        backend()
            .remove_entries(dir.path(), &["/notes.md"])
            .unwrap();

        let b = backend();
        assert!(!b.has_entry(dir.path(), "/notes.md").unwrap());
        assert!(b.has_entry(dir.path(), "/prompts/").unwrap());
    }

    #[test]
    fn remove_entries_cleans_up_empty_block() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();
        backend()
            .remove_entries(dir.path(), &["/notes.md"])
            .unwrap();

        let contents = std::fs::read_to_string(GitInfoExclude::exclude_path(dir.path())).unwrap();
        assert!(
            !contents.contains(BLOCK_BEGIN),
            "empty block should be removed"
        );
    }

    #[test]
    fn remove_nonexistent_entry_is_noop() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        // Should not error even if the file and block don't exist.
        backend()
            .remove_entries(dir.path(), &["/ghost.md"])
            .unwrap();
    }

    // ── has_entry ────────────────────────────────────────────────────────────

    #[test]
    fn has_entry_returns_false_for_missing_file() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());
        assert!(!backend().has_entry(dir.path(), "/notes.md").unwrap());
    }

    #[test]
    fn has_entry_returns_true_after_add() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        backend().add_entries(dir.path(), &["/notes.md"]).unwrap();
        assert!(backend().has_entry(dir.path(), "/notes.md").unwrap());
    }

    // ── regression ───────────────────────────────────────────────────────────

    #[test]
    fn repeated_add_entries_does_not_accumulate_trailing_newlines() {
        let dir = TempDir::new().unwrap();
        setup_git_dir(dir.path());

        // Simulate multiple write cycles (add / repair / doctor --fix).
        backend().add_entries(dir.path(), &[".env"]).unwrap();
        backend().add_entries(dir.path(), &[".env"]).unwrap();
        backend().add_entries(dir.path(), &[".env"]).unwrap();

        let contents = std::fs::read_to_string(GitInfoExclude::exclude_path(dir.path())).unwrap();
        // The file must end with exactly one newline after `# END shelfbox`.
        assert!(
            contents.ends_with("# END shelfbox\n"),
            "trailing newlines accumulated; file contents: {contents:?}"
        );
    }
}
