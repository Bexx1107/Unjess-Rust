//! Sandboxed Core Tools & Diff Engine implemented in Rust.
//!
//! Provides memory-safe file operations strictly confined to the workspace root,
//! LCS-based unified diff calculation and 1-click revert, regex grep search, glob matching,
//! timeout-enforced PowerShell/POSIX command execution, SSRF-protected URL fetching,
//! DuckDuckGo web search, AST/code-intelligence tools, MCP tool invocation, and
//! JSON schema definitions for LLM tool calling.

use crate::codeintel::{
    extract_imports, find_callers, find_symbol_definitions, generate_repo_map, outline_file,
};
use crate::config::Settings;
use crate::mcp::{McpManager, McpServerConfig};
use crate::memory::MemoryStore;
use crate::skills::SkillsManager;
use crate::workspace::{Workspace, WorkspaceError};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("File edit failed: target string was not found in '{0}'")]
    TargetNotFound(String),
    #[error("File edit failed: target string matched {0} times (at lines {1}) in '{2}' — include more context lines or set replace_all: true")]
    AmbiguousMatch(usize, String, String),
    #[error("File already exists: '{0}' (pass overwrite: true)")]
    AlreadyExists(String),
    #[error("Execution error: {0}")]
    Execution(String),
}

/// Record of a single tool invocation during an agent turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: String,
    pub status: String,
    pub duration_ms: u64,
}

/// Unified diff and before/after snapshot for a file modification, creation, or deletion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub status: String, // "created" | "modified" | "deleted"
    pub additions: usize,
    pub deletions: usize,
    pub diff: String,
    pub original_content: String,
    pub new_content: String,
}

/// Directory entry metadata returned by `list_dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

pub struct ToolEngine {
    workspace: Workspace,
    abort_flag: Option<Arc<AtomicBool>>,
}

impl ToolEngine {
    pub fn new(workspace: Workspace) -> Self {
        Self {
            workspace,
            abort_flag: None,
        }
    }

