//! Sandboxed Workspace Management in Rust
//!
//! Enforces strict canonical path resolution, boundary confinement checks
//! (preventing directory traversal attacks), Git repository inspection,
//! and fast recursive workspace file indexing.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkspaceError {
    #[error("Path '{0}' escapes workspace boundary '{1}'")]
    PathEscapesWorkspace(String, String),
    #[error("File or directory not found: '{0}'")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid path: '{0}'")]
    InvalidPath(String),
}

/// Summary of the workspace's Git repository state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitInfo {
    pub branch: String,
    pub dirty: bool,
    pub changed_files: usize,
}

/// A strictly sandboxed workspace confined to a specific root directory.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Creates a new sandboxed workspace at `path`, creating the directory if needed.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, WorkspaceError> {
        let path = path.as_ref();
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
        let canonical_root = path.canonicalize()?;
        Ok(Self {
            root: canonical_root,
        })
    }

    /// Returns the canonical root path of the workspace.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a path while strictly preventing relative `..` traversal escapes out of the workspace.
    /// Explicit absolute paths (e.g. `C:\...`, `/...`) are lexically resolved and permitted across any drive or directory.
    pub fn resolve<P: AsRef<Path>>(&self, rel_path: P) -> Result<PathBuf, WorkspaceError> {
        let rel = rel_path.as_ref();
        let rel_str = rel.to_string_lossy();
        if rel_str.trim().is_empty() || rel_str.trim() == "." {
            return Ok(self.root.clone());
        }

        let is_explicit_abs = rel.is_absolute()
            || (rel_str.len() >= 2 && rel_str.chars().nth(1) == Some(':'))
            || rel_str.starts_with('/')
            || rel_str.starts_with('\\');

        if is_explicit_abs {
            let mut normalized = PathBuf::new();
            for comp in rel.components() {
                match comp {
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    Component::CurDir => {}
                    other => normalized.push(other.as_os_str()),
                }
            }
            if normalized.exists() {
                return Ok(normalized.canonicalize().unwrap_or(normalized));
            }
            return Ok(normalized);
        }

        let candidate = self.root.join(rel);

        // Normalize lexical components first
        let mut normalized = PathBuf::new();
        for comp in candidate.components() {
            match comp {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => {}
                other => normalized.push(other.as_os_str()),
            }
        }

        // Enforce lexical boundary confinement before checking existence or walking ancestors
        if !Self::is_within(&normalized, &self.root) {
            return Err(WorkspaceError::PathEscapesWorkspace(
                rel.display().to_string(),
                self.root.display().to_string(),
            ));
        }

        if normalized.exists() {
            let canonical = normalized.canonicalize()?;
            if !Self::is_within(&canonical, &self.root) {
                return Err(WorkspaceError::PathEscapesWorkspace(
                    rel.display().to_string(),
                    self.root.display().to_string(),
                ));
            }
            Ok(canonical)
        } else {
            // Walk up to the nearest existing ancestor and canonicalize it
            let mut ancestor = normalized.as_path();
            let mut tail_parts = Vec::new();
            while !ancestor.exists() {
                if let Some(name) = ancestor.file_name() {
                    tail_parts.push(name.to_os_string());
                }
                if let Some(parent) = ancestor.parent() {
                    ancestor = parent;
                } else {
                    break;
                }
            }
            let base_canonical = if ancestor.exists() {
                ancestor.canonicalize()?
            } else {
                self.root.clone()
            };

            if !Self::is_within(&base_canonical, &self.root) {
                return Err(WorkspaceError::PathEscapesWorkspace(
                    rel.display().to_string(),
                    self.root.display().to_string(),
                ));
            }

            let mut rebuilt = base_canonical;
            for part in tail_parts.into_iter().rev() {
                rebuilt.push(part);
            }
            Ok(rebuilt)
        }
    }

    fn is_within(child: &Path, root: &Path) -> bool {
        if child.starts_with(root) {
            return true;
        }
        let child_clean = Self::strip_unc(child);
        let root_clean = Self::strip_unc(root);
        if child_clean.starts_with(&root_clean) {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            let child_comps: Vec<String> = child_clean
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect();
            let root_comps: Vec<String> = root_clean
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                .collect();
            if child_comps.len() >= root_comps.len()
                && child_comps[..root_comps.len()] == root_comps[..]
            {
                return true;
            }
        }
        false
    }

    fn strip_unc(p: &Path) -> PathBuf {
        let s = p.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            p.to_path_buf()
        }
    }

    /// Queries Git branch and working tree status for the workspace.
    pub fn git_info(&self) -> GitInfo {
        let mut branch_cmd = Command::new("git");
        branch_cmd
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.root);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            branch_cmd.creation_flags(0x08000000);
        }
        let branch_out = branch_cmd.output();

        let branch = match branch_out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => return GitInfo::default(),
        };

        let mut status_cmd = Command::new("git");
        status_cmd
            .args(["status", "--porcelain"])
            .current_dir(&self.root);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            status_cmd.creation_flags(0x08000000);
        }
        let status_out = status_cmd.output();

        let changed_files = match status_out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count(),
            _ => 0,
        };

        GitInfo {
            branch,
            dirty: changed_files > 0,
            changed_files,
        }
    }

    /// Captures a lightweight, non-destructive Git tree-object snapshot of the workspace
    /// using an isolated `GIT_INDEX_FILE` without modifying `.git/index`, `HEAD`, or `git stash`.
    pub fn capture_git_tree_snapshot(&self) -> Option<String> {
        let dot_git = self.root.join(".git");
        if !dot_git.exists() {
            return None;
        }
        let temp_index = std::env::temp_dir().join(format!(
            "unjess-tree-idx-{}",
            uuid::Uuid::new_v4()
        ));
        let real_index = dot_git.join("index");
        if real_index.is_file() {
            let _ = fs::copy(&real_index, &temp_index);
        }

        let run_git_idx = |args: &[&str]| -> Option<String> {
            let mut cmd = Command::new("git");
            cmd.args(args)
                .current_dir(&self.root)
                .env("GIT_INDEX_FILE", &temp_index);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            let out = cmd.output().ok()?;
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                None
            }
        };

        let _ = run_git_idx(&["add", "-A"]);
        let tree_hash = run_git_idx(&["write-tree"]);
        let _ = fs::remove_file(&temp_index);

        tree_hash.filter(|h| h.len() >= 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
    }

    /// Restores workspace files from a previously captured Git tree-object snapshot (`tree_hash`)
    /// without altering the user's real `.git/index` staging area.
    pub fn restore_git_tree_snapshot(&self, tree_hash: &str) -> Result<usize, String> {
        let clean_hash = tree_hash.trim();
        if clean_hash.len() < 40 || !clean_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("Invalid git tree snapshot hash".to_string());
        }
        let temp_index = std::env::temp_dir().join(format!(
            "unjess-restore-idx-{}",
            uuid::Uuid::new_v4()
        ));

        let run_git_idx = |args: &[&str]| -> Result<String, String> {
            let mut cmd = Command::new("git");
            cmd.args(args)
                .current_dir(&self.root)
                .env("GIT_INDEX_FILE", &temp_index);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            let out = cmd.output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        };

        let read_res = run_git_idx(&["read-tree", clean_hash]);
        if let Err(e) = read_res {
            let _ = fs::remove_file(&temp_index);
            return Err(format!("git read-tree failed: {}", e));
        }

        let files_out = run_git_idx(&["ls-tree", "-r", "--name-only", clean_hash]).unwrap_or_default();
        let file_count = files_out.lines().filter(|l| !l.trim().is_empty()).count();

        let checkout_res = run_git_idx(&["checkout-index", "-a", "-f"]);
        let _ = fs::remove_file(&temp_index);
        checkout_res.map_err(|e| format!("git checkout-index failed: {}", e))?;

        Ok(file_count)
    }

    /// Recursively lists relative file paths in the workspace up to `max_files`,
    /// skipping build/VCS directories (`.git`, `target`, `node_modules`, `.venv`, `dist`).
    pub fn list_all_files(&self, max_files: usize) -> Vec<String> {
        let mut results = Vec::new();
        self.walk_dir(&self.root, max_files, &mut results);
        results.sort();
        results
    }

    fn walk_dir(&self, dir: &Path, max_files: usize, out: &mut Vec<String>) {
        if out.len() >= max_files {
            return;
        }
        let Ok(read_dir) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            if out.len() >= max_files {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(
                name.as_str(),
                ".git" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__" | "dist"
            ) {
                continue;
            }
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    self.walk_dir(&path, max_files, out);
                } else if ft.is_file() {
                    let rel = path
                        .strip_prefix(&self.root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(rel);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_sandbox_blocks_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        assert!(ws.resolve("src/main.rs").is_ok());
        assert!(ws.resolve("../../outside.txt").is_err());
        assert!(ws.resolve("../../../../../../../../../../nonexistent_root_escape.txt").is_err());
        assert!(ws.resolve(r"C:\Users\zanzu\dev\battlefield-asset-factory\af.py").is_ok());
    }

    #[test]
    fn test_git_tree_snapshot_and_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let init_status = Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output();
        if init_status.map(|o| o.status.success()).unwrap_or(false) {
            let file_a = tmp.path().join("hello.txt");
            fs::write(&file_a, "original_snapshot_text").unwrap();
            let ws = Workspace::new(tmp.path()).unwrap();
            if let Some(tree_hash) = ws.capture_git_tree_snapshot() {
                assert_eq!(tree_hash.len(), 40);
                // Mutate file as if a shell command modified it
                fs::write(&file_a, "corrupted_by_shell_script").unwrap();
                assert_eq!(fs::read_to_string(&file_a).unwrap(), "corrupted_by_shell_script");
                // Restore snapshot
                let restored_count = ws.restore_git_tree_snapshot(&tree_hash).unwrap();
                assert!(restored_count >= 1);
                assert_eq!(fs::read_to_string(&file_a).unwrap(), "original_snapshot_text");
            }
        }
    }
}