    pub fn with_abort_flag(mut self, abort_flag: Arc<AtomicBool>) -> Self {
        self.abort_flag = Some(abort_flag);
        self
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Resolves a path inside the active workspace OR inside any registered skill directory
    /// (`~/.unjess-rust/skills`, `~/.unjess/skills`, `<workspace>/.agents/skills`, `<workspace>/.unjess/skills`).
    fn resolve_skill_or_workspace_path(
        &self,
        raw_path: &str,
        expect_dir: bool,
    ) -> Result<PathBuf, ToolError> {
        let trimmed = raw_path.trim();
        let ws_res = self.workspace.resolve(trimmed);
        if let Ok(ref p) = ws_res {
            if p.exists() {
                return Ok(p.clone());
            }
        }

        if !trimmed.contains("..") {
            let norm = trimmed.replace('\\', "/");
            let norm_lower = norm.to_lowercase();
            let skill_roots = [
                SkillsManager::skills_dir(),
                Settings::legacy_go_config_dir().join("skills"),
                self.workspace.root().join(".agents").join("skills"),
                self.workspace.root().join(".unjess").join("skills"),
            ];

            // 1. Direct absolute path inside any skill root (case-insensitive on Windows)
            let candidate = PathBuf::from(trimmed);
            if candidate.is_absolute() && candidate.exists() {
                let cand_norm = candidate.to_string_lossy().replace('\\', "/").to_lowercase();
                if skill_roots.iter().any(|r| {
                    let r_norm = r.to_string_lossy().replace('\\', "/").to_lowercase();
                    cand_norm.starts_with(&r_norm)
                }) {
                    return Ok(candidate);
                }
            }

            // 2. Extract `<skill>/<subpath>` after `/skills/` and search across all skill roots
            if let Some(idx) = norm_lower.find("/skills/") {
                let after_skills = norm[idx + "/skills/".len()..].trim_matches('/');
                if !after_skills.is_empty() {
                    for root in &skill_roots {
                        let try_path = root.join(after_skills);
                        if (expect_dir && try_path.is_dir()) || (!expect_dir && try_path.is_file()) || try_path.exists() {
                            return Ok(try_path);
                        }
                        // Also try stripping `-skill` suffix on the skill folder name
                        if let Some((first_seg, rest)) = after_skills.split_once('/') {
                            let alt_seg = first_seg.trim_end_matches("-skill");
                            let alt_path = root.join(alt_seg).join(rest);
                            if alt_path.exists() {
                                return Ok(alt_path);
                            }
                        }
                    }
                    return Err(ToolError::Execution(format!(
                        "Skill file or directory '{}' does not exist in any installed skill folder.",
                        after_skills
                    )));
                }
            }

            // 3. Relative skill companion path (e.g. `references/pro-rules.md` or `ui-ux-pro-max/references/pro-rules.md`)
            let rel_clean = norm.trim_start_matches("./").trim_start_matches('/');
            if rel_clean.starts_with("references/")
                || rel_clean.starts_with("scripts/")
                || rel_clean.starts_with("resources/")
                || rel_clean.starts_with("data/")
            {
                for root in &skill_roots {
                    if let Ok(entries) = fs::read_dir(root) {
                        for entry in entries.flatten() {
                            let try_path = entry.path().join(rel_clean);
                            if try_path.exists() {
                                return Ok(try_path);
                            }
                        }
                    }
                }
            } else if rel_clean.contains('/') {
                for root in &skill_roots {
                    let try_path = root.join(rel_clean);
                    if try_path.exists() {
                        return Ok(try_path);
                    }
                }
            }
        }

        ws_res.map_err(ToolError::Workspace)
    }

    /// Reads a file within the workspace (or within registered skill directories) with 1-indexed line numbers and a file range header.
    pub fn read_file(
        &self,
        rel_path: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<String, ToolError> {
        let abs_path = self.resolve_skill_or_workspace_path(rel_path, false)?;
        let content = fs::read_to_string(&abs_path)?;
        let norm_rel = rel_path.replace('\\', "/");

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        if total == 0 {
            return Ok(format!("[File: {} (0 lines, empty)]", norm_rel));
        }

        let start = start_line.unwrap_or(1).max(1).saturating_sub(1).min(total);
        let end = end_line.unwrap_or(total).min(total);

        if start >= end {
            return Ok(format!(
                "[File: {} ({} lines total, requested range {}..={} is empty)]",
                norm_rel,
                total,
                start + 1,
                end
            ));
        }

        let mut out = Vec::with_capacity(end - start + 1);
        out.push(format!(
            "[File: {} ({} lines total, showing lines {}..={})]",
            norm_rel,
            total,
            start + 1,
            end
        ));
        for (idx, line) in lines[start..end].iter().enumerate() {
            out.push(format!("{:>4}: {}", start + idx + 1, line));
        }
        Ok(out.join("\n"))
    }

    /// Writes a file within the workspace, creating parent directories if needed, and returns a `FileDiff`.
    pub fn write_file(
        &self,
        rel_path: &str,
        content: &str,
        overwrite: bool,
    ) -> Result<FileDiff, ToolError> {
        let abs_path = self.workspace.resolve(rel_path)?;
        let existed = abs_path.exists();

        if existed && !overwrite {
            return Err(ToolError::AlreadyExists(rel_path.to_string()));
        }

        let original_content = if existed {
            fs::read_to_string(&abs_path).unwrap_or_default()
        } else {
            String::new()
        };

        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&abs_path, content)?;

        let status = if existed { "modified" } else { "created" };
        Ok(Self::compute_diff(
            rel_path,
            status,
            &original_content,
            content,
        ))
    }

    /// Deletes a file within the workspace and returns a `FileDiff` with `status: "deleted"`
    /// so it can be reverted with 1 click.
    pub fn delete_file(&self, rel_path: &str) -> Result<FileDiff, ToolError> {
        let abs_path = self.workspace.resolve(rel_path)?;
        if !abs_path.is_file() {
            return Err(ToolError::Execution(format!(
                "File not found or is a directory: '{}'",
                rel_path
            )));
        }
        let original_content = fs::read_to_string(&abs_path).unwrap_or_default();
        fs::remove_file(&abs_path)?;
        Ok(Self::compute_diff(
            rel_path,
            "deleted",
            &original_content,
            "",
        ))
    }

    /// Edits a file by replacing `target` with `replacement`.
    /// Supports exact matching first, then falls back to normalized line-ending (`\r\n` vs `\n`)
    /// and trimmed line matching so minor whitespace differences never break autonomous edits.
    pub fn edit_file(
        &self,
        rel_path: &str,
        target: &str,
        replacement: &str,
    ) -> Result<FileDiff, ToolError> {
        self.edit_file_ext(rel_path, target, replacement, false)
    }

    /// Edits a file with optional `replace_all` support.
    pub fn edit_file_ext(
        &self,
        rel_path: &str,
        target: &str,
        replacement: &str,
        replace_all: bool,
    ) -> Result<FileDiff, ToolError> {
        let abs_path = self.workspace.resolve(rel_path)?;
        let original_content = fs::read_to_string(&abs_path)?;
        let new_content = Self::apply_replacement(
            rel_path,
            &original_content,
            target,
            replacement,
            replace_all,
        )?;

        fs::write(&abs_path, &new_content)?;
        Ok(Self::compute_diff(
            rel_path,
            "modified",
            &original_content,
            &new_content,
        ))
    }

    /// Applies multiple sequential `(target, replacement)` edits atomically to a single file.
    pub fn multi_edit_file(
        &self,
        rel_path: &str,
        edits: &[(String, String)],
    ) -> Result<FileDiff, ToolError> {
        if edits.is_empty() {
            return Err(ToolError::Execution("edits array cannot be empty".to_string()));
        }
        let abs_path = self.workspace.resolve(rel_path)?;
        let original_content = fs::read_to_string(&abs_path)?;
        let mut working = original_content.clone();
        for (target, replacement) in edits {
            working = Self::apply_replacement(rel_path, &working, target, replacement, false)?;
        }
        fs::write(&abs_path, &working)?;
        Ok(Self::compute_diff(
            rel_path,
            "modified",
            &original_content,
            &working,
        ))
    }

    fn match_line_numbers(content: &str, needle: &str) -> String {
        let mut lines = Vec::new();
        for (byte_idx, _) in content.match_indices(needle) {
            let line_no = content[..byte_idx].bytes().filter(|&b| b == b'\n').count() + 1;
            lines.push(line_no.to_string());
        }
        lines.join(", ")
    }

    fn apply_replacement(
        rel_path: &str,
        original_content: &str,
        target: &str,
        replacement: &str,
        replace_all: bool,
    ) -> Result<String, ToolError> {
        if target.is_empty() {
            return Err(ToolError::TargetNotFound(rel_path.to_string()));
        }
        let exact_matches = original_content.matches(target).count();
        if exact_matches == 1 {
            return Ok(original_content.replacen(target, replacement, 1));
        } else if exact_matches > 1 {
            if replace_all {
                return Ok(original_content.replace(target, replacement));
            }
            let line_nums = Self::match_line_numbers(original_content, target);
            return Err(ToolError::AmbiguousMatch(
                exact_matches,
                line_nums,
                rel_path.to_string(),
            ));
        }

        let norm_content = original_content.replace("\r\n", "\n");
        let norm_target = target.replace("\r\n", "\n");
        let norm_replacement = replacement.replace("\r\n", "\n");
        let norm_matches = norm_content.matches(&norm_target).count();
        if norm_matches == 1 || (norm_matches > 1 && replace_all) {
            let replaced = if replace_all {
                norm_content.replace(&norm_target, &norm_replacement)
            } else {
                norm_content.replacen(&norm_target, &norm_replacement, 1)
            };
            return Ok(if original_content.contains("\r\n") {
                replaced.replace('\n', "\r\n")
            } else {
                replaced
            });
        } else if norm_matches > 1 {
            let line_nums = Self::match_line_numbers(&norm_content, &norm_target);
            return Err(ToolError::AmbiguousMatch(
                norm_matches,
                line_nums,
                rel_path.to_string(),
            ));
        }

        if let Some(flex_replaced) =
            Self::flexible_line_replace(original_content, target, replacement)
        {
            Ok(flex_replaced)
        } else {
            Err(ToolError::TargetNotFound(rel_path.to_string()))
        }
    }

    fn flexible_line_replace(content: &str, target: &str, replacement: &str) -> Option<String> {
        let content_lines: Vec<&str> = content.lines().collect();
        let target_lines: Vec<&str> = target.lines().map(|l| l.trim()).collect();
        if target_lines.is_empty() || target_lines.len() > content_lines.len() {
            return None;
        }
        let mut match_idx = None;
        for start in 0..=(content_lines.len() - target_lines.len()) {
            let window_matches = (0..target_lines.len())
                .all(|offset| content_lines[start + offset].trim() == target_lines[offset]);
            if window_matches {
                if match_idx.is_some() {
                    return None; // ambiguous
                }
                match_idx = Some(start);
            }
        }
        let idx = match_idx?;
        let file_first_line = content_lines[idx];
        let file_indent: String = file_first_line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let target_first_has_indent = target
            .lines()
            .next()
            .map(|l| l.starts_with(' ') || l.starts_with('\t'))
            .unwrap_or(false);

        let mut out_lines: Vec<String> = content_lines[..idx]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        for r_line in replacement.lines() {
            if !target_first_has_indent && !file_indent.is_empty() && !r_line.trim().is_empty() {
                out_lines.push(format!("{}{}", file_indent, r_line));
            } else {
                out_lines.push(r_line.to_string());
            }
        }
        out_lines.extend(
            content_lines[idx + target_lines.len()..]
                .iter()
                .map(|s| (*s).to_string()),
        );
        let sep = if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        Some(out_lines.join(sep))
    }

    /// Computes a unified diff using common prefix/suffix stripping and bounded LCS alignment.
    pub fn compute_diff(path: &str, status: &str, old: &str, new: &str) -> FileDiff {
        let norm_path = path.replace('\\', "/");
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        let mut prefix_len = 0usize;
        while prefix_len < old_lines.len()
            && prefix_len < new_lines.len()
            && old_lines[prefix_len] == new_lines[prefix_len]
        {
            prefix_len += 1;
        }

        let mut suffix_len = 0usize;
        while suffix_len < (old_lines.len() - prefix_len)
            && suffix_len < (new_lines.len() - prefix_len)
            && old_lines[old_lines.len() - 1 - suffix_len]
                == new_lines[new_lines.len() - 1 - suffix_len]
        {
            suffix_len += 1;
        }

        let old_mid = &old_lines[prefix_len..old_lines.len() - suffix_len];
        let new_mid = &new_lines[prefix_len..new_lines.len() - suffix_len];

        let mut additions = 0usize;
        let mut deletions = 0usize;
        let mut body_ops: Vec<(char, &str)> = Vec::new();

        // Use exact LCS when the changed middle window is <= 1600 lines per side
        if old_mid.len() <= 1600 && new_mid.len() <= 1600 {
            let m = old_mid.len();
            let n = new_mid.len();
            let stride = n + 1;
            let mut dp = vec![0u16; (m + 1) * stride];
            for i in (0..m).rev() {
                for j in (0..n).rev() {
                    if old_mid[i] == new_mid[j] {
                        dp[i * stride + j] = dp[(i + 1) * stride + (j + 1)] + 1;
                    } else {
                        dp[i * stride + j] =
                            dp[(i + 1) * stride + j].max(dp[i * stride + (j + 1)]);
                    }
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < m && j < n {
                if old_mid[i] == new_mid[j] {
                    body_ops.push((' ', old_mid[i]));
                    i += 1;
                    j += 1;
                } else if dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)] {
                    deletions += 1;
                    body_ops.push(('-', old_mid[i]));
                    i += 1;
                } else {
                    additions += 1;
                    body_ops.push(('+', new_mid[j]));
                    j += 1;
                }
            }
            while i < m {
                deletions += 1;
                body_ops.push(('-', old_mid[i]));
                i += 1;
            }
            while j < n {
                additions += 1;
                body_ops.push(('+', new_mid[j]));
                j += 1;
            }
        } else {
            // Greedy lookahead anchor alignment for very large files (>1600 lines)
            let (mut i, mut j) = (0, 0);
            while i < old_mid.len() && j < new_mid.len() {
                if old_mid[i] == new_mid[j] {
                    body_ops.push((' ', old_mid[i]));
                    i += 1;
                    j += 1;
                } else if let Some(rel_j) = new_mid[j..(j + 30).min(new_mid.len())]
                    .iter()
                    .position(|&nl| nl == old_mid[i])
                {
                    for k in 0..rel_j {
                        additions += 1;
                        body_ops.push(('+', new_mid[j + k]));
                    }
                    j += rel_j;
                } else if let Some(rel_i) = old_mid[i..(i + 30).min(old_mid.len())]
                    .iter()
                    .position(|&ol| ol == new_mid[j])
                {
                    for k in 0..rel_i {
                        deletions += 1;
                        body_ops.push(('-', old_mid[i + k]));
                    }
                    i += rel_i;
                } else {
                    deletions += 1;
                    body_ops.push(('-', old_mid[i]));
                    additions += 1;
                    body_ops.push(('+', new_mid[j]));
                    i += 1;
                    j += 1;
                }
            }
            while i < old_mid.len() {
                deletions += 1;
                body_ops.push(('-', old_mid[i]));
                i += 1;
            }
            while j < new_mid.len() {
                additions += 1;
                body_ops.push(('+', new_mid[j]));
                j += 1;
            }
        }

        let mut diff_lines = Vec::new();
        diff_lines.push(format!("--- a/{}", norm_path));
        diff_lines.push(format!("+++ b/{}", norm_path));

        let ctx_before = prefix_len.saturating_sub(3);
        let ctx_after_end = (old_lines.len() - suffix_len + 3).min(old_lines.len());
        let old_start = ctx_before + 1;
        let old_count = (old_lines.len() - suffix_len - ctx_before)
            + (ctx_after_end - (old_lines.len() - suffix_len));
        let new_count = (old_count + additions).saturating_sub(deletions);
        diff_lines.push(format!(
            "@@ -{},{} +{},{} @@",
            old_start, old_count, old_start, new_count
        ));

        for &line in &old_lines[ctx_before..prefix_len] {
            diff_lines.push(format!(" {}", line));
        }

        // Collapse long runs of unchanged `' '` lines inside `body_ops` to +/- 3 context lines
        // and emit exact 1-indexed hunk headers (`@@ -old, +new @@`) when skipping interior lines
        let mut skipped_run = 0usize;
        let mut old_cur = prefix_len + 1;
        let mut new_cur = prefix_len + 1;
        for idx in 0..body_ops.len() {
            if diff_lines.len() >= 350 {
                diff_lines.push(" ...".to_string());
                break;
            }
            let (op, line) = body_ops[idx];
            if op != ' ' {
                if skipped_run > 0 {
                    diff_lines.push(format!(
                        "@@ -{}, +{} @@ (... skipped {} unchanged lines ...)",
                        old_cur, new_cur, skipped_run
                    ));
                    skipped_run = 0;
                }
                diff_lines.push(format!("{}{}", op, line));
                if op == '-' {
                    old_cur += 1;
                } else if op == '+' {
                    new_cur += 1;
                }
            } else {
                let near_change_before = (1..=3).any(|d| idx >= d && body_ops[idx - d].0 != ' ');
                let near_change_after =
                    (1..=3).any(|d| idx + d < body_ops.len() && body_ops[idx + d].0 != ' ');
                if near_change_before || near_change_after {
                    if skipped_run > 0 {
                        diff_lines.push(format!(
                            "@@ -{}, +{} @@ (... skipped {} unchanged lines ...)",
                            old_cur, new_cur, skipped_run
                        ));
                        skipped_run = 0;
                    }
                    diff_lines.push(format!(" {}", line));
                } else {
                    skipped_run += 1;
                }
                old_cur += 1;
                new_cur += 1;
            }
        }

        for &line in &old_lines[old_lines.len() - suffix_len..ctx_after_end] {
            diff_lines.push(format!(" {}", line));
        }

        FileDiff {
            path: norm_path,
            status: status.to_string(),
            additions,
            deletions,
            diff: diff_lines.join("\n"),
            original_content: old.to_string(),
            new_content: new.to_string(),
        }
    }

    /// Reverts a `FileDiff` on disk inside `workspace_root` (handles `"created"`, `"modified"`, and `"deleted"`).
    pub fn revert_diff(workspace_root: &Path, diff: &FileDiff) -> Result<(), String> {
        let ws = Workspace::new(workspace_root).map_err(|e| e.to_string())?;
        let target_path = ws.resolve(&diff.path).map_err(|e| e.to_string())?;
        if diff.status == "created" && diff.original_content.is_empty() {
            if target_path.exists() {
                fs::remove_file(&target_path).map_err(|e| e.to_string())?;
            }
        } else {
            if let Some(parent) = target_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&target_path, &diff.original_content).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Lists directory contents inside the workspace (or inside registered skill directories).
    pub fn list_dir(&self, rel_path: &str) -> Result<Vec<DirEntryInfo>, ToolError> {
        let abs_path = self.resolve_skill_or_workspace_path(rel_path, true)?;
        let mut entries = Vec::new();

        for entry in fs::read_dir(&abs_path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let file_name = entry.file_name().to_string_lossy().to_string();

            if matches!(
                file_name.as_str(),
                ".git" | "node_modules" | "target" | ".venv" | "__pycache__"
            ) {
                continue;
            }

            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(self.workspace.root())
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .replace('\\', "/");

            entries.push(DirEntryInfo {
                name: file_name,
                path: relative,
                is_dir: metadata.is_dir(),
                size_bytes: metadata.len(),
            });
        }

        entries.sort_by(|a, b| {
            if a.is_dir == b.is_dir {
                a.name.cmp(&b.name)
            } else if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });

        Ok(entries)
    }

    /// Converts a glob pattern (`**`, `*`, `?`, `{a,b}`) into an anchored case-insensitive `Regex`.
    fn glob_to_regex(glob: &str) -> Option<Regex> {
        let norm = glob.trim().replace('\\', "/");
        let has_brace_group = norm.contains('{') && norm.contains('}');
        if !norm.contains('*') && !norm.contains('?') && !has_brace_group {
            return None;
        }
        let mut re_str = if norm.contains('/') {
            String::from("(?i)^")
        } else {
            String::from("(?i)(?:^|/)")
        };
        let chars: Vec<char> = norm.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
                if i + 2 < chars.len() && chars[i + 2] == '/' {
                    re_str.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    re_str.push_str(".*");
                    i += 2;
                }
            } else if chars[i] == '*' {
                re_str.push_str("[^/]*");
                i += 1;
            } else if chars[i] == '?' {
                re_str.push_str("[^/]");
                i += 1;
            } else if chars[i] == '{' {
                if let Some(rel_close) = chars[i + 1..].iter().position(|&c| c == '}') {
                    let inner: String = chars[i + 1..i + 1 + rel_close].iter().collect();
                    let alts: Vec<String> = inner
                        .split(',')
                        .map(|s| regex::escape(s.trim()))
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !alts.is_empty() {
                        re_str.push_str("(?:");
                        re_str.push_str(&alts.join("|"));
                        re_str.push(')');
                        i += rel_close + 2;
                        continue;
                    }
                }
                re_str.push_str(&regex::escape(&chars[i].to_string()));
                i += 1;
            } else {
                re_str.push_str(&regex::escape(&chars[i].to_string()));
                i += 1;
            }
        }
        re_str.push('$');
        Regex::new(&re_str).ok()
    }

    /// Finds files in the workspace matching `pattern` (supports full globs `**/*.rs`, `*.toml`, or substring match).
    pub fn find_by_name(&self, pattern: &str) -> Result<Vec<String>, ToolError> {
        let all_files = self.workspace.list_all_files(5000);
        let trimmed = pattern.trim();
        if trimmed.is_empty() || trimmed == "*" || trimmed == "**/*" {
            return Ok(all_files.into_iter().take(200).collect());
        }

        if let Some(glob_re) = Self::glob_to_regex(trimmed) {
            let matched: Vec<String> = all_files
                .into_iter()
                .filter(|f| {
                    let file_name = Path::new(f)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    glob_re.is_match(f) || glob_re.is_match(&file_name)
                })
                .take(200)
                .collect();
            return Ok(matched);
        }

        let needle = trimmed.to_lowercase();
        let matched: Vec<String> = all_files
            .into_iter()
            .filter(|f| f.to_lowercase().contains(&needle))
            .take(200)
            .collect();
        Ok(matched)
    }

    /// Searches files in the workspace using a regular expression and returns `file:line: content` matches.
    pub fn grep_search(&self, query: &str, rel_path: Option<&str>) -> Result<String, ToolError> {
        self.grep_search_filtered(query, rel_path, None, None)
    }

    /// Searches files in the workspace with an optional `include` glob filter, `context_lines`, and line truncation.
    pub fn grep_search_filtered(
        &self,
        query: &str,
        rel_path: Option<&str>,
        include_glob: Option<&str>,
        context_lines: Option<usize>,
    ) -> Result<String, ToolError> {
        let ctx = context_lines.unwrap_or(0).min(5);
        let re = RegexBuilder::new(query)
            .case_insensitive(true)
            .build()
            .or_else(|_| {
                RegexBuilder::new(&regex::escape(query))
                    .case_insensitive(true)
                    .build()
            })
            .map_err(|e| ToolError::Execution(format!("Invalid regex: {}", e)))?;

        let include_re = include_glob
            .filter(|g| !g.trim().is_empty())
            .and_then(Self::glob_to_regex);

        let files = if let Some(sub) = rel_path.filter(|s| !s.trim().is_empty() && *s != ".") {
            let resolved = self.workspace.resolve(sub)?;
            let clean_sub = sub.replace('\\', "/");
            let prefix = clean_sub
                .trim_start_matches("./")
                .trim_matches('/')
                .to_string();
            if resolved.is_file() {
                vec![prefix]
            } else if prefix.is_empty() {
                self.workspace.list_all_files(4000)
            } else {
                let dir_prefix = format!("{}/", prefix);
                self.workspace
                    .list_all_files(5000)
                    .into_iter()
                    .filter(|f| f == &prefix || f.starts_with(&dir_prefix))
                    .collect()
            }
        } else {
            self.workspace.list_all_files(4000)
        };

        let mut hits = Vec::new();
        for rel in files {
            if hits.len() >= 150 {
                break;
            }
            if let Some(ref inc) = include_re {
                if !inc.is_match(&rel) {
                    continue;
                }
            }
            if let Ok(abs) = self.workspace.resolve(&rel) {
                if let Ok(content) = fs::read_to_string(&abs) {
                    let all_lines: Vec<&str> = content.lines().collect();
                    for (idx, line) in all_lines.iter().enumerate() {
                        if re.is_match(line) {
                            if ctx == 0 {
                                let trimmed_line = line.trim();
                                let snippet: String = if trimmed_line.chars().count() > 240 {
                                    let prefix: String = trimmed_line.chars().take(240).collect();
                                    format!("{}...", prefix)
                                } else {
                                    trimmed_line.to_string()
                                };
                                hits.push(format!("{}:{}: {}", rel, idx + 1, snippet));
                            } else {
                                let start_c = idx.saturating_sub(ctx);
                                let end_c = (idx + ctx + 1).min(all_lines.len());
                                let mut block = Vec::new();
                                for c_idx in start_c..end_c {
                                    let sep = if c_idx == idx { ':' } else { '-' };
                                    let l_txt: String =
                                        all_lines[c_idx].chars().take(200).collect();
                                    block.push(format!(
                                        "{}{}{}{}",
                                        rel,
                                        sep,
                                        c_idx + 1,
                                        format!(" {}", l_txt)
                                    ));
                                }
                                hits.push(block.join("\n"));
                            }
                            if hits.len() >= 150 {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if hits.is_empty() {
            Ok(format!("No matches found for '{}'", query))
        } else {
            Ok(hits.join(if ctx > 0 { "\n--\n" } else { "\n" }))
        }
    }

    /// Executes a shell command in the workspace root with real timeout and abort enforcement,
    /// capping stdout/stderr pipes to 2 MiB and killing the child process tree if it exceeds the deadline.
    pub fn run_command(&self, cmd: &str, timeout_secs: u64) -> Result<String, ToolError> {
        self.run_command_with_code(cmd, timeout_secs)
            .map(|(out, _)| out)
    }

    /// Executes a shell command in the workspace root and returns `(truncated_output, exit_code)`.
    pub fn run_command_with_code(
        &self,
        cmd: &str,
        timeout_secs: u64,
    ) -> Result<(String, i32), ToolError> {
        let effective_timeout = Duration::from_secs(timeout_secs.clamp(1, 3600));
        const MAX_PIPE_BYTES: u64 = 2 * 1024 * 1024;

        #[cfg(target_os = "windows")]
        let mut child = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    cmd,
                ])
                .current_dir(self.workspace.root())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()?
        };
        #[cfg(not(target_os = "windows"))]
        let mut child = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(self.workspace.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let child_pid = child.id();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(out) = stdout_pipe {
                let _ = out.take(MAX_PIPE_BYTES).read_to_end(&mut buf);
            }
            buf
        });
        let stderr_handle = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(err) = stderr_pipe {
                let _ = err.take(MAX_PIPE_BYTES).read_to_end(&mut buf);
            }
            buf
        });

        let start = Instant::now();
        let mut timed_out = false;
        let mut aborted = false;
        let exit_code = loop {
            match child.try_wait()? {
                Some(status) => break status.code().unwrap_or(-1),
                None => {
                    let user_aborted = self
                        .abort_flag
                        .as_ref()
                        .map(|f| f.load(Ordering::SeqCst))
                        .unwrap_or(false);
                    if user_aborted || start.elapsed() >= effective_timeout {
                        #[cfg(target_os = "windows")]
                        {
                            use std::os::windows::process::CommandExt;
                            let _ = Command::new("taskkill")
                                .args(["/F", "/T", "/PID", &child_pid.to_string()])
                                .creation_flags(0x08000000)
                                .status();
                        }
                        let _ = child.kill();
                        let _ = child.wait();
                        if user_aborted {
                            aborted = true;
                        } else {
                            timed_out = true;
                        }
                        break -1;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };

        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);

        let mut combined = String::new();
        if !stdout.trim().is_empty() {
            combined.push_str(stdout.trim());
        }
        if !stderr.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str("[stderr]\n");
            combined.push_str(stderr.trim());
        }
        if aborted {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str("[Command aborted by user]");
        } else if timed_out {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&format!(
                "[Command timed out and was terminated after {}s]",
                effective_timeout.as_secs()
            ));
        } else if combined.is_empty() {
            combined = format!("(Command completed with exit code {})", exit_code);
        } else if exit_code != 0 {
            combined.push_str(&format!("\n[exit code: {}]", exit_code));
        }

        Ok((Self::truncate_head_tail(&combined, 12000), exit_code))
    }

    pub fn truncate_head_tail(text: &str, max_chars: usize) -> String {
        let total_chars = text.chars().count();
        if total_chars <= max_chars {
            return text.to_string();
        }
        let half = max_chars / 2;
        let head: String = text.chars().take(half).collect();
        let tail: String = text
            .chars()
            .rev()
            .take(half)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!(
            "{}\n\n... [truncated {} chars] ...\n\n{}",
            head,
            total_chars.saturating_sub(max_chars),
            tail
        )
    }

    /// Validates a URL against Server-Side Request Forgery (SSRF) targeting loopback,
    /// RFC1918 private networks, link-local cloud metadata endpoints (`169.254.169.254`),
    /// or non-HTTP(S) schemes.
    pub fn validate_url_against_ssrf(raw_url: &str) -> Result<reqwest::Url, ToolError> {
        let parsed = reqwest::Url::parse(raw_url.trim())
            .map_err(|e| ToolError::Execution(format!("Invalid URL '{}': {}", raw_url, e)))?;

        match parsed.scheme() {
            "http" | "https" => {}
            other => {
                return Err(ToolError::Execution(format!(
                    "SSRF protection: unsupported URL scheme '{}' (only http/https allowed)",
                    other
                )));
            }
        }

        let host_raw = parsed
            .host_str()
            .ok_or_else(|| ToolError::Execution("URL is missing host".to_string()))?
            .to_lowercase();
        let host = host_raw.trim_matches(['[', ']']);

        if host == "localhost"
            || host.ends_with(".localhost")
            || host.ends_with(".local")
            || host.ends_with(".internal")
            || host == "metadata.google.internal"
            || host == "0.0.0.0"
            || host == "169.254.169.254"
            || host == "100.100.100.200"
        {
            return Err(ToolError::Execution(format!(
                "SSRF protection blocked internal/local host '{}'",
                host_raw
            )));
        }

        let port = parsed.port_or_known_default().unwrap_or(80);
        if let Ok(ip) = host.parse::<IpAddr>() {
            if Self::is_blocked_ip(&ip) {
                return Err(ToolError::Execution(format!(
                    "SSRF protection blocked private/loopback IP '{}'",
                    ip
                )));
            }
        } else if let Ok(addrs) = (host, port).to_socket_addrs() {
            for sock in addrs {
                if Self::is_blocked_ip(&sock.ip()) {
                    return Err(ToolError::Execution(format!(
                        "SSRF protection: host '{}' resolved to blocked internal IP '{}'",
                        host,
                        sock.ip()
                    )));
                }
            }
        }

        Ok(parsed)
    }

    fn is_blocked_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_unspecified()
                    || (v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64) // CGNAT / Alibaba metadata 100.100.100.200
            }
            IpAddr::V6(v6) => {
                if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                    return Self::is_blocked_ip(&IpAddr::V4(mapped_v4));
                }
                v6.is_loopback()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00 // Unique local fc00::/7 (includes fd00:ec2::254)
                    || (v6.segments()[0] & 0xffc0) == 0xfe80 // Link-local fe80::/10
            }
        }
    }

    /// Performs a DuckDuckGo HTML web search and extracts top results.
    pub fn web_search(&self, query: &str) -> Result<String, ToolError> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            Self::url_encode(query)
        );
        let query_owned = query.to_string();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) UnjessRust/4.0")
                .build()
                .map_err(|e| ToolError::Execution(e.to_string()))?;

            let resp = client
                .get(&url)
                .send()
                .map_err(|e| ToolError::Execution(e.to_string()))?;
            let status = resp.status();
            let mut buf = Vec::new();
            resp.take(512 * 1024)
                .read_to_end(&mut buf)
                .map_err(|e| ToolError::Execution(e.to_string()))?;
            if !status.is_success() {
                return Err(ToolError::Execution(format!(
                    "Web search HTTP {} error",
                    status
                )));
            }
            let html = String::from_utf8_lossy(&buf);
            let formatted = Self::parse_ddg_html_results(&query_owned, &html);
            Ok(Self::truncate_head_tail(&formatted, 8000))
        })
        .join()
        .map_err(|_| ToolError::Execution("web_search worker thread panicked".to_string()))?
    }

    /// Fetches a URL with SSRF validation (including redirect hops) and extracts readable text.
    pub fn read_url(&self, url: &str) -> Result<String, ToolError> {
        let validated = Self::validate_url_against_ssrf(url)?;
        std::thread::spawn(move || {
            let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.stop();
                }
                if let Err(e) = Self::validate_url_against_ssrf(attempt.url().as_str()) {
                    return attempt.error(e);
                }
                attempt.follow()
            });

            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(12))
                .redirect(redirect_policy)
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) UnjessRust/4.0")
                .build()
                .map_err(|e| ToolError::Execution(e.to_string()))?;

            let resp = client
                .get(validated)
                .send()
                .map_err(|e| ToolError::Execution(e.to_string()))?;
            let status = resp.status();
            let mut buf = Vec::new();
            resp.take(512 * 1024)
                .read_to_end(&mut buf)
                .map_err(|e| ToolError::Execution(e.to_string()))?;
            if !status.is_success() {
                return Err(ToolError::Execution(format!(
                    "HTTP {} fetching URL",
                    status
                )));
            }
            let body = String::from_utf8_lossy(&buf).into_owned();

            let text = if body.contains("<html") || body.contains("<body") {
                Self::strip_html_tags(&body)
            } else {
                body
            };
            Ok(Self::truncate_head_tail(&text, 8000))
        })
        .join()
        .map_err(|_| ToolError::Execution("read_url worker thread panicked".to_string()))?
    }

    fn url_encode(input: &str) -> String {
        let mut out = String::new();
        for b in input.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{:02X}", b)),
            }
        }
        out
    }

    fn decode_html_entities(s: &str) -> String {
        s.replace("&nbsp;", " ")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&#34;", "\"")
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&#x2F;", "/")
            .replace("&mdash;", "—")
            .replace("&ndash;", "–")
    }

    fn strip_html_tags(html: &str) -> String {
        // First strip <script>, <style>, <select>, <noscript>, <svg>, and <head> blocks completely
        let mut cleaned = html.to_string();
        for tag in ["script", "style", "select", "noscript", "svg", "head"] {
            let pat = format!(r"(?is)<{}\b[^>]*>.*?</{}>", tag, tag);
            if let Ok(re) = Regex::new(&pat) {
                cleaned = re.replace_all(&cleaned, " ").into_owned();
            }
        }

        let mut out = String::with_capacity(cleaned.len() / 2);
        let mut in_tag = false;
        for ch in cleaned.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => {
                    in_tag = false;
                    out.push(' ');
                }
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
        Self::decode_html_entities(&collapsed)
    }

    fn parse_ddg_html_results(query: &str, html: &str) -> String {
        let title_re =
            Regex::new(r#"(?is)<a[^>]*class="[^"]*result__a[^"]*"[^>]*>(.*?)</a>"#).ok();
        let snippet_re = Regex::new(
            r#"(?is)<a[^>]*class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<div[^>]*class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</div>"#,
        )
        .ok();
        let url_re =
            Regex::new(r#"(?is)<a[^>]*class="[^"]*result__url[^"]*"[^>]*>(.*?)</a>"#).ok();

        let titles: Vec<String> = title_re
            .as_ref()
            .map(|re| {
                re.captures_iter(html)
                    .filter_map(|c| c.get(1).map(|m| Self::strip_html_tags(m.as_str())))
                    .collect()
            })
            .unwrap_or_default();

        let snippets: Vec<String> = snippet_re
            .as_ref()
            .map(|re| {
                re.captures_iter(html)
                    .filter_map(|c| {
                        c.get(1)
                            .or_else(|| c.get(2))
                            .map(|m| Self::strip_html_tags(m.as_str()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let urls: Vec<String> = url_re
            .as_ref()
            .map(|re| {
                re.captures_iter(html)
                    .filter_map(|c| {
                        c.get(1)
                            .map(|m| Self::strip_html_tags(m.as_str()).trim().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !titles.is_empty() {
            let mut blocks = Vec::new();
            blocks.push(format!("## Web Search Results for `{}`\n", query.trim()));
            for (idx, title) in titles.iter().take(10).enumerate() {
                let url_txt = urls.get(idx).map(|s| s.as_str()).unwrap_or("");
                let snip_txt = snippets.get(idx).map(|s| s.as_str()).unwrap_or("");
                let mut item = format!("### {}. **{}**", idx + 1, title.trim());
                if !url_txt.is_empty() {
                    let full_url = if url_txt.starts_with("http") {
                        url_txt.to_string()
                    } else {
                        format!("https://{}", url_txt.trim_start_matches('/'))
                    };
                    item.push_str(&format!("\n- Source: `{}`", full_url));
                }
                if !snip_txt.is_empty() {
                    item.push_str(&format!("\n- {}", snip_txt.trim()));
                }
                blocks.push(item);
            }
            return blocks.join("\n\n");
        }

        // Fallback if DuckDuckGo markup structure differs
        let stripped = Self::strip_html_tags(html);
        if stripped.trim().is_empty() {
            format!("No web search results found for '{}'.", query)
        } else {
            format!("## Web Search Results for `{}`\n\n{}", query.trim(), stripped)
        }
    }

    /// Dispatches a tool call by `name` with JSON `args`, returning `(result_text, is_ok, optional_file_diff)`.
    pub fn execute_tool(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> (String, bool, Option<FileDiff>) {
        let get_str = |k: &str| -> String {
            args.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        match name {
            "read_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                let start = args
                    .get("start_line")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                let end = args
                    .get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                match self.read_file(&path, start, end) {
                    Ok(content) => (Self::truncate_head_tail(&content, 16000), true, None),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "write_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                let content = get_str("content");
                let overwrite = args
                    .get("overwrite")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                match self.write_file(&path, &content, overwrite) {
                    Ok(diff) => (
                        format!(
                            "Successfully wrote '{}' (+{} -{} lines)",
                            diff.path, diff.additions, diff.deletions
                        ),
                        true,
                        Some(diff),
                    ),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "delete_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                match self.delete_file(&path) {
                    Ok(diff) => (
                        format!(
                            "Successfully deleted '{}' (-{} lines)",
                            diff.path, diff.deletions
                        ),
                        true,
                        Some(diff),
                    ),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "edit_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                let target = {
                    let t = get_str("target");
                    if t.is_empty() {
                        get_str("old_str")
                    } else {
                        t
                    }
                };
                let replacement = {
                    let r = get_str("replacement");
                    if r.is_empty() {
                        get_str("new_str")
                    } else {
                        r
                    }
                };
                let replace_all = args
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match self.edit_file_ext(&path, &target, &replacement, replace_all) {
                    Ok(diff) => (
                        format!(
                            "Successfully edited '{}' (+{} -{} lines)",
                            diff.path, diff.additions, diff.deletions
                        ),
                        true,
                        Some(diff),
                    ),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "multi_edit_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                let edits_vec: Vec<(String, String)> = args
                    .get("edits")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                let t = item
                                    .get("target")
                                    .or_else(|| item.get("old_str"))
                                    .and_then(|v| v.as_str())?;
                                let r = item
                                    .get("replacement")
                                    .or_else(|| item.get("new_str"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                Some((t.to_string(), r.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self.multi_edit_file(&path, &edits_vec) {
                    Ok(diff) => (
                        format!(
                            "Successfully applied {} edits to '{}' (+{} -{} lines)",
                            edits_vec.len(),
                            diff.path,
                            diff.additions,
                            diff.deletions
                        ),
                        true,
                        Some(diff),
                    ),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "list_dir" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        ".".to_string()
                    } else {
                        p
                    }
                };
                match self.list_dir(&path) {
                    Ok(entries) => {
                        if entries.is_empty() {
                            (
                                format!("(Directory '{}' is empty — 0 files or subdirectories)", path),
                                true,
                                None,
                            )
                        } else {
                            let lines: Vec<String> = entries
                                .into_iter()
                                .map(|e| {
                                    if e.is_dir {
                                        format!("[DIR]  {}", e.path)
                                    } else {
                                        format!("[FILE] {} ({} bytes)", e.path, e.size_bytes)
                                    }
                                })
                                .collect();
                            (lines.join("\n"), true, None)
                        }
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "find_by_name" => {
                let pattern = get_str("pattern");
                match self.find_by_name(&pattern) {
                    Ok(files) => {
                        if files.is_empty() {
                            (
                                format!("(No files matching '{}' found in workspace)", pattern),
                                true,
                                None,
                            )
                        } else {
                            (files.join("\n"), true, None)
                        }
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "grep_search" => {
                let query = get_str("query");
                let path = args.get("path").and_then(|v| v.as_str());
                let include = args.get("include").and_then(|v| v.as_str());
                let context_lines = args
                    .get("context_lines")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                match self.grep_search_filtered(&query, path, include, context_lines) {
                    Ok(res) => (res, true, None),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "outline_file" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                match self.workspace.resolve(&path) {
                    Ok(abs) => {
                        let imports = extract_imports(&abs);
                        let symbols = outline_file(&abs);
                        if symbols.is_empty() && imports.is_empty() {
                            (format!("No symbols found in '{}'", path), true, None)
                        } else {
                            let mut out_sections = Vec::new();
                            if !imports.is_empty() {
                                out_sections.push(format!("Imports:\n  {}", imports.join("\n  ")));
                            }
                            let lines: Vec<String> = symbols
                                .into_iter()
                                .map(|s| {
                                    format!(
                                        "{}:L{}-L{}: [{}] {}",
                                        path.replace('\\', "/"),
                                        s.line,
                                        s.end_line,
                                        s.kind,
                                        s.signature
                                    )
                                })
                                .collect();
                            if !lines.is_empty() {
                                out_sections.push(lines.join("\n"));
                            }
                            (out_sections.join("\n\n"), true, None)
                        }
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "find_symbol" => {
                let symbol = get_str("symbol");
                if symbol.trim().is_empty() {
                    return ("Error: 'symbol' is required".to_string(), false, None);
                }
                let max_lines = args
                    .get("max_lines")
                    .and_then(|v| v.as_u64())
                    .map(|n| (n as usize).clamp(5, 200))
                    .unwrap_or(60);
                let defs = find_symbol_definitions(self.workspace.root(), symbol.trim());
                if defs.is_empty() {
                    (
                        format!("No symbol definitions found matching '{}'", symbol),
                        true,
                        None,
                    )
                } else {
                    let mut out = Vec::new();
                    for d in defs.into_iter().take(15) {
                        let mut snippet = String::new();
                        if let Ok(abs) = self.workspace.resolve(&d.file) {
                            if let Ok(content) = fs::read_to_string(abs) {
                                let lines: Vec<&str> = content.lines().collect();
                                let start_idx = d.line.saturating_sub(1).min(lines.len());
                                let end_idx = d
                                    .end_line
                                    .min(start_idx + max_lines)
                                    .min(lines.len());
                                if start_idx < end_idx {
                                    let body: Vec<String> = lines[start_idx..end_idx]
                                        .iter()
                                        .enumerate()
                                        .map(|(i, l)| format!("  {:>4}: {}", start_idx + i + 1, l))
                                        .collect();
                                    snippet = format!("\n{}", body.join("\n"));
                                }
                            }
                        }
                        out.push(format!(
                            "{}:L{}-L{} [{}] {}{}",
                            d.file, d.line, d.end_line, d.kind, d.signature, snippet
                        ));
                    }
                    (out.join("\n\n"), true, None)
                }
            }
            "find_callers" => {
                let symbol = get_str("symbol");
                if symbol.trim().is_empty() {
                    return ("Error: 'symbol' is required".to_string(), false, None);
                }
                let hits = find_callers(self.workspace.root(), symbol.trim());
                if hits.is_empty() {
                    (format!("No callers found for '{}'", symbol), true, None)
                } else {
                    (hits.join("\n"), true, None)
                }
            }
            "repo_map" => {
                let max_files = args
                    .get("max_files")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(60);
                let map = generate_repo_map(self.workspace.root(), max_files, 12000);
                if map.is_empty() {
                    ("Workspace repository map is empty.".to_string(), true, None)
                } else {
                    (map, true, None)
                }
            }
            "run_command" => {
                let cmd = {
                    let c = get_str("command");
                    if c.is_empty() {
                        get_str("cmd")
                    } else {
                        c
                    }
                };
                let timeout = args
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(120);
                match self.run_command(&cmd, timeout) {
                    Ok(out) => (out, true, None),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "web_search" => {
                let query = get_str("query");
                match self.web_search(&query) {
                    Ok(out) => (out, true, None),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "read_url" => {
                let url = get_str("url");
                match self.read_url(&url) {
                    Ok(out) => (out, true, None),
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "save_memory" => {
                let category = {
                    let c = get_str("category");
                    if c.is_empty() {
                        "general".to_string()
                    } else {
                        c
                    }
                };
                let key = get_str("key");
                let content = get_str("content");
                let entry = MemoryStore::save_entry(&category, &key, &content);
                (
                    format!(
                        "Saved memory entry [{}] {}: {}",
                        entry.category, entry.key, entry.id
                    ),
                    true,
                    None,
                )
            }
            "recall_memory" => {
                let query = get_str("query");
                let matches = MemoryStore::recall(&query);
                if matches.is_empty() {
                    ("No matching memories found.".to_string(), true, None)
                } else {
                    let formatted: Vec<String> = matches
                        .into_iter()
                        .map(|m| format!("[{}] {}: {}", m.category, m.key, m.content))
                        .collect();
                    (formatted.join("\n"), true, None)
                }
            }
            "delete_memory" => {
                let target = {
                    let k = get_str("key");
                    if k.is_empty() {
                        get_str("id")
                    } else {
                        k
                    }
                };
                if target.trim().is_empty() {
                    return ("Error: 'key' or 'id' is required".to_string(), false, None);
                }
                if MemoryStore::delete_entry(target.trim()) {
                    (format!("Deleted memory entry '{}'", target), true, None)
                } else {
                    (
                        format!("No memory entry found matching '{}'", target),
                        false,
                        None,
                    )
                }
            }
            "git_status" => {
                let mut cmd = Command::new("git");
                cmd.args(["status", "--short", "--branch"])
                    .current_dir(self.workspace.root())
                    .stdin(Stdio::null());
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                match cmd.output() {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let combined = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
                            stderr.trim().to_string()
                        } else {
                            stdout.trim().to_string()
                        };
                        (Self::truncate_head_tail(&combined, 12000), out.status.success(), None)
                    }
                    Err(e) => (format!("Error running git status: {}", e), false, None),
                }
            }
            "git_diff" => {
                let path = get_str("path");
                let mut cmd = Command::new("git");
                cmd.arg("diff")
                    .current_dir(self.workspace.root())
                    .stdin(Stdio::null());
                if !path.trim().is_empty() {
                    let Ok(abs) = self.workspace.resolve(path.trim()) else {
                        return (
                            format!("Error: invalid workspace path '{}'", path),
                            false,
                            None,
                        );
                    };
                    cmd.arg("--").arg(abs);
                }
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                match cmd.output() {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let combined = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
                            stderr.trim().to_string()
                        } else if stdout.trim().is_empty() {
                            "(No unstaged git diff changes)".to_string()
                        } else {
                            stdout.trim().to_string()
                        };
                        (Self::truncate_head_tail(&combined, 12000), out.status.success(), None)
                    }
                    Err(e) => (format!("Error running git diff: {}", e), false, None),
                }
            }
            "create_skill_plugin" => {
                let name = get_str("name");
                let description = get_str("description");
                let instructions = {
                    let i = get_str("instructions");
                    if i.is_empty() {
                        get_str("content")
                    } else {
                        i
                    }
                };
                let script_name = get_str("script_name");
                let script_content = get_str("script_content");

                match SkillsManager::save_skill(&name, &description, &instructions) {
                    Ok(skill) => {
                        let mut msg = format!(
                            "Created and hot-loaded Skill Plugin '{}' ({}) at {}",
                            skill.name, skill.id, skill.path
                        );
                        if !script_name.trim().is_empty() && !script_content.is_empty() {
                            let safe_script = Path::new(script_name.trim())
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "plugin.ps1".to_string());
                            let scripts_dir = SkillsManager::skills_dir()
                                .join(&skill.id)
                                .join("scripts");
                            let _ = fs::create_dir_all(&scripts_dir);
                            let script_path = scripts_dir.join(&safe_script);
                            if let Err(e) = fs::write(&script_path, &script_content) {
                                return (
                                    format!("Saved SKILL.md but failed writing script: {}", e),
                                    false,
                                    None,
                                );
                            }
                            msg.push_str(&format!(
                                " with executable script '{}'",
                                script_path.display()
                            ));
                        }
                        (msg, true, None)
                    }
                    Err(e) => (format!("Error creating skill plugin: {}", e), false, None),
                }
            }
            "register_mcp_plugin" => {
                let name = get_str("name");
                if name.trim().is_empty() {
                    return ("Error: MCP plugin 'name' is required".to_string(), false, None);
                }
                let safe_plugin_id = Path::new(name.trim())
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "mcp-plugin".to_string());
                let mut command = get_str("command");
                let url = get_str("url");
                let description = {
                    let d = get_str("description");
                    if d.is_empty() {
                        "Self-authored Unjess MCP Plugin".to_string()
                    } else {
                        d
                    }
                };
                let mut args_vec: Vec<String> = args
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let script_name = get_str("script_name");
                let script_content = get_str("script_content");
                if !script_name.trim().is_empty() && !script_content.is_empty() {
                    let safe_file = Path::new(script_name.trim())
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "server.js".to_string());
                    let plugin_dir = Settings::config_dir()
                        .join("plugins")
                        .join(&safe_plugin_id);
                    let _ = fs::create_dir_all(&plugin_dir);
                    let full_script_path = plugin_dir.join(&safe_file);
                    if let Err(e) = fs::write(&full_script_path, &script_content) {
                        return (
                            format!("Failed to write MCP plugin script: {}", e),
                            false,
                            None,
                        );
                    }
                    if command.is_empty() {
                        if safe_file.ends_with(".py") {
                            command = "python".to_string();
                        } else if safe_file.ends_with(".ps1") {
                            command = "powershell".to_string();
                        } else {
                            command = "node".to_string();
                        }
                    }
                    if args_vec.is_empty() {
                        args_vec.push(full_script_path.to_string_lossy().to_string());
                    }
                }

                let mut servers = McpManager::load();
                servers.retain(|s| !s.name.eq_ignore_ascii_case(&safe_plugin_id));
                servers.push(McpServerConfig {
                    name: safe_plugin_id.clone(),
                    command,
                    url,
                    args: args_vec,
                    env: std::collections::HashMap::new(),
                    cwd: None,
                    headers: std::collections::HashMap::new(),
                    enabled: true,
                    description,
                    cached_tools: Vec::new(),
                    last_probe_ok: None,
                    last_error: None,
                });
                match McpManager::save(&servers) {
                    Ok(()) => (
                        format!(
                            "Registered and enabled MCP Plugin '{}' in ~/.unjess-rust/mcp.json",
                            safe_plugin_id
                        ),
                        true,
                        None,
                    ),
                    Err(e) => (format!("Error saving MCP plugin: {}", e), false, None),
                }
            }
            "run_skill_plugin" => {
                let raw_skill_id = get_str("skill_id");
                if raw_skill_id.trim().is_empty() {
                    return ("Error: 'skill_id' is required".to_string(), false, None);
                }
                let script_name = get_str("script_name");
                let reference_name = get_str("reference_name");
                let script_args = get_str("args");

                let skills = SkillsManager::list_skills_for_workspace(Some(self.workspace.root()));
                let target_slug = crate::skills::slugify_skill_id(&raw_skill_id);
                let matched_skill = skills.into_iter().find(|s| {
                    s.id.eq_ignore_ascii_case(raw_skill_id.trim())
                        || s.name.eq_ignore_ascii_case(raw_skill_id.trim())
                        || crate::skills::slugify_skill_id(&s.name) == target_slug
                });

                if !reference_name.trim().is_empty() {
                    let rel = if reference_name.trim().starts_with("references/") {
                        reference_name.trim().to_string()
                    } else {
                        format!("references/{}", reference_name.trim())
                    };
                    match SkillsManager::read_skill_file(
                        raw_skill_id.trim(),
                        &rel,
                        Some(self.workspace.root()),
                    )
                    .or_else(|_| {
                        SkillsManager::read_skill_file(
                            raw_skill_id.trim(),
                            reference_name.trim(),
                            Some(self.workspace.root()),
                        )
                    }) {
                        Ok(txt) => return (txt, true, None),
                        Err(e) => return (format!("Error reading skill reference: {}", e), false, None),
                    }
                }

                if !script_name.trim().is_empty() {
                    if script_args
                        .chars()
                        .any(|c| matches!(c, ';' | '|' | '&' | '`' | '$' | '(' | ')' | '<' | '>' | '\n' | '\r'))
                    {
                        return (
                            "Error: script 'args' contains disallowed shell control characters"
                                .to_string(),
                            false,
                            None,
                        );
                    }
                    let safe_script = Path::new(script_name.trim())
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let safe_skill_id = matched_skill
                        .as_ref()
                        .map(|s| s.id.clone())
                        .unwrap_or_else(|| target_slug.clone());

                    let mut candidate_paths = Vec::new();
                    if let Some(ref sk) = matched_skill {
                        if !sk.dir_path.is_empty() {
                            let d = PathBuf::from(&sk.dir_path);
                            candidate_paths.push(d.join("scripts").join(&safe_script));
                            candidate_paths.push(d.join(&safe_script));
                        }
                    }
                    candidate_paths.extend([
                        SkillsManager::skills_dir()
                            .join(&safe_skill_id)
                            .join("scripts")
                            .join(&safe_script),
                        Settings::legacy_go_config_dir()
                            .join("skills")
                            .join(&safe_skill_id)
                            .join("scripts")
                            .join(&safe_script),
                        self.workspace
                            .root()
                            .join(".agents")
                            .join("skills")
                            .join(&safe_skill_id)
                            .join("scripts")
                            .join(&safe_script),
                        self.workspace
                            .root()
                            .join(".unjess")
                            .join("skills")
                            .join(&safe_skill_id)
                            .join("scripts")
                            .join(&safe_script),
                    ]);

                    let Some(script_path) = candidate_paths.into_iter().find(|p| p.is_file()) else {
                        return (
                            format!(
                                "Plugin script '{}' not found for skill '{}'",
                                safe_script, safe_skill_id
                            ),
                            false,
                            None,
                        );
                    };

                    let cmd_line = if safe_script.ends_with(".py") {
                        format!("python \"{}\" {}", script_path.display(), script_args)
                    } else if safe_script.ends_with(".js") {
                        format!("node \"{}\" {}", script_path.display(), script_args)
                    } else if safe_script.ends_with(".ps1") {
                        format!(
                            "powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\" {}",
                            script_path.display(),
                            script_args
                        )
                    } else if safe_script.ends_with(".sh") {
                        format!("sh \"{}\" {}", script_path.display(), script_args)
                    } else if cfg!(target_os = "windows") {
                        format!("& \"{}\" {}", script_path.display(), script_args)
                    } else {
                        format!("sh \"{}\" {}", script_path.display(), script_args)
                    };
                    match self.run_command(&cmd_line, 120) {
                        Ok(out) => (out, true, None),
                        Err(e) => (format!("Error executing plugin script: {}", e), false, None),
                    }
                } else if let Some(sk) = matched_skill {
                    let mut out = sk.body.clone();
                    if !sk.dir_path.is_empty() || !sk.scripts.is_empty() || !sk.references.is_empty() {
                        out.push_str("\n\n---\n### Skill Directory & Companion Files\n");
                        if !sk.dir_path.is_empty() {
                            out.push_str(&format!("- **Skill Directory**: `{}`\n", sk.dir_path));
                        }
                        if !sk.scripts.is_empty() {
                            out.push_str(&format!(
                                "- **Scripts** (run via `run_skill_plugin` with `script_name`): `{}`\n",
                                sk.scripts.join("`, `")
                            ));
                        }
                        if !sk.references.is_empty() {
                            out.push_str(&format!(
                                "- **References** (read via `run_skill_plugin` with `reference_name`): `{}`\n",
                                sk.references.join("`, `")
                            ));
                        }
                    }
                    (out, true, None)
                } else {
                    (
                        format!("Skill plugin '{}' not found.", raw_skill_id),
                        false,
                        None,
                    )
                }
            }
            "probe_mcp_server" => {
                let server_name = get_str("server");
                let servers = McpManager::load();
                let Some(cfg) = servers
                    .into_iter()
                    .find(|s| s.name.eq_ignore_ascii_case(server_name.trim()))
                else {
                    return (
                        format!("MCP server '{}' not found in mcp.json", server_name),
                        false,
                        None,
                    );
                };
                let probe = McpManager::probe_server(&cfg);
                let ok = probe.ok;
                (
                    serde_json::to_string_pretty(&probe).unwrap_or_default(),
                    ok,
                    None,
                )
            }
            "mcp_status" => {
                let servers = McpManager::load();
                let mut lines = vec!["### Configured MCP Servers".to_string()];
                for s in &servers {
                    let status = if !s.enabled {
                        "disabled"
                    } else if s.last_probe_ok == Some(true) {
                        "online"
                    } else {
                        "enabled"
                    };
                    let tool_names: Vec<String> =
                        s.cached_tools.iter().map(|t| t.name.clone()).collect();
                    lines.push(format!(
                        "- **{}** ({}): {} | tools ({}): [{}]",
                        s.name,
                        status,
                        s.description,
                        tool_names.len(),
                        tool_names.join(", ")
                    ));
                }
                (lines.join("\n"), true, None)
            }
            "security_scan" => {
                let report = crate::security::scan_workspace(self.workspace.root());
                (
                    serde_json::to_string_pretty(&report).unwrap_or_default(),
                    true,
                    None,
                )
            }
            "schedule_timer" => {
                let secs = args
                    .get("seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10)
                    .max(1);
                let action = get_str("action");
                let payload = get_str("prompt_or_command");
                if payload.trim().is_empty() {
                    return (
                        "schedule_timer requires a non-empty 'prompt_or_command'".to_string(),
                        false,
                        None,
                    );
                }
                let ws_str = self.workspace.root().to_string_lossy().to_string();
                let task = crate::scheduler::SchedulerManager::start_timer(
                    secs,
                    if action.is_empty() { "command" } else { &action },
                    &payload,
                    &ws_str,
                    None,
                );
                (
                    serde_json::to_string_pretty(&task).unwrap_or_default(),
                    true,
                    None,
                )
            }
            "start_watcher" => {
                let target = get_str("watch_target");
                let interval = args
                    .get("interval_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5)
                    .max(2);
                let on_change = get_str("on_change_command");
                let ws_str = self.workspace.root().to_string_lossy().to_string();
                let task = crate::scheduler::SchedulerManager::start_watcher(
                    if target.is_empty() { "." } else { &target },
                    interval,
                    &on_change,
                    &ws_str,
                    None,
                );
                (
                    serde_json::to_string_pretty(&task).unwrap_or_default(),
                    true,
                    None,
                )
            }
            "manage_background_tasks" => {
                let action = get_str("action");
                let task_id = get_str("task_id");
                if action.eq_ignore_ascii_case("cancel") || action.eq_ignore_ascii_case("kill") {
                    let cancelled = crate::scheduler::SchedulerManager::cancel_live_task(
                        if task_id.is_empty() { "all" } else { &task_id },
                    );
                    (
                        format!("Cancelled {} background task(s).", cancelled),
                        true,
                        None,
                    )
                } else {
                    let live = crate::scheduler::SchedulerManager::list_live_tasks();
                    let cron = crate::scheduler::SchedulerManager::load();
                    let payload = serde_json::json!({
                        "live_timers_and_watchers": live,
                        "cron_schedules": cron
                    });
                    (
                        serde_json::to_string_pretty(&payload).unwrap_or_default(),
                        true,
                        None,
                    )
                }
            }
            "call_mcp_tool" => {
                let server_name = get_str("server");
                let tool_name = get_str("tool");
                let tool_args = args
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                match McpManager::call_tool(server_name.trim(), tool_name.trim(), &tool_args) {
                    Ok(val) => (val, true, None),
                    Err(e) => (format!("MCP tool error: {}", e), false, None),
                }
            }
            other if other.starts_with("mcp__") => {
                let rest = &other[5..];
                if let Some((srv, tl)) = rest.split_once("__") {
                    match McpManager::call_tool(srv, tl, args) {
                        Ok(val) => (val, true, None),
                        Err(e) => (format!("MCP tool error: {}", e), false, None),
                    }
                } else {
                    (format!("Unknown MCP tool '{}'", other), false, None)
                }
            }
            other => (format!("Unknown tool '{}'", other), false, None),
        }
    }

    /// Returns OpenAI-compatible JSON function tool schemas for LLM tool-calling,
    /// including dynamically discovered tools from enabled MCP servers.
    pub fn tool_schemas() -> serde_json::Value {
        let mut base = serde_json::json!([
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read the contents of a file inside the workspace with 1-indexed line numbers.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "start_line": { "type": "integer", "description": "Optional 1-indexed start line" },
                            "end_line": { "type": "integer", "description": "Optional 1-indexed end line" }
                        },
                        "required": ["path"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "write_file",
                    "description": "Create or overwrite a file in the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "content": { "type": "string", "description": "Full content to write" },
                            "overwrite": { "type": "boolean", "description": "Whether to overwrite if file exists" }
                        },
                        "required": ["path", "content"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "edit_file",
                    "description": "Replace a target block of text in a file with replacement text.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "target": { "type": "string", "description": "Exact existing text to replace" },
                            "replacement": { "type": "string", "description": "New text to insert" },
                            "replace_all": { "type": "boolean", "description": "Optional: replace all occurrences of target in the file (default false)" }
                        },
                        "required": ["path", "target", "replacement"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "multi_edit_file",
                    "description": "Apply multiple sequential target/replacement edits atomically to a single file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file" },
                            "edits": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "target": { "type": "string" },
                                        "replacement": { "type": "string" }
                                    },
                                    "required": ["target", "replacement"]
                                }
                            }
                        },
                        "required": ["path", "edits"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "delete_file",
                    "description": "Delete a file inside the workspace (revertible via FileDiff).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the file to delete" }
                        },
                        "required": ["path"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "list_dir",
                    "description": "List files and subdirectories in a workspace directory.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative directory path (use '.' for root)" }
                        },
                        "required": ["path"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "find_by_name",
                    "description": "Find files in the workspace matching a glob (`**/*.rs`, `*.toml`) or filename substring.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": { "type": "string", "description": "Glob or substring pattern" }
                        },
                        "required": ["pattern"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "grep_search",
                    "description": "Search file contents across the workspace using a regular expression.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Regex pattern to search for" },
                            "path": { "type": "string", "description": "Optional relative directory or file filter" },
                            "include": { "type": "string", "description": "Optional file glob filter (e.g. '*.rs')" },
                            "context_lines": { "type": "integer", "description": "Optional number of surrounding context lines (0-5)" }
                        },
                        "required": ["query"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "find_symbol",
                    "description": "Find symbol definitions (functions, structs, traits, classes) across the workspace with their source body.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "symbol": { "type": "string", "description": "Symbol name or substring to locate" },
                            "max_lines": { "type": "integer", "description": "Optional max body lines to display per symbol (default 60, max 200)" }
                        },
                        "required": ["symbol"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "outline_file",
                    "description": "Extract structural code symbols (functions, structs, traits, classes) and line numbers from a file.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative path to the source file" }
                        },
                        "required": ["path"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "find_callers",
                    "description": "Find all call sites and references to a symbol across the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "symbol": { "type": "string", "description": "Function, method, or type name" }
                        },
                        "required": ["symbol"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "repo_map",
                    "description": "Generate a structural repository map of files and top-level symbols across the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "max_files": { "type": "integer", "description": "Maximum number of files to summarize (default 60)" }
                        }
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "run_command",
                    "description": "Execute a shell command inside the workspace root with timeout enforcement.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string", "description": "Command line to execute" },
                            "timeout_secs": { "type": "integer", "description": "Optional timeout in seconds (default 120)" }
                        },
                        "required": ["command"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": "Search the web for documentation or technical references.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Search query" }
                        },
                        "required": ["query"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "read_url",
                    "description": "Fetch and extract readable text from an HTTP/HTTPS URL (SSRF-protected).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "HTTP/HTTPS URL" }
                        },
                        "required": ["url"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "save_memory",
                    "description": "Persist a cross-session fact, preference, or architectural note.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "category": { "type": "string", "description": "Memory category (e.g. architecture, preference)" },
                            "key": { "type": "string", "description": "Short key/title" },
                            "content": { "type": "string", "description": "Detailed note to remember" }
                        },
                        "required": ["key", "content"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "recall_memory",
                    "description": "Search persistent cross-session memory entries.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Search keyword" }
                        },
                        "required": ["query"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "delete_memory",
                    "description": "Delete a stale or obsolete entry from persistent cross-session memory.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "key": { "type": "string", "description": "Memory key or ID to delete" }
                        },
                        "required": ["key"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "git_status",
                    "description": "Inspect the workspace Git branch and modified/untracked files.",
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "git_diff",
                    "description": "Inspect the current Git working tree diff in the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Optional relative file path to diff" }
                        }
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "create_skill_plugin",
                    "description": "Create and hot-load a new Unjess Skill Plugin (SKILL.md + optional executable script in ~/.unjess-rust/skills/<id>/).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Plugin/skill name (e.g. 'docker-auditor')" },
                            "description": { "type": "string", "description": "Short summary of what this plugin does" },
                            "instructions": { "type": "string", "description": "Full Markdown instructions (SKILL.md content)" },
                            "script_name": { "type": "string", "description": "Optional executable script filename (e.g. 'audit.ps1', 'tool.py', 'run.js')" },
                            "script_content": { "type": "string", "description": "Optional full source code for the executable plugin script" }
                        },
                        "required": ["name", "description", "instructions"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "register_mcp_plugin",
                    "description": "Create and register a new Model Context Protocol (MCP) server plugin in ~/.unjess-rust/mcp.json.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Unique MCP server name" },
                            "description": { "type": "string", "description": "Description of the MCP plugin" },
                            "command": { "type": "string", "description": "Command to launch the server (e.g. 'node', 'python', 'npx')" },
                            "args": { "type": "array", "items": { "type": "string" }, "description": "Command arguments" },
                            "url": { "type": "string", "description": "Optional HTTP/SSE MCP endpoint URL" },
                            "script_name": { "type": "string", "description": "Optional script filename to write into ~/.unjess-rust/plugins/<name>/" },
                            "script_content": { "type": "string", "description": "Optional source code for the MCP server script" }
                        },
                        "required": ["name", "description"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "run_skill_plugin",
                    "description": "Read a skill plugin's full instructions, read a reference file, or execute its bundled script.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "skill_id": { "type": "string", "description": "Skill ID or name" },
                            "script_name": { "type": "string", "description": "Optional script filename inside the skill's scripts/ folder to execute" },
                            "reference_name": { "type": "string", "description": "Optional reference filename inside the skill's references/ folder to read" },
                            "args": { "type": "string", "description": "Optional CLI arguments to pass to the script" }
                        },
                        "required": ["skill_id"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "probe_mcp_server",
                    "description": "Probe a configured MCP server via JSON-RPC 2.0 (`initialize` + `tools/list`) to inspect its status and tools.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "server": { "type": "string", "description": "MCP server name" }
                        },
                        "required": ["server"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "mcp_status",
                    "description": "List all configured MCP servers, their status, and their discovered tools.",
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "security_scan",
                    "description": "Run the built-in static security scanner (SAST, secret detection, unsafe patterns) across the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {}
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "schedule_timer",
                    "description": "Schedule a background countdown timer (seconds) that runs a command or task when the timer expires.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "seconds": { "type": "integer", "description": "Duration in seconds before the timer fires" },
                            "action": { "type": "string", "description": "'command' (run shell command) or 'prompt'" },
                            "prompt_or_command": { "type": "string", "description": "Shell command (prefix with !) or task to run when timer fires" }
                        },
                        "required": ["seconds", "prompt_or_command"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "start_watcher",
                    "description": "Start a live background file or command watcher that monitors workspace changes at a recurring interval.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "watch_target": { "type": "string", "description": "Relative path/glob to watch (e.g. 'src', '.rs') or 'cmd:<command>' to watch command output" },
                            "interval_seconds": { "type": "integer", "description": "Polling interval in seconds (default 5)" },
                            "on_change_command": { "type": "string", "description": "Optional command (prefix with !) to execute automatically when a change is detected" }
                        },
                        "required": ["watch_target"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "manage_background_tasks",
                    "description": "List or cancel active background timers, file/command watchers, and cron schedules.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "description": "'list' or 'cancel'" },
                            "task_id": { "type": "string", "description": "Task ID to cancel (or 'all')" }
                        },
                        "required": ["action"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "call_mcp_tool",
                    "description": "Invoke a tool exposed by a configured MCP server via JSON-RPC 2.0 `tools/call`.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "server": { "type": "string", "description": "MCP server name" },
                            "tool": { "type": "string", "description": "Tool name on the MCP server" },
                            "arguments": { "type": "object", "description": "JSON arguments object for the tool" }
                        },
                        "required": ["server", "tool"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "delegate_subagent",
                    "description": "Delegate a specialized coding, architecture, or security sub-task to an autonomous subagent.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "subagent_id": { "type": "string", "description": "Subagent ID or name (e.g. 'sub-architect', 'sub-coder', 'sub-security')" },
                            "task": { "type": "string", "description": "Clear, actionable task instructions for the subagent" }
                        },
                        "required": ["subagent_id", "task"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "delegate_parallel_subagents",
                    "description": "Fan out up to 4 subagent tasks concurrently in parallel and collect their combined outputs and diffs.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "tasks": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "subagent_id": { "type": "string", "description": "Subagent ID or role" },
                                        "task": { "type": "string", "description": "Task instructions" }
                                    },
                                    "required": ["subagent_id", "task"]
                                }
                            }
                        },
                        "required": ["tasks"]
                    }
                }
            }
        ]);

        if let Some(arr) = base.as_array_mut() {
            for srv in McpManager::load() {
                if !srv.enabled {
                    continue;
                }
                let safe_srv: String = srv
                    .name
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                    .collect();
                for t in &srv.cached_tools {
                    let safe_tl: String = t
                        .name
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                        .collect();
                    let full_name = format!("mcp__{}__{}", safe_srv, safe_tl);
                    let params = if t.input_schema.is_object()
                        && t.input_schema.get("type").is_some()
                    {
                        t.input_schema.clone()
                    } else {
                        serde_json::json!({
                            "type": "object",
                            "properties": {}
                        })
                    };
                    arr.push(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": full_name,
                            "description": format!("[MCP: {}] {}", srv.name, t.description),
                            "parameters": params
                        }
                    }));
                }
            }
        }

        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_edit_delete_and_revert_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        let mut engine = ToolEngine::new(ws.clone());

        let diff1 = engine
            .write_file("src/hello.rs", "fn main() {\n    println!(\"hi\");\n}\n", true)
            .unwrap();
        assert_eq!(diff1.status, "created");
        assert_eq!(diff1.additions, 3);

        let diff2 = engine
            .edit_file("src/hello.rs", "println!(\"hi\");", "println!(\"hello rust\");")
            .unwrap();
        assert_eq!(diff2.status, "modified");
        assert_eq!(diff2.additions, 1);
        assert_eq!(diff2.deletions, 1);
        assert!(engine
            .read_file("src/hello.rs", None, None)
            .unwrap()
            .contains("hello rust"));

        ToolEngine::revert_diff(ws.root(), &diff2).unwrap();
        assert!(engine
            .read_file("src/hello.rs", None, None)
            .unwrap()
            .contains("println!(\"hi\");"));

        // Test brace expansion glob `*.{rs,toml}` and `edit_file_ext` ambiguous line numbers + `replace_all: true`
        engine
            .write_file("Cargo.toml", "[package]\nname = \"demo\"\n", true)
            .unwrap();
        let glob_hits = engine.find_by_name("*.{rs,toml}").unwrap();
        assert_eq!(glob_hits.len(), 2);

        engine
            .write_file("src/dup.rs", "let x = 1;\nlet y = 2;\nlet x = 1;\n", true)
            .unwrap();
        let ambig_err = engine.edit_file("src/dup.rs", "let x = 1;", "let x = 9;").unwrap_err();
        assert!(ambig_err.to_string().contains("at lines 1, 3"));
        let rep_all_diff = engine
            .edit_file_ext("src/dup.rs", "let x = 1;", "let x = 9;", true)
            .unwrap();
        assert_eq!(rep_all_diff.additions, 2);

        // Test UTF-8 safe `truncate_head_tail` with multi-byte emojis and CJK characters
        let emoji_str = "🦀🔥🚀日本語".repeat(50);
        let truncated = ToolEngine::truncate_head_tail(&emoji_str, 40);
        assert!(truncated.contains("[truncated"));

        let del_diff = engine.delete_file("src/hello.rs").unwrap();
        assert_eq!(del_diff.status, "deleted");
        assert!(!ws.root().join("src/hello.rs").exists());

        ToolEngine::revert_diff(ws.root(), &del_diff).unwrap();
        assert!(ws.root().join("src/hello.rs").exists());

        let (_, ok, _) = engine.execute_tool(
            "grep_search",
            &serde_json::json!({ "query": "println", "path": "./src" }),
        );
        assert!(ok);
    }

    #[test]
    fn test_ssrf_protection_blocks_internal_hosts() {
        assert!(ToolEngine::validate_url_against_ssrf("http://127.0.0.1:9758/api/health").is_err());
        assert!(ToolEngine::validate_url_against_ssrf("http://localhost:11434/api/tags").is_err());
        assert!(ToolEngine::validate_url_against_ssrf("http://[::1]:9758/api/health").is_err());
        assert!(ToolEngine::validate_url_against_ssrf("http://[::ffff:169.254.169.254]/latest/meta-data/").is_err());
        assert!(ToolEngine::validate_url_against_ssrf("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(ToolEngine::validate_url_against_ssrf("file:///C:/Windows/win.ini").is_err());
    }
}
