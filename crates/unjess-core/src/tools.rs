//! Sandboxed Core Tools & Diff Engine implemented in Rust.
//!
//! Provides memory-safe file operations strictly confined to the workspace root,
//! LCS-based unified diff calculation and 1-click revert, regex grep search, glob matching,
//! timeout-enforced PowerShell/POSIX command execution, SSRF-protected URL fetching,
//! DuckDuckGo web search, AST/code-intelligence tools, MCP tool invocation, and
//! JSON schema definitions for LLM tool calling.

use crate::codeintel::{
    debug_trace_command, edit_symbol, extract_imports, find_callers, find_symbol_definitions,
    generate_repo_map, git_archeology, outline_file, rename_symbol_across_repo,
    run_post_edit_check, run_workspace_diagnostics,
};
use crate::config::Settings;
use crate::mcp::{McpManager, McpServerConfig};
use crate::memory::MemoryStore;
use crate::skills::SkillsManager;
use crate::workspace::{Workspace, WorkspaceError};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("File edit failed: target string was not found in '{0}'{1}")]
    TargetNotFound(String, String),
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

/// Structured checklist item managed via the `todo_write` Frontier tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String, // "pending" | "in_progress" | "completed" | "cancelled"
    #[serde(default)]
    pub priority: String, // "high" | "medium" | "low"
}

/// User/agent synthesized custom tool definition stored in `~/.unjess-rust/custom_tools/tools.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesizedToolDef {
    pub name: String,
    pub description: String,
    pub script_lang: String,
    pub script_path: String,
    pub parameters: serde_json::Value,
}

/// Extracts negative file modification constraints from a user prompt
/// (e.g. "Do not modify `/app/run_pretrain.sh`", "Never touch `config.yaml`", "leave `test_eval.py` unchanged").
pub fn extract_protected_paths_from_prompt(prompt: &str) -> Vec<String> {
    let mut protected = Vec::new();
    let split_re =
        Regex::new(r"(?i)(?:\r?\n|;|!|\.\s+|\b(?:but|however|instead)\b)").ok();
    let clauses: Vec<&str> = match split_re {
        Some(ref sre) => sre.split(prompt).collect(),
        None => vec![prompt],
    };

    let patterns = [
        r"(?i)(?:do\s+not|don't|never|must\s+not|cannot|can't)\s+(?:modify|edit|change|touch|alter|overwrite|delete|remove)\s+(.+)",
        r"(?i)without\s+(?:modifying|editing|changing|touching|altering)\s+(.+)",
        r"(?i)(?:leave|keep)\s+(.+?)\s+(?:unchanged|untouched|as-is|as\s+is|intact)",
    ];
    let token_re = Regex::new(r#"[`"']([^`"'\s]+)[`"']|([A-Za-z0-9_./\\-]+\.[A-Za-z0-9_]+)"#).ok();

    for sentence in clauses {
        for pat in patterns {
            if let Ok(re) = Regex::new(pat) {
                for cap in re.captures_iter(sentence) {
                    let Some(clause) = cap.get(1).map(|m| m.as_str()) else {
                        continue;
                    };
                    if let Some(ref tre) = token_re {
                        for tcap in tre.captures_iter(clause) {
                            let raw = tcap
                                .get(1)
                                .or_else(|| tcap.get(2))
                                .map(|m| m.as_str())
                                .unwrap_or("");
                            let cleaned = raw
                                .trim_matches(|c: char| {
                                    matches!(
                                        c,
                                        '`' | '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '.'
                                    )
                                })
                                .replace('\\', "/");
                            if cleaned.len() >= 3
                                && (cleaned.contains('.') || cleaned.contains('/'))
                                && !cleaned.ends_with('.')
                                && !protected
                                    .iter()
                                    .any(|p: &String| p.eq_ignore_ascii_case(&cleaned))
                            {
                                protected.push(cleaned);
                            }
                        }
                    }
                }
            }
        }
    }
    protected
}

#[derive(Clone)]
pub struct ToolEngine {
    workspace: Workspace,
    abort_flag: Option<Arc<AtomicBool>>,
    session_cwd: Arc<Mutex<PathBuf>>,
    session_env: Arc<Mutex<HashMap<String, String>>>,
    protected_paths: Arc<Mutex<Vec<String>>>,
    read_files: Arc<Mutex<HashMap<String, u64>>>,
    todo_items: Arc<Mutex<Vec<TodoItem>>>,
    pending_vision_images: Arc<Mutex<Vec<String>>>,
    vision_enabled: bool,
}

impl ToolEngine {
    pub fn new(workspace: Workspace) -> Self {
        let initial_cwd = workspace.root().to_path_buf();
        Self {
            workspace,
            abort_flag: None,
            session_cwd: Arc::new(Mutex::new(initial_cwd)),
            session_env: Arc::new(Mutex::new(HashMap::new())),
            protected_paths: Arc::new(Mutex::new(Vec::new())),
            read_files: Arc::new(Mutex::new(HashMap::new())),
            todo_items: Arc::new(Mutex::new(Vec::new())),
            pending_vision_images: Arc::new(Mutex::new(Vec::new())),
            vision_enabled: true,
        }
    }

    /// Declares whether the active model can actually perceive images.
    pub fn with_vision(mut self, enabled: bool) -> Self {
        self.vision_enabled = enabled;
        self
    }

    pub fn with_abort_flag(mut self, abort_flag: Arc<AtomicBool>) -> Self {
        self.abort_flag = Some(abort_flag);
        self
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Returns the current persistent working directory of the shell session.
    pub fn session_cwd(&self) -> PathBuf {
        self.session_cwd
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| self.workspace.root().to_path_buf())
    }

    /// Returns a snapshot of persistent environment variables set during the shell session.
    pub fn session_env(&self) -> HashMap<String, String> {
        self.session_env
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Configures paths that must not be modified or deleted during this turn.
    pub fn set_protected_paths(&self, paths: Vec<String>) {
        if let Ok(mut guard) = self.protected_paths.lock() {
            *guard = paths;
        }
    }

    /// Returns the list of protected file paths.
    pub fn protected_paths(&self) -> Vec<String> {
        self.protected_paths
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Returns the current structured todo checklist items.
    pub fn todo_items(&self) -> Vec<TodoItem> {
        self.todo_items
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Queues a base64 encoded image to be attached to the multimodal vision turn.
    pub fn queue_vision_image(&self, b64: String) {
        if let Ok(mut guard) = self.pending_vision_images.lock() {
            guard.push(b64);
        }
    }

    /// Drains and clears all queued base64 vision images.
    pub fn drain_vision_images(&self) -> Vec<String> {
        if let Ok(mut guard) = self.pending_vision_images.lock() {
            let imgs = guard.clone();
            guard.clear();
            imgs
        } else {
            Vec::new()
        }
    }

    /// Returns whether the file path has a supported image extension.
    pub fn is_supported_image_extension(path: &str) -> bool {
        let p = path.to_lowercase();
        p.ends_with(".png")
            || p.ends_with(".jpg")
            || p.ends_with(".jpeg")
            || p.ends_with(".webp")
            || p.ends_with(".gif")
            || p.ends_with(".bmp")
    }

    /// Encodes arbitrary byte slices to standard RFC 4648 Base64 string.
    pub fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(triple & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    /// Optimizes and downscales an image buffer for multimodal LLM vision consumption.
    /// Resizes if dimensions exceed 1568px, and compresses to efficient JPEG (or PNG if has alpha channel)
    /// to prevent huge Base64 strings from blowing Ollama/OpenAI/Claude HTTP payload limits.
    /// If image cannot be decoded (e.g. mock test bytes or unsupported format), falls back to raw bytes.
    pub fn optimize_image_for_vision(raw_bytes: &[u8], original_ext: &str) -> (Vec<u8>, String) {
        let default_mime = match original_ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        };

        // Try decoding with the image crate
        let img = match image::load_from_memory(raw_bytes) {
            Ok(img) => img,
            Err(_) => {
                // If decoding fails (e.g. mock test bytes), fallback to raw bytes directly
                return (raw_bytes.to_vec(), default_mime.to_string());
            }
        };

        let (width, height) = (img.width(), img.height());
        const MAX_DIMENSION: u32 = 1568;

        let resized_img = if width > MAX_DIMENSION || height > MAX_DIMENSION {
            img.resize(MAX_DIMENSION, MAX_DIMENSION, image::imageops::FilterType::Triangle)
        } else {
            img
        };

        let mut output_bytes = Vec::new();
        let mime;

        let has_alpha = resized_img.color().has_alpha();
        let use_png = has_alpha && original_ext.eq_ignore_ascii_case("png");

        if use_png {
            mime = "image/png".to_string();
            let mut cursor = std::io::Cursor::new(&mut output_bytes);
            if resized_img.write_to(&mut cursor, image::ImageFormat::Png).is_err() {
                return (raw_bytes.to_vec(), default_mime.to_string());
            }
        } else {
            mime = "image/jpeg".to_string();
            let rgb_img = resized_img.to_rgb8();
            let mut cursor = std::io::Cursor::new(&mut output_bytes);
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 82);
            if encoder.encode(
                rgb_img.as_raw(),
                rgb_img.width(),
                rgb_img.height(),
                image::ExtendedColorType::Rgb8,
            ).is_err() {
                return (raw_bytes.to_vec(), default_mime.to_string());
            }
        }

        if !output_bytes.is_empty() && output_bytes.len() < raw_bytes.len() {
            (output_bytes, mime)
        } else if !output_bytes.is_empty() && (width > MAX_DIMENSION || height > MAX_DIMENSION) {
            (output_bytes, mime)
        } else {
            (raw_bytes.to_vec(), default_mime.to_string())
        }
    }

    /// Loads an image file from disk, base64 encodes it, queues it for multimodal vision input,
    /// and returns a descriptive summary.
    pub fn view_image(&self, rel_path: &str) -> Result<String, ToolError> {
        let abs_path = self.resolve_skill_or_workspace_path(rel_path, false)?;
        if !abs_path.exists() {
            return Err(ToolError::Execution(format!("File not found: {}", rel_path)));
        }
        let bytes = fs::read(&abs_path)?;
        if bytes.is_empty() {
            return Err(ToolError::Execution(format!("Image file '{}' is empty (0 bytes)", rel_path)));
        }
        let ext = abs_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("png")
            .to_lowercase();
        let (opt_bytes, mime) = Self::optimize_image_for_vision(&bytes, &ext);
        let b64 = Self::base64_encode(&opt_bytes);
        if !self.vision_enabled {
            return Ok(format!(
                "[NO VISION] The active model cannot perceive images, so '{}' was NOT shown to you ({} KB). Do not pretend to have seen it and do not try to reverse-engineer its content with coordinate math or pixel scripts. Tell the user plainly that this model cannot see images and ask them to switch to a vision-capable model (Gemini, Claude, GPT-5, Kimi, Qwen-VL) or to describe what is in the image.",
                rel_path.replace('\\', "/"),
                opt_bytes.len() / 1024
            ));
        }
        self.queue_vision_image(b64);
        let orig_kb = bytes.len() / 1024;
        let opt_kb = opt_bytes.len() / 1024;
        let norm_path = rel_path.replace('\\', "/");
        if opt_kb < orig_kb {
            Ok(format!(
                "Image file loaded and attached to your Vision input ({}, {} KB, optimized from {} KB): {} — you can now see and inspect this image directly.",
                mime, opt_kb, orig_kb, norm_path
            ))
        } else {
            Ok(format!(
                "Image file loaded and attached to your Vision input ({}, {} KB): {} — you can now see and inspect this image directly.",
                mime, opt_kb, norm_path
            ))
        }
    }

    /// Checks whether `candidate_path` matches any protected path constraint.
    pub fn check_protected_path(&self, candidate_path: &str) -> Result<(), ToolError> {
        let norm_cand = candidate_path
            .trim()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_lowercase();
        let cand_base = Path::new(&norm_cand)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        for prot in self.protected_paths() {
            let norm_prot = prot
                .trim()
                .replace('\\', "/")
                .trim_start_matches("./")
                .to_lowercase();
            let prot_base = Path::new(&norm_prot)
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();

            let matched = norm_cand == norm_prot
                || norm_cand.ends_with(&format!("/{}", norm_prot.trim_start_matches('/')))
                || norm_prot.ends_with(&format!("/{}", norm_cand.trim_start_matches('/')))
                || (!prot_base.is_empty() && ! norm_prot.contains('/') && cand_base == prot_base);

            if matched {
                return Err(ToolError::Execution(format!(
                    "Protected file constraint: '{}' is marked as DO NOT MODIFY by the user prompt.",
                    prot
                )));
            }
        }
        Ok(())
    }

    /// Enforces the Granular Tool Policy Matrix on sensitive credential/secret files
    /// (`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `credentials.json`).
    pub fn check_sensitive_file_policy(&self, candidate_path: &str) -> Result<(), ToolError> {
        let norm = candidate_path
            .trim()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_lowercase();
        let base = Path::new(&norm)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        // Allow `.env.example` / `.env.sample` / `.env.template`
        if base.ends_with(".example") || base.ends_with(".sample") || base.ends_with(".template") {
            return Ok(());
        }

        let is_sensitive_secret = base == ".env"
            || base.starts_with(".env.")
            || base == "id_rsa"
            || base == "id_ed25519"
            || base.ends_with(".pem")
            || base.ends_with(".key")
            || base == "credentials.json"
            || base == ".npmrc"
            || base == ".pypirc";

        if is_sensitive_secret {
            // Only block if the file already exists on disk (preventing accidental secret destruction)
            if let Ok(abs) = self.workspace.resolve(candidate_path) {
                if abs.exists() {
                    return Err(ToolError::Execution(format!(
                        "Tool Policy Guard: Modifying or deleting existing sensitive credential file '{}' is blocked by default to protect secrets.",
                        candidate_path
                    )));
                }
            }
        }
        Ok(())
    }

    /// Enforces the Granular Command Policy Matrix on high-impact destructive or external-publish commands.
    pub fn check_command_policy_matrix(&self, cmd: &str) -> Result<(), ToolError> {
        let lower = cmd.trim().to_lowercase();
        let high_risk_patterns = [
            ("git push --force", "Force-pushing to remote repositories (`git push --force`) is blocked by the Tool Policy Matrix."),
            ("git push -f", "Force-pushing to remote repositories (`git push -f`) is blocked by the Tool Policy Matrix."),
            ("git reset --hard", "Destructive working-tree reset (`git reset --hard`) is blocked by the Tool Policy Matrix; use `edit_file` or `git checkout -- <file>` on specific files."),
            ("git clean -fd", "Destructive untracked file wipe (`git clean -fd`) is blocked by the Tool Policy Matrix; use `delete_file` on specific scratch files."),
            ("npm publish", "Publishing packages (`npm publish`) is blocked by the Tool Policy Matrix without manual user execution."),
            ("cargo publish", "Publishing crates (`cargo publish`) is blocked by the Tool Policy Matrix without manual user execution."),
        ];
        for (pat, reason) in high_risk_patterns {
            if lower.contains(pat) {
                return Err(ToolError::Execution(format!("Tool Policy Guard: {}", reason)));
            }
        }
        Ok(())
    }

    fn hash_content(content: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    fn record_file_snapshot(&self, rel_path: &str, content: &str) {
        let norm = rel_path.replace('\\', "/").trim_start_matches("./").to_string();
        if let Ok(mut guard) = self.read_files.lock() {
            guard.insert(norm, Self::hash_content(content));
        }
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
            // 1. Direct absolute path anywhere on disk
            let candidate = PathBuf::from(trimmed);
            if candidate.is_absolute() && candidate.exists() {
                return Ok(candidate);
            }

            let mut skill_roots = vec![
                SkillsManager::skills_dir(),
                Settings::legacy_go_config_dir().join("skills"),
                self.workspace.root().join(".agents").join("skills"),
                self.workspace.root().join(".unjess").join("skills"),
            ];
            if let Some(home) = dirs::home_dir() {
                skill_roots.push(home.join(".gemini").join("config").join("skills"));
                skill_roots.push(home.join(".gemini").join("config").join("plugins"));
                skill_roots.push(home.join(".gemini").join("antigravity").join("builtin").join("skills"));
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
        if Self::is_supported_image_extension(rel_path) {
            return self.view_image(rel_path);
        }
        let abs_path = self.resolve_skill_or_workspace_path(rel_path, false)?;
        let content = fs::read_to_string(&abs_path)?;
        self.record_file_snapshot(rel_path, &content);
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
        self.check_protected_path(rel_path)?;
        self.check_sensitive_file_policy(rel_path)?;
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
        self.record_file_snapshot(rel_path, content);

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
        self.check_protected_path(rel_path)?;
        self.check_sensitive_file_policy(rel_path)?;
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
    /// Supports exact matching first, then falls back to normalized line-ending (`\r\n` vs `\n`),
    /// `read_file` line-number prefix stripping (`123: `), and smart-indentation trimmed line matching.
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
        self.check_protected_path(rel_path)?;
        self.check_sensitive_file_policy(rel_path)?;
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
        self.record_file_snapshot(rel_path, &new_content);
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
        self.check_protected_path(rel_path)?;
        self.check_sensitive_file_policy(rel_path)?;
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
        self.record_file_snapshot(rel_path, &working);
        Ok(Self::compute_diff(
            rel_path,
            "modified",
            &original_content,
            &working,
        ))
    }

    /// Applies a multi-file or single-file patch supporting both structured patch headers
    /// (`*** Begin Patch`, `*** Add File:`, `*** Update File:`, `*** Delete File:`, `*** Move to:`)
    /// and standard unified diff headers (`--- a/...`, `+++ b/...`, `@@ ... @@`).
    pub fn apply_patch(
        &self,
        patch_text: &str,
        default_path: Option<&str>,
    ) -> Result<(String, Vec<FileDiff>), ToolError> {
        let norm_patch = patch_text.replace("\r\n", "\n");
        let lines: Vec<&str> = norm_patch.lines().collect();
        if lines.is_empty() {
            return Err(ToolError::Execution("Patch content cannot be empty".to_string()));
        }

        #[derive(Debug)]
        enum PatchOp {
            Add { path: String, content_lines: Vec<String> },
            Delete { path: String },
            Update {
                path: String,
                move_to: Option<String>,
                hunks: Vec<(Vec<String>, Vec<String>)>, // (old_block_lines, new_block_lines)
            },
        }

        let mut ops: Vec<PatchOp> = Vec::new();
        let mut idx = 0usize;

        let parse_hunks = |slice: &[&str], cursor: &mut usize| -> Vec<(Vec<String>, Vec<String>)> {
            let mut hunks = Vec::new();
            let mut cur_old: Vec<String> = Vec::new();
            let mut cur_new: Vec<String> = Vec::new();
            let mut in_hunk = false;

            while *cursor < slice.len() {
                let l = slice[*cursor];
                if l.starts_with("*** Add File:")
                    || l.starts_with("*** Update File:")
                    || l.starts_with("*** Delete File:")
                    || l.starts_with("*** End Patch")
                    || l.starts_with("--- a/")
                    || l.starts_with("diff --git ")
                {
                    break;
                }
                if l.starts_with("@@") {
                    if in_hunk && (!cur_old.is_empty() || !cur_new.is_empty()) {
                        hunks.push((std::mem::take(&mut cur_old), std::mem::take(&mut cur_new)));
                    }
                    in_hunk = true;
                    *cursor += 1;
                    continue;
                }
                if l.starts_with("\\ No newline") {
                    *cursor += 1;
                    continue;
                }
                if let Some(added) = l.strip_prefix('+') {
                    in_hunk = true;
                    cur_new.push(added.to_string());
                } else if let Some(removed) = l.strip_prefix('-') {
                    in_hunk = true;
                    cur_old.push(removed.to_string());
                } else if let Some(ctx) = l.strip_prefix(' ') {
                    if in_hunk {
                        cur_old.push(ctx.to_string());
                        cur_new.push(ctx.to_string());
                    }
                } else if in_hunk && l.trim().is_empty() {
                    cur_old.push(String::new());
                    cur_new.push(String::new());
                }
                *cursor += 1;
            }
            if in_hunk && (!cur_old.is_empty() || !cur_new.is_empty()) {
                hunks.push((cur_old, cur_new));
            }
            hunks
        };

        while idx < lines.len() {
            let line = lines[idx].trim_end();
            if line.starts_with("*** Begin Patch")
                || line.starts_with("*** End Patch")
                || line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.trim().is_empty()
            {
                idx += 1;
                continue;
            }
            if let Some(p) = line.strip_prefix("*** Add File:") {
                let path = p.trim().to_string();
                idx += 1;
                let mut content_lines = Vec::new();
                while idx < lines.len() {
                    let nl = lines[idx];
                    if nl.starts_with("*** ") || nl.starts_with("--- a/") || nl.starts_with("diff --git ") {
                        break;
                    }
                    if let Some(stripped) = nl.strip_prefix('+') {
                        content_lines.push(stripped.to_string());
                    } else {
                        content_lines.push(nl.to_string());
                    }
                    idx += 1;
                }
                ops.push(PatchOp::Add { path, content_lines });
                continue;
            }
            if let Some(p) = line.strip_prefix("*** Delete File:") {
                let path = p.trim().to_string();
                idx += 1;
                ops.push(PatchOp::Delete { path });
                continue;
            }
            if let Some(p) = line.strip_prefix("*** Update File:") {
                let path = p.trim().to_string();
                idx += 1;
                let mut move_to = None;
                if idx < lines.len() {
                    if let Some(mv) = lines[idx].trim_end().strip_prefix("*** Move to:") {
                        move_to = Some(mv.trim().to_string());
                        idx += 1;
                    }
                }
                let hunks = parse_hunks(&lines, &mut idx);
                ops.push(PatchOp::Update { path, move_to, hunks });
                continue;
            }
            if line.starts_with("--- ") {
                let old_hdr = line.trim_start_matches("--- ").trim();
                let old_path = old_hdr.strip_prefix("a/").unwrap_or(old_hdr).trim();
                idx += 1;
                let mut new_path = old_path.to_string();
                if idx < lines.len() && lines[idx].starts_with("+++ ") {
                    let new_hdr = lines[idx].trim_start_matches("+++ ").trim();
                    new_path = new_hdr.strip_prefix("b/").unwrap_or(new_hdr).trim().to_string();
                    idx += 1;
                }
                let hunks = parse_hunks(&lines, &mut idx);
                if old_path == "/dev/null" {
                    let mut content_lines = Vec::new();
                    for (_, new_b) in hunks {
                        content_lines.extend(new_b);
                    }
                    ops.push(PatchOp::Add {
                        path: new_path,
                        content_lines,
                    });
                } else if new_path == "/dev/null" {
                    ops.push(PatchOp::Delete {
                        path: old_path.to_string(),
                    });
                } else {
                    let move_to = if !new_path.is_empty() && new_path != old_path {
                        Some(new_path)
                    } else {
                        None
                    };
                    ops.push(PatchOp::Update {
                        path: old_path.to_string(),
                        move_to,
                        hunks,
                    });
                }
                continue;
            }
            if line.starts_with("@@") {
                if let Some(def_p) = default_path.filter(|s| !s.trim().is_empty()) {
                    let hunks = parse_hunks(&lines, &mut idx);
                    ops.push(PatchOp::Update {
                        path: def_p.trim().to_string(),
                        move_to: None,
                        hunks,
                    });
                    continue;
                }
            }
            idx += 1;
        }

        if ops.is_empty() {
            return Err(ToolError::Execution(
                "No valid patch operations (`*** Add/Update/Delete File:` or unified `--- a/ +++ b/ @@` hunks) found in patch.".to_string(),
            ));
        }

        let mut diffs = Vec::new();
        let mut summary_lines = Vec::new();

        for op in ops {
            match op {
                PatchOp::Add { path, content_lines } => {
                    let content = content_lines.join("\n");
                    let d = self.write_file(&path, &content, true)?;
                    let suffix = self.format_post_edit_suffix(&path);
                    summary_lines.push(format!("+ Added `{}` ({} lines){}", path, content_lines.len(), suffix));
                    diffs.push(d);
                }
                PatchOp::Delete { path } => {
                    let d = self.delete_file(&path)?;
                    summary_lines.push(format!("- Deleted `{}`", path));
                    diffs.push(d);
                }
                PatchOp::Update { path, move_to, hunks } => {
                    self.check_protected_path(&path)?;
                    self.check_sensitive_file_policy(&path)?;
                    let abs_path = self.workspace.resolve(&path)?;
                    let orig_content = fs::read_to_string(&abs_path)?;
                    let mut working = orig_content.clone();
                    for (old_b, new_b) in &hunks {
                        let target = old_b.join("\n");
                        let replacement = new_b.join("\n");
                        if target.trim().is_empty() {
                            if !working.is_empty() && !working.ends_with('\n') {
                                working.push('\n');
                            }
                            working.push_str(&replacement);
                        } else {
                            working = Self::apply_replacement(&path, &working, &target, &replacement, false)?;
                        }
                    }
                    let final_rel = if let Some(ref dest) = move_to {
                        self.check_protected_path(dest)?;
                        self.check_sensitive_file_policy(dest)?;
                        let dest_abs = self.workspace.resolve(dest)?;
                        if let Some(p) = dest_abs.parent() {
                            fs::create_dir_all(p)?;
                        }
                        fs::write(&dest_abs, &working)?;
                        let _ = fs::remove_file(&abs_path);
                        dest.clone()
                    } else {
                        fs::write(&abs_path, &working)?;
                        path.clone()
                    };
                    self.record_file_snapshot(&final_rel, &working);
                    let d = Self::compute_diff(&final_rel, "modified", &orig_content, &working);
                    let suffix = self.format_post_edit_suffix(&final_rel);
                    summary_lines.push(format!(
                        "~ Updated `{}` ({} hunk(s), +{} -{}){}",
                        final_rel,
                        hunks.len(),
                        d.additions,
                        d.deletions,
                        suffix
                    ));
                    diffs.push(d);
                }
            }
        }

        Ok((summary_lines.join("\n"), diffs))
    }

    fn match_line_numbers(content: &str, needle: &str) -> String {
        let mut lines = Vec::new();
        for (byte_idx, _) in content.match_indices(needle) {
            let line_no = content[..byte_idx].bytes().filter(|&b| b == b'\n').count() + 1;
            lines.push(line_no.to_string());
        }
        lines.join(", ")
    }

    /// Strips accidental `read_file` line prefixes (`  12: code`) if >= 50% of non-empty lines have them.
    pub fn strip_read_file_line_prefixes(text: &str) -> Option<String> {
        let prefix_re = Regex::new(r"^\s*\d+:\s?(.*)$").ok()?;
        let lines: Vec<&str> = text.lines().collect();
        let non_empty: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        if non_empty.is_empty() {
            return None;
        }
        let prefixed_count = non_empty.iter().filter(|l| prefix_re.is_match(l)).count();
        if prefixed_count * 2 < non_empty.len() {
            return None;
        }
        let stripped: Vec<String> = lines
            .into_iter()
            .map(|l| {
                if let Some(cap) = prefix_re.captures(l) {
                    cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default()
                } else {
                    l.to_string()
                }
            })
            .collect();
        let joined = stripped.join("\n");
        if joined == text {
            None
        } else {
            Some(joined)
        }
    }

    fn apply_replacement(
        rel_path: &str,
        original_content: &str,
        target: &str,
        replacement: &str,
        replace_all: bool,
    ) -> Result<String, ToolError> {
        if target.is_empty() {
            return Err(ToolError::TargetNotFound(
                rel_path.to_string(),
                String::new(),
            ));
        }

        if let Some(res) =
            Self::try_apply_replacement_core(rel_path, original_content, target, replacement, replace_all)?
        {
            return Ok(res);
        }

        // Fallback: if the LLM accidentally copied `123: ` line number prefixes from `read_file`, strip them
        if let Some(stripped_target) = Self::strip_read_file_line_prefixes(target) {
            let stripped_replacement = Self::strip_read_file_line_prefixes(replacement)
                .unwrap_or_else(|| replacement.to_string());
            if let Some(res) = Self::try_apply_replacement_core(
                rel_path,
                original_content,
                &stripped_target,
                &stripped_replacement,
                replace_all,
            )? {
                return Ok(res);
            }
        }

        let preview = Self::find_closest_match_preview(original_content, target);
        Err(ToolError::TargetNotFound(rel_path.to_string(), preview))
    }

    fn try_apply_replacement_core(
        rel_path: &str,
        original_content: &str,
        target: &str,
        replacement: &str,
        replace_all: bool,
    ) -> Result<Option<String>, ToolError> {
        let exact_matches = original_content.matches(target).count();
        if exact_matches == 1 {
            return Ok(Some(original_content.replacen(target, replacement, 1)));
        } else if exact_matches > 1 {
            if replace_all {
                return Ok(Some(original_content.replace(target, replacement)));
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
            return Ok(Some(if original_content.contains("\r\n") {
                replaced.replace('\n', "\r\n")
            } else {
                replaced
            }));
        } else if norm_matches > 1 {
            let line_nums = Self::match_line_numbers(&norm_content, &norm_target);
            return Err(ToolError::AmbiguousMatch(
                norm_matches,
                line_nums,
                rel_path.to_string(),
            ));
        }

        Ok(Self::flexible_line_replace(
            original_content,
            target,
            replacement,
        ))
    }

    fn flexible_line_replace(content: &str, target: &str, replacement: &str) -> Option<String> {
        let content_lines: Vec<&str> = content.lines().collect();
        let target_raw_lines: Vec<&str> = target.lines().collect();
        let target_lines: Vec<&str> = target_raw_lines.iter().map(|l| l.trim()).collect();
        if target_lines.is_empty() || target_lines.len() > content_lines.len() {
            return None;
        }
        let is_crlf = content.contains("\r\n");

        let splice_window = |idx: usize, win_len: usize, rep_str: &str| -> String {
            let first_non_empty_offset = (0..target_raw_lines.len())
                .find(|&o| !target_raw_lines[o].trim().is_empty())
                .unwrap_or(0);
            let file_anchor_line =
                content_lines[(idx + first_non_empty_offset).min(content_lines.len() - 1)];
            let target_anchor_line = target_raw_lines[first_non_empty_offset];

            let file_indent: String = file_anchor_line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let target_indent: String = target_anchor_line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();

            let shifted_replacement =
                Self::shift_replacement_indentation(rep_str, &target_indent, &file_indent);

            let mut out_lines: Vec<String> = content_lines[..idx]
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            for r_line in shifted_replacement.lines() {
                out_lines.push(r_line.to_string());
            }
            out_lines.extend(
                content_lines[idx + win_len..]
                    .iter()
                    .map(|s| (*s).to_string()),
            );
            let sep = if is_crlf { "\r\n" } else { "\n" };
            out_lines.join(sep)
        };

        // Stage 3: Line-trimmed exact match
        let mut match_idx = None;
        let mut ambiguous = false;
        for start in 0..=(content_lines.len() - target_lines.len()) {
            let window_matches = (0..target_lines.len())
                .all(|offset| content_lines[start + offset].trim() == target_lines[offset]);
            if window_matches {
                if match_idx.is_some() {
                    ambiguous = true;
                    break;
                }
                match_idx = Some(start);
            }
        }
        if !ambiguous {
            if let Some(idx) = match_idx {
                return Some(splice_window(idx, target_lines.len(), replacement));
            }
        }

        // Stage 4: Internal whitespace-normalized line match (collapses runs of spaces/tabs)
        let norm_ws = |s: &str| -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") };
        let target_ws: Vec<String> = target_lines.iter().map(|l| norm_ws(l)).collect();
        let mut ws_idx = None;
        let mut ws_ambiguous = false;
        for start in 0..=(content_lines.len() - target_ws.len()) {
            let ok = (0..target_ws.len())
                .all(|o| norm_ws(content_lines[start + o]) == target_ws[o]);
            if ok {
                if ws_idx.is_some() {
                    ws_ambiguous = true;
                    break;
                }
                ws_idx = Some(start);
            }
        }
        if !ws_ambiguous {
            if let Some(idx) = ws_idx {
                return Some(splice_window(idx, target_ws.len(), replacement));
            }
        }

        // Stage 5: Blank-line tolerant match (matches non-empty trimmed lines across optional blank lines)
        let target_non_empty: Vec<&str> = target_lines
            .iter()
            .copied()
            .filter(|l| !l.is_empty())
            .collect();
        if !target_non_empty.is_empty() {
            let non_empty_positions: Vec<(usize, &str)> = content_lines
                .iter()
                .enumerate()
                .map(|(i, l)| (i, l.trim()))
                .filter(|(_, l)| !l.is_empty())
                .collect();
            if non_empty_positions.len() >= target_non_empty.len() {
                let mut blank_match: Option<(usize, usize)> = None;
                let mut blank_ambig = false;
                for n_start in 0..=(non_empty_positions.len() - target_non_empty.len()) {
                    let ok = (0..target_non_empty.len()).all(|o| {
                        non_empty_positions[n_start + o].1 == target_non_empty[o]
                    });
                    if ok {
                        let first_line_idx = non_empty_positions[n_start].0;
                        let last_line_idx =
                            non_empty_positions[n_start + target_non_empty.len() - 1].0;
                        let span_len = last_line_idx - first_line_idx + 1;
                        if span_len <= target_lines.len() + 6 {
                            if blank_match.is_some() {
                                blank_ambig = true;
                                break;
                            }
                            blank_match = Some((first_line_idx, span_len));
                        }
                    }
                }
                if !blank_ambig {
                    if let Some((idx, span_len)) = blank_match {
                        return Some(splice_window(idx, span_len, replacement));
                    }
                }
            }
        }

        // Stage 6: Escape-normalized match (handles over-escaped `\"` or `\\n` in tool JSON)
        if target.contains("\\\"") || target.contains("\\n") || target.contains("\\t") {
            let unescaped_target = target
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\t", "\t");
            let unescaped_rep = replacement
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\t", "\t");
            if unescaped_target != target {
                if let Some(res) =
                    Self::flexible_line_replace(content, &unescaped_target, &unescaped_rep)
                {
                    return Some(res);
                }
            }
        }

        // Stage 7: Context-trimmed diff hunk prefix stripper (if target lines start with `+`/`-`/` `)
        let looks_like_diff = target_raw_lines.len() >= 2
            && target_raw_lines
                .iter()
                .all(|l| l.is_empty() || l.starts_with(' ') || l.starts_with('-'));
        if looks_like_diff {
            let stripped_t: Vec<String> = target_raw_lines
                .iter()
                .map(|l| {
                    l.strip_prefix('-')
                        .or_else(|| l.strip_prefix(' '))
                        .unwrap_or(l)
                        .to_string()
                })
                .collect();
            let joined_t = stripped_t.join("\n");
            if joined_t != target {
                if let Some(res) = Self::flexible_line_replace(content, &joined_t, replacement) {
                    return Some(res);
                }
            }
        }

        // Helper: line similarity in [0.0, 1.0] using character bigrams (Dice coefficient)
        let line_similarity = |a: &str, b: &str| -> f64 {
            let ta = a.trim();
            let tb = b.trim();
            if ta == tb {
                return 1.0;
            }
            if ta.is_empty() || tb.is_empty() {
                return 0.0;
            }
            let ca: Vec<char> = ta.chars().collect();
            let cb: Vec<char> = tb.chars().collect();
            if ca.len() < 2 || cb.len() < 2 {
                return if ca == cb { 1.0 } else { 0.0 };
            }
            let mut bigrams_a: Vec<(char, char)> = ca.windows(2).map(|w| (w[0], w[1])).collect();
            let mut bigrams_b: Vec<(char, char)> = cb.windows(2).map(|w| (w[0], w[1])).collect();
            bigrams_a.sort_unstable();
            bigrams_b.sort_unstable();
            let (mut i, mut j, mut matches) = (0usize, 0usize, 0usize);
            while i < bigrams_a.len() && j < bigrams_b.len() {
                if bigrams_a[i] == bigrams_b[j] {
                    matches += 1;
                    i += 1;
                    j += 1;
                } else if bigrams_a[i] < bigrams_b[j] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
            (2.0 * matches as f64) / ((bigrams_a.len() + bigrams_b.len()) as f64)
        };

        // Stage 8: Block-anchor match (matches first & last non-empty anchor lines + interior similarity >= 0.70)
        if target_lines.len() >= 3 {
            let first_ne = target_lines.iter().position(|l| !l.is_empty());
            let last_ne = target_lines.iter().rposition(|l| !l.is_empty());
            if let (Some(f_off), Some(l_off)) = (first_ne, last_ne) {
                if l_off > f_off && target_lines[f_off].len() >= 4 && target_lines[l_off].len() >= 2
                {
                    let anchor_span = l_off - f_off + 1;
                    let mut anchor_cand: Option<(usize, f64)> = None;
                    let mut anchor_ambig = false;
                    for start in 0..=(content_lines.len() - target_lines.len()) {
                        if content_lines[start + f_off].trim() == target_lines[f_off]
                            && content_lines[start + l_off].trim() == target_lines[l_off]
                        {
                            let mut sum_sim = 0.0;
                            for o in f_off..=l_off {
                                sum_sim += line_similarity(content_lines[start + o], target_lines[o]);
                            }
                            let avg_sim = sum_sim / (anchor_span as f64);
                            if avg_sim >= 0.70 {
                                if anchor_cand.is_some() {
                                    anchor_ambig = true;
                                    break;
                                }
                                anchor_cand = Some((start, avg_sim));
                            }
                        }
                    }
                    if !anchor_ambig {
                        if let Some((idx, _)) = anchor_cand {
                            return Some(splice_window(idx, target_lines.len(), replacement));
                        }
                    }
                }
            }
        }

        // Stage 9: High-threshold fuzzy similarity match (average line similarity >= 0.92, unique best window)
        if target_lines.len() >= 2 {
            let mut best_start = None;
            let mut best_sim = 0.0f64;
            let mut second_best = 0.0f64;
            for start in 0..=(content_lines.len() - target_lines.len()) {
                if line_similarity(content_lines[start], target_lines[0]) < 0.75 {
                    continue;
                }
                let mut sum = 0.0;
                for o in 0..target_lines.len() {
                    sum += line_similarity(content_lines[start + o], target_lines[o]);
                }
                let avg = sum / (target_lines.len() as f64);
                if avg > best_sim {
                    second_best = best_sim;
                    best_sim = avg;
                    best_start = Some(start);
                } else if avg > second_best {
                    second_best = avg;
                }
            }
            if best_sim >= 0.92 && (best_sim - second_best) >= 0.05 {
                if let Some(idx) = best_start {
                    return Some(splice_window(idx, target_lines.len(), replacement));
                }
            }
        }

        None
    }

    /// Shifts the indentation of `replacement` lines from `target_indent` to `file_indent`
    /// while preserving relative indentation of nested lines.
    pub fn shift_replacement_indentation(
        replacement: &str,
        target_indent: &str,
        file_indent: &str,
    ) -> String {
        if target_indent == file_indent {
            return replacement.to_string();
        }
        let shifted: Vec<String> = replacement
            .lines()
            .map(|line| {
                if line.trim().is_empty() {
                    return String::new();
                }
                if !target_indent.is_empty() && line.starts_with(target_indent) {
                    format!("{}{}", file_indent, &line[target_indent.len()..])
                } else if target_indent.is_empty() {
                    format!("{}{}", file_indent, line)
                } else {
                    let line_ws_len = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                    let strip_len = line_ws_len.min(target_indent.len());
                    format!("{}{}", file_indent, &line[strip_len..])
                }
            })
            .collect();
        shifted.join("\n")
    }

    /// Finds the closest matching lines in `content` when `target` is not found,
    /// returning a numbered snippet so the LLM can immediately retry with the exact lines.
    pub fn find_closest_match_preview(content: &str, target: &str) -> String {
        let content_lines: Vec<&str> = content.lines().collect();
        if content_lines.is_empty() {
            return String::new();
        }
        let target_non_empty: Vec<&str> = target
            .lines()
            .map(|l| l.trim())
            .filter(|l| l.len() >= 3)
            .collect();
        if target_non_empty.is_empty() {
            return String::new();
        }

        let anchor = target_non_empty[0].to_lowercase();
        let anchor_tokens: Vec<&str> = anchor
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 2)
            .collect();

        let mut best_line_idx = None;
        let mut best_score = 0.0f64;

        for (idx, &line) in content_lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_lowercase();
            let mut score = 0.0;
            if lower == anchor {
                score = 1.0;
            } else if lower.contains(&anchor) || anchor.contains(&lower) {
                let min_l = lower.len().min(anchor.len()) as f64;
                let max_l = lower.len().max(anchor.len()) as f64;
                score = 0.65 + 0.30 * (min_l / max_l.max(1.0));
            } else if !anchor_tokens.is_empty() {
                let matched_toks = anchor_tokens
                    .iter()
                    .filter(|&&tok| lower.contains(tok))
                    .count();
                score = (matched_toks as f64) / (anchor_tokens.len() as f64) * 0.75;
            }

            if score > best_score {
                best_score = score;
                best_line_idx = Some(idx);
            }
        }

        let Some(center) = best_line_idx.filter(|_| best_score >= 0.35) else {
            return String::new();
        };

        let start = center.saturating_sub(2);
        let end = (center + target.lines().count() + 3).min(content_lines.len());
        let mut snippet = Vec::new();
        snippet.push(format!(
            "\n\nClosest match in file (lines {}..={}):",
            start + 1,
            end
        ));
        for i in start..end {
            snippet.push(format!("{:>4}: {}", i + 1, content_lines[i]));
        }
        snippet.join("\n")
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

    /// Checks whether a shell command attempts to modify or delete any protected path.
    fn check_shell_command_protected_paths(&self, cmd: &str) -> Result<(), ToolError> {
        let protected = self.protected_paths();
        if protected.is_empty() {
            return Ok(());
        }
        let lower_cmd = cmd.to_lowercase();
        let has_mutating_op = lower_cmd.contains('>')
            || lower_cmd.contains("sed -i")
            || lower_cmd.contains("rm ")
            || lower_cmd.contains("del ")
            || lower_cmd.contains("mv ")
            || lower_cmd.contains("cp ")
            || lower_cmd.contains("remove-item")
            || lower_cmd.contains("set-content")
            || lower_cmd.contains("out-file")
            || lower_cmd.contains("add-content")
            || lower_cmd.contains("move-item")
            || lower_cmd.contains("copy-item")
            || lower_cmd.contains("truncate ")
            || lower_cmd.contains("tee ");

        if !has_mutating_op {
            return Ok(());
        }

        let norm_cmd = lower_cmd.replace('\\', "/");
        for prot in protected {
            let norm_prot = prot
                .trim()
                .replace('\\', "/")
                .trim_start_matches("./")
                .to_lowercase();
            let base_prot = Path::new(&norm_prot)
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if (!norm_prot.is_empty() && norm_cmd.contains(&norm_prot))
                || (!base_prot.is_empty() && base_prot.contains('.') && norm_cmd.contains(&base_prot))
            {
                return Err(ToolError::Execution(format!(
                    "Protected file constraint: shell command attempts to modify protected file '{}', which the user explicitly forbade modifying.",
                    prot
                )));
            }
        }
        Ok(())
    }

    /// Translates unquoted `&&` operators into PowerShell 5.1-compatible conditional chains
    /// (`; if ($? -and ($null -eq $LASTEXITCODE -or $LASTEXITCODE -eq 0)) { ... }`) while leaving
    /// `&&` inside single or double quotes untouched.
    pub fn translate_powershell_and_and(cmd: &str) -> String {
        let chars: Vec<char> = cmd.chars().collect();
        let mut segments: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];
            if in_single {
                current.push(c);
                if c == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        current.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    in_single = false;
                }
                i += 1;
            } else if in_double {
                current.push(c);
                if c == '`' && i + 1 < chars.len() {
                    current.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if c == '"' {
                    in_double = false;
                }
                i += 1;
            } else {
                if c == '\'' {
                    in_single = true;
                    current.push(c);
                    i += 1;
                } else if c == '"' {
                    in_double = true;
                    current.push(c);
                    i += 1;
                } else if c == '&'
                    && i + 1 < chars.len()
                    && chars[i + 1] == '&'
                    && (i + 2 >= chars.len() || chars[i + 2] != '&')
                {
                    segments.push(current.trim().to_string());
                    current.clear();
                    i += 2;
                } else {
                    current.push(c);
                    i += 1;
                }
            }
        }
        segments.push(current.trim().to_string());

        let non_empty: Vec<String> = segments.into_iter().filter(|s| !s.is_empty()).collect();
        if non_empty.len() <= 1 {
            return cmd.to_string();
        }

        let mut acc = non_empty.last().cloned().unwrap_or_default();
        for seg in non_empty[..non_empty.len() - 1].iter().rev() {
            acc = format!(
                "$LASTEXITCODE = 0; {}; if ($? -and ($null -eq $LASTEXITCODE -or $LASTEXITCODE -eq 0)) {{ {} }}",
                seg, acc
            );
        }
        acc
    }

    /// Encodes a PowerShell script as UTF-16LE Base64 for `-EncodedCommand`.
    pub fn encode_powershell_utf16le_base64(script: &str) -> String {
        let mut bytes = Vec::with_capacity(script.len() * 2);
        for unit in script.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(triple & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    /// Extracts `__UNJESS_STATE_PWD__` and `__UNJESS_STATE_ENV_BEGIN__` sentinels from stdout,
    /// updates persistent `session_cwd` and `session_env`, and returns cleaned stdout.
    fn extract_and_strip_shell_state(&self, raw_stdout: &str) -> String {
        const PWD_MARKER: &str = "__UNJESS_STATE_PWD__";
        const ENV_BEGIN: &str = "__UNJESS_STATE_ENV_BEGIN__";
        const ENV_END: &str = "__UNJESS_STATE_ENV_END__";

        let Some(pwd_pos) = raw_stdout.rfind(PWD_MARKER) else {
            return raw_stdout.to_string();
        };

        let user_stdout = raw_stdout[..pwd_pos].trim_end_matches(['\r', '\n']);
        let after_pwd = &raw_stdout[pwd_pos + PWD_MARKER.len()..];

        if let Some(env_begin_rel) = after_pwd.find(ENV_BEGIN) {
            let pwd_str = after_pwd[..env_begin_rel].trim();
            if !pwd_str.is_empty() {
                let candidate = PathBuf::from(pwd_str);
                if candidate.is_dir() {
                    if let Ok(mut cwd_guard) = self.session_cwd.lock() {
                        *cwd_guard = candidate;
                    }
                }
            }
            let after_env_begin = &after_pwd[env_begin_rel + ENV_BEGIN.len()..];
            let env_block = if let Some(env_end_rel) = after_env_begin.find(ENV_END) {
                &after_env_begin[..env_end_rel]
            } else {
                after_env_begin
            };

            if let Ok(mut env_guard) = self.session_env.lock() {
                for line in env_block.lines() {
                    let trimmed = line.trim();
                    if let Some((k, v)) = trimmed.split_once('=') {
                        let key = k.trim();
                        if !key.is_empty()
                            && !matches!(
                                key,
                                "PWD" | "OLDPWD" | "_" | "SHLVL" | "__UNJESS_EC" | "PSExecutionPolicyPreference"
                            )
                        {
                            env_guard.insert(key.to_string(), v.to_string());
                        }
                    }
                }
            }
        } else {
            let pwd_str = after_pwd.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
            if !pwd_str.is_empty() {
                let candidate = PathBuf::from(pwd_str);
                if candidate.is_dir() {
                    if let Ok(mut cwd_guard) = self.session_cwd.lock() {
                        *cwd_guard = candidate;
                    }
                }
            }
        }

        user_stdout.to_string()
    }

    /// Analyzes shell command errors for common syntax/platform pitfalls, returns an actionable
    /// `[SYNTAX AUTO-HINT]` string, and persists the learned hint to `~/.unjess-rust/memory/error_hints.json`.
    pub fn analyze_syntax_error_and_learn(cmd: &str, output: &str, exit_code: i32) -> Option<String> {
        let lower_out = output.to_lowercase();
        let lower_cmd = cmd.to_lowercase();

        let hint = if lower_out.contains("is not a valid statement separator") {
            Some("[SYNTAX AUTO-HINT]: PowerShell 5.1 does not support raw '&&'. Separate commands with ';' or rely on automatic chaining.")
        } else if (lower_cmd.contains("curl ") || lower_cmd.starts_with("curl"))
            && (lower_out.contains("invoke-webrequest")
                || lower_out.contains("parameterbindingexception")
                || lower_out.contains("positional parameter cannot be found"))
        {
            Some("[SYNTAX AUTO-HINT]: In Windows PowerShell, 'curl' is an alias for Invoke-WebRequest. Use 'curl.exe' explicitly when passing standard curl flags (-s, -X, -H, -d).")
        } else if lower_out.contains("remove-item")
            && (lower_out.contains("cannot find a parameter with the name 'rf'")
                || lower_out.contains("cannot find a parameter with the name 'fr'"))
        {
            Some("[SYNTAX AUTO-HINT]: In Windows PowerShell, 'rm -rf' fails because 'rm' aliases Remove-Item. Use 'Remove-Item -Recurse -Force <path>' or the 'delete_file' tool.")
        } else if lower_out.contains("missingendcurlybrace")
            || lower_out.contains("unexpectedtoken")
            || lower_out.contains("parsererror")
            || lower_out.contains("syntax error near unexpected token")
            || lower_out.contains("unterminated quoted string")
        {
            Some("[SYNTAX AUTO-HINT]: Shell parser/quoting error detected. Avoid deeply nested quotes in inline one-liners; prefer writing a script file with write_file or using native tools (read_file, edit_file, grep_search).")
        } else if exit_code != 0
            && (lower_cmd.starts_with("sed ") || lower_cmd.contains(" sed "))
            && lower_out.contains("is not recognized as the name of a cmdlet")
        {
            Some("[SYNTAX AUTO-HINT]: 'sed' is not available in Windows PowerShell. Use the 'edit_file' or 'multi_edit_file' tool instead.")
        } else {
            None
        }?;

        // Persist to ~/.unjess-rust/memory/error_hints.json
        let hints_dir = Settings::config_dir().join("memory");
        let _ = fs::create_dir_all(&hints_dir);
        let hints_file = hints_dir.join("error_hints.json");
        let mut stored: Vec<String> = fs::read_to_string(&hints_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if !stored.iter().any(|h| h == hint) {
            stored.push(hint.to_string());
            if stored.len() > 50 {
                let drain_count = stored.len() - 50;
                stored.drain(0..drain_count);
            }
            if let Ok(json) = serde_json::to_string_pretty(&stored) {
                let _ = fs::write(&hints_file, json);
            }
        }

        Some(hint.to_string())
    }

    /// Loads previously learned shell error hints from `~/.unjess-rust/memory/error_hints.json`.
    pub fn load_learned_error_hints() -> Vec<String> {
        let hints_file = Settings::config_dir().join("memory").join("error_hints.json");
        fs::read_to_string(hints_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Executes a shell command in the persistent session working directory with real timeout and abort enforcement,
    /// capping stdout/stderr pipes to 2 MiB and killing the child process tree if it exceeds the deadline.
    pub fn run_command(&self, cmd: &str, timeout_secs: u64) -> Result<String, ToolError> {
        self.run_command_with_code(cmd, timeout_secs)
            .map(|(out, _)| out)
    }

    /// Executes a shell command in the persistent session working directory and returns `(truncated_output, exit_code)`.
    pub fn run_command_with_code(
        &self,
        cmd: &str,
        timeout_secs: u64,
    ) -> Result<(String, i32), ToolError> {
        self.check_shell_command_protected_paths(cmd)?;
        self.check_command_policy_matrix(cmd)?;

        let effective_timeout = Duration::from_secs(timeout_secs.clamp(1, 3600));
        const MAX_PIPE_BYTES: u64 = 2 * 1024 * 1024;

        let mut active_cwd = self.session_cwd();
        if !active_cwd.is_dir() {
            active_cwd = self.workspace.root().to_path_buf();
            if let Ok(mut guard) = self.session_cwd.lock() {
                *guard = active_cwd.clone();
            }
        }
        let active_env = self.session_env();

        #[cfg(target_os = "windows")]
        let mut child = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let translated_cmd = Self::translate_powershell_and_and(cmd);
            let wrapped_ps = format!(
                "$LASTEXITCODE = 0\n{}\n$__unjess_ec = if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ $LASTEXITCODE }} elseif (-not $?) {{ 1 }} else {{ 0 }}\nWrite-Output \"\"\nWrite-Output \"__UNJESS_STATE_PWD__\"\nWrite-Output (Get-Location).Path\nWrite-Output \"__UNJESS_STATE_ENV_BEGIN__\"\nGet-ChildItem Env: | ForEach-Object {{ Write-Output \"$($_.Name)=$($_.Value)\" }}\nWrite-Output \"__UNJESS_STATE_ENV_END__\"\nexit $__unjess_ec",
                translated_cmd
            );
            let encoded = Self::encode_powershell_utf16le_base64(&wrapped_ps);
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-WindowStyle",
                    "Hidden",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-EncodedCommand",
                    &encoded,
                ])
                .current_dir(&active_cwd)
                .envs(&active_env)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()?
        };
        #[cfg(not(target_os = "windows"))]
        let mut child = {
            let wrapped_sh = format!(
                "{}\n__unjess_ec=$?\nprintf \"\\n__UNJESS_STATE_PWD__\\n\"\npwd\nprintf \"__UNJESS_STATE_ENV_BEGIN__\\n\"\nenv\nprintf \"__UNJESS_STATE_ENV_END__\\n\"\nexit $__unjess_ec",
                cmd
            );
            Command::new("sh")
                .args(["-c", &wrapped_sh])
                .current_dir(&active_cwd)
                .envs(&active_env)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?
        };

        #[allow(unused_variables)]
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
        let raw_stdout = String::from_utf8_lossy(&stdout_bytes);
        let stdout = self.extract_and_strip_shell_state(&raw_stdout);
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

        if let Some(auto_hint) = Self::analyze_syntax_error_and_learn(cmd, &combined, exit_code) {
            combined.push_str("\n\n");
            combined.push_str(&auto_hint);
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

    fn format_post_edit_suffix(&self, rel_path: &str) -> String {
        let check = run_post_edit_check(self.workspace.root(), rel_path);
        if check.is_empty() {
            String::new()
        } else {
            format!("\n{}", check)
        }
    }

    fn custom_tools_dir() -> PathBuf {
        let dir = Settings::config_dir().join("custom_tools");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn load_synthesized_tools() -> Vec<SynthesizedToolDef> {
        let manifest = Self::custom_tools_dir().join("tools.json");
        fs::read_to_string(manifest)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_synthesized_tools(tools: &[SynthesizedToolDef]) -> Result<(), String> {
        let manifest = Self::custom_tools_dir().join("tools.json");
        let json = serde_json::to_string_pretty(tools).map_err(|e| e.to_string())?;
        fs::write(manifest, json).map_err(|e| e.to_string())
    }

    fn shadow_sandbox_dir(&self) -> PathBuf {
        let root_str = self.workspace.root().to_string_lossy();
        let hash = Self::hash_content(&root_str);
        std::env::temp_dir().join(format!("unjess-shadow-{:x}", hash))
    }

    fn copy_workspace_to_shadow(src: &Path, dst: &Path) -> Result<usize, String> {
        fs::create_dir_all(dst).map_err(|e| e.to_string())?;
        let mut count = 0usize;
        let entries = fs::read_dir(src).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if matches!(
                file_name.as_str(),
                ".git" | "target" | "node_modules" | ".venv" | "__pycache__" | "dist" | "build"
            ) {
                continue;
            }
            let src_path = entry.path();
            let dst_path = dst.join(&file_name);
            if src_path.is_dir() {
                count += Self::copy_workspace_to_shadow(&src_path, &dst_path)?;
            } else if src_path.is_file() {
                if let Ok(meta) = src_path.metadata() {
                    if meta.len() <= 10 * 1024 * 1024 {
                        let _ = fs::copy(&src_path, &dst_path);
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    }

    fn execute_shadow_sandbox(&self, args: &serde_json::Value) -> (String, bool, Option<FileDiff>) {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("status")
            .trim()
            .to_lowercase();
        let shadow_dir = self.shadow_sandbox_dir();

        match action.as_str() {
            "create" => {
                if shadow_dir.exists() {
                    let _ = fs::remove_dir_all(&shadow_dir);
                }
                match Self::copy_workspace_to_shadow(self.workspace.root(), &shadow_dir) {
                    Ok(files) => (
                        format!(
                            "Created Copy-on-Write shadow sandbox at '{}' ({} files snapshotted).",
                            shadow_dir.display(),
                            files
                        ),
                        true,
                        None,
                    ),
                    Err(e) => (format!("Error creating shadow sandbox: {}", e), false, None),
                }
            }
            "diff" | "status" => {
                if !shadow_dir.exists() {
                    return (
                        "No active shadow sandbox. Call shadow_sandbox with action: 'create' first."
                            .to_string(),
                        false,
                        None,
                    );
                }
                let Ok(shadow_ws) = Workspace::new(&shadow_dir) else {
                    return ("Failed to open shadow workspace".to_string(), false, None);
                };
                let shadow_files = shadow_ws.list_all_files(2000);
                let mut diffs = Vec::new();
                for rel in shadow_files {
                    let s_path = shadow_dir.join(&rel);
                    let w_path = self.workspace.root().join(&rel);
                    let s_txt = fs::read_to_string(&s_path).unwrap_or_default();
                    let w_txt = fs::read_to_string(&w_path).unwrap_or_default();
                    if s_txt != w_txt {
                        let d = Self::compute_diff(&rel, if w_path.exists() { "modified" } else { "created" }, &w_txt, &s_txt);
                        diffs.push(format!(
                            "### {} ({}, +{} -{} lines)\n{}",
                            d.path, d.status, d.additions, d.deletions, d.diff
                        ));
                    }
                }
                if diffs.is_empty() {
                    ("Shadow sandbox matches main workspace (0 modified files).".to_string(), true, None)
                } else {
                    (diffs.join("\n\n"), true, None)
                }
            }
            "promote" => {
                if !shadow_dir.exists() {
                    return ("No active shadow sandbox to promote.".to_string(), false, None);
                }
                let Ok(shadow_ws) = Workspace::new(&shadow_dir) else {
                    return ("Failed to open shadow workspace".to_string(), false, None);
                };
                let shadow_files = shadow_ws.list_all_files(2000);
                let mut promoted = 0usize;
                for rel in shadow_files {
                    if self.check_protected_path(&rel).is_err() {
                        continue;
                    }
                    let s_path = shadow_dir.join(&rel);
                    let w_path = self.workspace.root().join(&rel);
                    let s_txt = fs::read_to_string(&s_path).unwrap_or_default();
                    let w_txt = fs::read_to_string(&w_path).unwrap_or_default();
                    if s_txt != w_txt {
                        if let Some(p) = w_path.parent() {
                            let _ = fs::create_dir_all(p);
                        }
                        if fs::write(&w_path, &s_txt).is_ok() {
                            promoted += 1;
                        }
                    }
                }
                (
                    format!("Promoted {} modified/created file(s) from shadow sandbox into main workspace.", promoted),
                    true,
                    None,
                )
            }
            "discard" => {
                if shadow_dir.exists() {
                    let _ = fs::remove_dir_all(&shadow_dir);
                    ("Discarded and removed shadow sandbox.".to_string(), true, None)
                } else {
                    ("Shadow sandbox was already clean/non-existent.".to_string(), true, None)
                }
            }
            "revert_hunk" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.trim().is_empty() {
                    return ("Error: 'path' is required for revert_hunk".to_string(), false, None);
                }
                if shadow_dir.exists() {
                    let s_path = shadow_dir.join(path.trim());
                    let w_path = self.workspace.root().join(path.trim());
                    if s_path.is_file() {
                        if let Ok(orig) = fs::read_to_string(&s_path) {
                            let _ = fs::write(&w_path, orig);
                            return (
                                format!("Reverted '{}' from shadow sandbox snapshot.", path),
                                true,
                                None,
                            );
                        }
                    }
                }
                (
                    format!("No shadow snapshot found for '{}'", path),
                    false,
                    None,
                )
            }
            other => (
                format!(
                    "Unknown shadow_sandbox action '{}' (expected 'create', 'diff', 'promote', 'discard', or 'revert_hunk')",
                    other
                ),
                false,
                None,
            ),
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
            "view_image" | "view_images" | "visual_qa" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        let p2 = get_str("file_path");
                        if p2.is_empty() {
                            let p3 = get_str("image");
                            if p3.is_empty() {
                                get_str("target")
                            } else {
                                p3
                            }
                        } else {
                            p2
                        }
                    } else {
                        p
                    }
                };
                if path.is_empty() {
                    (
                        "Error: 'path' parameter is required for view_image.".to_string(),
                        false,
                        None,
                    )
                } else {
                    match self.view_image(&path) {
                        Ok(msg) => (msg, true, None),
                        Err(e) => (format!("Error: {}", e), false, None),
                    }
                }
            }
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
                    Ok(diff) => {
                        let verify_suffix = self.format_post_edit_suffix(&diff.path);
                        (
                            format!(
                                "Successfully wrote '{}' (+{} -{} lines){}",
                                diff.path, diff.additions, diff.deletions, verify_suffix
                            ),
                            true,
                            Some(diff),
                        )
                    }
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
                    Ok(diff) => {
                        let verify_suffix = self.format_post_edit_suffix(&diff.path);
                        (
                            format!(
                                "Successfully edited '{}' (+{} -{} lines){}",
                                diff.path, diff.additions, diff.deletions, verify_suffix
                            ),
                            true,
                            Some(diff),
                        )
                    }
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
                    Ok(diff) => {
                        let verify_suffix = self.format_post_edit_suffix(&diff.path);
                        (
                            format!(
                                "Successfully applied {} edits to '{}' (+{} -{} lines){}",
                                edits_vec.len(),
                                diff.path,
                                diff.additions,
                                diff.deletions,
                                verify_suffix
                            ),
                            true,
                            Some(diff),
                        )
                    }
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
            "todo_write" => {
                let items_val = args.get("todos").or_else(|| args.get("items"));
                let mut parsed_items: Vec<TodoItem> = Vec::new();
                if let Some(arr) = items_val.and_then(|v| v.as_array()) {
                    for (idx, item) in arr.iter().enumerate() {
                        let id = item
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("{}", idx + 1));
                        let content = item
                            .get("content")
                            .or_else(|| item.get("task"))
                            .or_else(|| item.get("title"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if content.trim().is_empty() {
                            continue;
                        }
                        let status = item
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pending")
                            .to_string();
                        let priority = item
                            .get("priority")
                            .and_then(|v| v.as_str())
                            .unwrap_or("medium")
                            .to_string();
                        parsed_items.push(TodoItem {
                            id,
                            content,
                            status,
                            priority,
                        });
                    }
                }
                if let Ok(mut guard) = self.todo_items.lock() {
                    *guard = parsed_items.clone();
                }
                let completed = parsed_items
                    .iter()
                    .filter(|t| t.status == "completed" || t.status == "done")
                    .count();
                let mut summary = vec![format!(
                    "Updated Todo Checklist ({}/{} completed):",
                    completed,
                    parsed_items.len()
                )];
                for t in &parsed_items {
                    let mark = match t.status.as_str() {
                        "completed" | "done" => "[x]",
                        "in_progress" | "active" => "[/]",
                        "cancelled" => "[-]",
                        _ => "[ ]",
                    };
                    summary.push(format!("  {} #{}: {} ({})", mark, t.id, t.content, t.status));
                }
                (summary.join("\n"), true, None)
            }
            "ast_edit_symbol" => {
                let path = {
                    let p = get_str("path");
                    if p.is_empty() {
                        get_str("file_path")
                    } else {
                        p
                    }
                };
                let symbol = {
                    let s = get_str("symbol");
                    if s.is_empty() {
                        get_str("symbol_name")
                    } else {
                        s
                    }
                };
                let new_code = {
                    let c = get_str("new_code");
                    if c.is_empty() {
                        get_str("replacement")
                    } else {
                        c
                    }
                };
                if let Err(e) = self.check_protected_path(&path) {
                    return (format!("Error: {}", e), false, None);
                }
                match edit_symbol(self.workspace.root(), &path, &symbol, &new_code) {
                    Ok((old_content, new_content, callers_report)) => {
                        self.record_file_snapshot(&path, &new_content);
                        let diff =
                            Self::compute_diff(&path, "modified", &old_content, &new_content);
                        let verify_suffix = self.format_post_edit_suffix(&path);
                        let msg = format!(
                            "Successfully updated symbol `{}` in `{}`.{}\n\n### Cross-Repo Callers of `{}`:\n{}",
                            symbol, path, verify_suffix, symbol, callers_report
                        );
                        (msg, true, Some(diff))
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "ast_rename_symbol" => {
                let old_name = get_str("old_name");
                let new_name = get_str("new_name");
                if !self.protected_paths().is_empty() {
                    return (
                        "Error: ast_rename_symbol is disabled when protected file constraints are active; use edit_file on non-protected files instead.".to_string(),
                        false,
                        None,
                    );
                }
                match rename_symbol_across_repo(self.workspace.root(), &old_name, &new_name) {
                    Ok((files, count)) => {
                        let file_list = if files.is_empty() {
                            "None".to_string()
                        } else {
                            files.join(", ")
                        };
                        (
                            format!(
                                "Renamed `{}` -> `{}` ({} replacement(s) across {} file(s): {})",
                                old_name,
                                new_name,
                                count,
                                files.len(),
                                file_list
                            ),
                            true,
                            None,
                        )
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "git_archeology" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let query = args
                    .get("query")
                    .or_else(|| args.get("symbol"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let start_line = args
                    .get("start_line")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(0);
                let end_line = args
                    .get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(0);
                let out = git_archeology(self.workspace.root(), path, query, start_line, end_line);
                (Self::truncate_head_tail(&out, 12000), true, None)
            }
            "debug_trace" => {
                let command = {
                    let c = get_str("command");
                    if c.is_empty() {
                        get_str("cmd")
                    } else {
                        c
                    }
                };
                let out = debug_trace_command(self.workspace.root(), &command);
                (Self::truncate_head_tail(&out, 12000), true, None)
            }
            "apply_patch" | "patch" => {
                let patch_text = {
                    let p = get_str("patch");
                    if p.is_empty() {
                        get_str("diff")
                    } else {
                        p
                    }
                };
                let default_path = args.get("path").and_then(|v| v.as_str());
                match self.apply_patch(&patch_text, default_path) {
                    Ok((summary, mut diffs)) => {
                        let first_diff = diffs.drain(..).next();
                        (summary, true, first_diff)
                    }
                    Err(e) => (format!("Error: {}", e), false, None),
                }
            }
            "lsp_diagnostics" | "workspace_diagnostics" => {
                let filter_file = args
                    .get("path")
                    .or_else(|| args.get("file"))
                    .and_then(|v| v.as_str());
                let out = run_workspace_diagnostics(self.workspace.root(), filter_file);
                (Self::truncate_head_tail(&out, 12000), true, None)
            }
            "synthesize_tool" => {
                let tool_name = get_str("name");
                let description = get_str("description");
                let script_lang = {
                    let l = get_str("script_lang");
                    if l.is_empty() {
                        "python".to_string()
                    } else {
                        l.to_lowercase()
                    }
                };
                let script_code = {
                    let c = get_str("script_code");
                    if c.is_empty() {
                        get_str("code")
                    } else {
                        c
                    }
                };
                let safe_name: String = tool_name
                    .trim()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
                    .collect();
                if safe_name.is_empty() || script_code.trim().is_empty() {
                    return (
                        "Error: synthesize_tool requires non-empty 'name' and 'script_code'"
                            .to_string(),
                        false,
                        None,
                    );
                }
                let ext = match script_lang.as_str() {
                    "powershell" | "ps1" => "ps1",
                    "node" | "js" | "javascript" => "js",
                    "bash" | "sh" => "sh",
                    _ => "py",
                };
                let script_path = Self::custom_tools_dir().join(format!("{}.{}", safe_name, ext));
                if let Err(e) = fs::write(&script_path, &script_code) {
                    return (
                        format!("Error writing synthesized tool script: {}", e),
                        false,
                        None,
                    );
                }
                let params = args
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
                let mut tools = Self::load_synthesized_tools();
                tools.retain(|t| !t.name.eq_ignore_ascii_case(&safe_name));
                tools.push(SynthesizedToolDef {
                    name: safe_name.clone(),
                    description: if description.is_empty() {
                        format!("Synthesized {} tool", script_lang)
                    } else {
                        description
                    },
                    script_lang,
                    script_path: script_path.to_string_lossy().to_string(),
                    parameters: params,
                });
                let _ = Self::save_synthesized_tools(&tools);
                (
                    format!(
                        "Synthesized and registered custom tool '{}' at '{}'. You can now invoke '{}' directly.",
                        safe_name,
                        script_path.display(),
                        safe_name
                    ),
                    true,
                    None,
                )
            }
            "shadow_sandbox" => self.execute_shadow_sandbox(args),
            "call_mcp_tool" => {
                let server_name = args.get("server")
                    .or_else(|| args.get("ServerName"))
                    .or_else(|| args.get("server_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_name = args.get("tool")
                    .or_else(|| args.get("ToolName"))
                    .or_else(|| args.get("tool_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_args = args.get("arguments")
                    .or_else(|| args.get("Arguments"))
                    .or_else(|| args.get("args"))
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
            other if other.starts_with("mcp_") => {
                let rest = &other[4..];
                let servers = McpManager::load();
                let mut matched = false;
                let mut res = (String::new(), false, None);
                for srv in &servers {
                    let srv_safe = srv.name.replace('-', "_");
                    if rest.starts_with(&srv.name) || rest.starts_with(&srv_safe) {
                        let prefix_len = if rest.starts_with(&srv.name) { srv.name.len() } else { srv_safe.len() };
                        let tl = rest[prefix_len..].trim_start_matches('_');
                        match McpManager::call_tool(&srv.name, tl, args) {
                            Ok(val) => { res = (val, true, None); matched = true; break; }
                            Err(e) => { res = (format!("MCP tool error: {}", e), false, None); matched = true; break; }
                        }
                    }
                }
                if matched {
                    res
                } else {
                    (format!("Unknown MCP tool '{}'", other), false, None)
                }
            }
            other => {
                // 1. Direct MCP tool name match across all loaded and active MCP servers
                // (e.g. `generate_image`, `restyle_image`, `dmd_pipeline_generate_image`)
                let mcp_servers = McpManager::load();
                for srv in &mcp_servers {
                    if !srv.enabled {
                        continue;
                    }
                    if let Some(tool) = srv.cached_tools.iter().find(|t| t.name.eq_ignore_ascii_case(other)) {
                        match McpManager::call_tool(&srv.name, &tool.name, args) {
                            Ok(val) => return (val, true, None),
                            Err(e) => return (format!("MCP tool error: {}", e), false, None),
                        }
                    }
                }

                if let Some(synth) = Self::load_synthesized_tools()
                    .into_iter()
                    .find(|t| t.name.eq_ignore_ascii_case(other))
                {
                    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
                    if let Ok(mut env_guard) = self.session_env.lock() {
                        env_guard.insert("UNJESS_TOOL_ARGS".to_string(), args_json);
                    }
                    let cmd_line = match synth.script_lang.as_str() {
                        "powershell" | "ps1" => format!(
                            "powershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
                            synth.script_path
                        ),
                        "node" | "js" | "javascript" => format!("node \"{}\"", synth.script_path),
                        "bash" | "sh" => format!("sh \"{}\"", synth.script_path),
                        _ => format!("python \"{}\"", synth.script_path),
                    };
                    match self.run_command(&cmd_line, 120) {
                        Ok(out) => (out, true, None),
                        Err(e) => (format!("Synthesized tool error: {}", e), false, None),
                    }
                } else {
                    (format!("Unknown tool '{}'", other), false, None)
                }
            }
        }
    }

    /// Returns OpenAI-compatible JSON function tool schemas for LLM tool-calling,
    /// including Frontier tools, synthesized custom tools, and dynamically discovered MCP tools.
    pub fn tool_schemas() -> serde_json::Value {
        let mut base = serde_json::json!([
            {
                "type": "function",
                "function": {
                    "name": "view_image",
                    "description": "View and inspect an image file (.png, .jpg, .jpeg, .webp, .gif, .bmp) inside the workspace. Loads the image directly into your visual multimodal context so you can see it.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative or absolute path to the image file" }
                        },
                        "required": ["path"]
                    }
                }
            },
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
                    "name": "todo_write",
                    "description": "Update the structured task checklist to track multi-step engineering progress.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "todos": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "content": { "type": "string" },
                                        "status": { "type": "string", "description": "'pending', 'in_progress', 'completed', or 'cancelled'" },
                                        "priority": { "type": "string", "description": "'high', 'medium', or 'low'" }
                                    },
                                    "required": ["content", "status"]
                                }
                            }
                        },
                        "required": ["todos"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "ast_edit_symbol",
                    "description": "Surgically replace an entire AST symbol (function, struct, class, method) by name in a file without line-number drift.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Relative file path" },
                            "symbol": { "type": "string", "description": "Exact symbol name to replace" },
                            "new_code": { "type": "string", "description": "Complete replacement code for the symbol" }
                        },
                        "required": ["path", "symbol", "new_code"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "ast_rename_symbol",
                    "description": "Rename an identifier across all source files in the workspace using word-boundary AST token matching.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "old_name": { "type": "string", "description": "Existing identifier name" },
                            "new_name": { "type": "string", "description": "New identifier name" }
                        },
                        "required": ["old_name", "new_name"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "git_archeology",
                    "description": "Inspect Git blame, pickaxe (`-S`) symbol history, or recent commit log for a file or symbol.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Optional relative file path" },
                            "query": { "type": "string", "description": "Optional symbol or string for git log -S pickaxe search" },
                            "start_line": { "type": "integer", "description": "Optional start line for git blame" },
                            "end_line": { "type": "integer", "description": "Optional end line for git blame" }
                        }
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "debug_trace",
                    "description": "Execute a failing command or test with full stack/backtrace variables (`RUST_BACKTRACE=1`, `GOTRACEBACK=all`, `PYTHONFAULTHANDLER=1`) and automatically extract workspace source frames around the crash site.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string", "description": "Command or test to run and trace" }
                        },
                        "required": ["command"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "synthesize_tool",
                    "description": "Synthesize and hot-register a reusable custom tool backed by a Python, PowerShell, Node.js, or Bash script.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Tool identifier (e.g. 'parse_binary_header')" },
                            "description": { "type": "string", "description": "Description of what the tool does" },
                            "script_lang": { "type": "string", "description": "'python', 'powershell', 'node', or 'bash'" },
                            "script_code": { "type": "string", "description": "Executable script source code (reads JSON args from env UNJESS_TOOL_ARGS)" },
                            "parameters": { "type": "object", "description": "Optional JSON schema for tool parameters" }
                        },
                        "required": ["name", "description", "script_code"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "shadow_sandbox",
                    "description": "Manage a Copy-on-Write shadow workspace snapshot ('create', 'diff', 'promote', 'discard', 'revert_hunk') for safe speculative refactoring.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "description": "'create', 'diff', 'promote', 'discard', or 'revert_hunk'" },
                            "path": { "type": "string", "description": "Optional relative file path for 'revert_hunk'" }
                        },
                        "required": ["action"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "run_command",
                    "description": "Execute a shell command inside the stateful workspace session (persists working directory and environment variables across calls).",
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
                    "name": "apply_patch",
                    "description": "Apply a multi-file or single-file patch (`*** Begin Patch`, `*** Add File:`, `*** Update File:`, `*** Delete File:`, `*** Move to:` or unified `--- a/ +++ b/ @@ ... @@` diff hunks).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "patch": { "type": "string", "description": "Full patch or unified diff text" },
                            "path": { "type": "string", "description": "Optional fallback relative path when applying bare @@ hunks to a single file" }
                        },
                        "required": ["patch"]
                    }
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "lsp_diagnostics",
                    "description": "Run incremental workspace compiler and type diagnostics (`cargo check`, `tsc --noEmit`, `go vet`, syntax checks) and return errors/warnings.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Optional relative file path to filter compiler diagnostics" }
                        }
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
            for synth in Self::load_synthesized_tools() {
                arr.push(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": synth.name,
                        "description": format!("[Synthesized Tool] {}", synth.description),
                        "parameters": synth.parameters
                    }
                }));
            }

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

                    // 1. Direct clean tool name (e.g. `generate_image`, `restyle_image`) if not colliding with core tools
                    let is_core_collision = arr.iter().any(|existing| {
                        existing.get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            == Some(&t.name)
                    });
                    if !is_core_collision {
                        arr.push(serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": format!("[MCP: {}] {}", srv.name, t.description),
                                "parameters": params.clone()
                            }
                        }));
                    }

                    // 2. Namespaced full name mcp__{safe_srv}__{safe_tl}
                    let full_name = format!("mcp__{}__{}", safe_srv, safe_tl);
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

    /// Returns OpenAI-compatible JSON function tool schemas filtered to `allowed` tool names
    /// (used for role-scoped Explorer/Auditor/Worker subagents).
    pub fn tool_schemas_filtered(allowed: Option<&[String]>) -> serde_json::Value {
        let all = Self::tool_schemas();
        let Some(allow_list) = allowed.filter(|a| !a.is_empty()) else {
            return all;
        };
        let Some(arr) = all.as_array() else {
            return all;
        };
        let filtered: Vec<serde_json::Value> = arr
            .iter()
            .filter(|item| {
                let name = item
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let desc = item
                    .get("function")
                    .and_then(|f| f.get("description"))
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                allow_list.iter().any(|a| {
                    a.eq_ignore_ascii_case(name)
                        || (a.eq_ignore_ascii_case("ast_outline") && name == "outline_file")
                        || (a.eq_ignore_ascii_case("outline_file") && name == "ast_outline")
                        || (a.eq_ignore_ascii_case("ast_find_callers") && name == "find_callers")
                        || (a.eq_ignore_ascii_case("find_callers") && name == "ast_find_callers")
                        || (a.eq_ignore_ascii_case("call_mcp_tool") && (name.starts_with("mcp__") || name.starts_with("mcp_") || desc.starts_with("[MCP:")))
                })
            })
            .cloned()
            .collect();
        serde_json::Value::Array(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tools_exposed_in_schemas_and_dispatchable() {
        let schemas = ToolEngine::tool_schemas();
        let arr = schemas.as_array().expect("Tool schemas must be array");
        let has_generate_image = arr.iter().any(|item| {
            item.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                == Some("generate_image")
        });
        assert!(has_generate_image, "generate_image must be directly present in tool schemas for DMD Studio");
    }

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
    fn test_stateful_shell_cwd_env_and_and_operator() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        let engine = ToolEngine::new(ws.clone());
        fs::create_dir_all(ws.root().join("nested_dir")).unwrap();

        // 1. Test `&&` operator + persistent `cd` across `run_command` calls
        #[cfg(target_os = "windows")]
        let (cmd1, cmd2, cmd3, expected1, expected2) = (
            "cd nested_dir && Write-Output 'inside_nested'",
            "$env:UNJESS_TEST_STATE = 'persisted_ok'",
            "Write-Output $env:UNJESS_TEST_STATE",
            "inside_nested",
            "persisted_ok",
        );
        #[cfg(not(target_os = "windows"))]
        let (cmd1, cmd2, cmd3, expected1, expected2) = (
            "cd nested_dir && echo 'inside_nested'",
            "export UNJESS_TEST_STATE='persisted_ok'",
            "echo $UNJESS_TEST_STATE",
            "inside_nested",
            "persisted_ok",
        );

        let out1 = engine.run_command(cmd1, 30).unwrap();
        assert!(out1.contains(expected1));
        assert!(engine.session_cwd().ends_with("nested_dir"));

        // 2. Next command runs inside `nested_dir` and sets a persistent env var
        let _ = engine.run_command(cmd2, 30).unwrap();
        let out2 = engine.run_command(cmd3, 30).unwrap();
        assert!(out2.contains(expected2));
    }

    #[test]
    fn test_protected_paths_and_edit_precision_helpers() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        let mut engine = ToolEngine::new(ws.clone());

        engine
            .write_file(
                "run_pretrain.sh",
                "#!/bin/bash\necho 'training'\n",
                true,
            )
            .unwrap();

        let extracted = extract_protected_paths_from_prompt(
            "Fix the model bug, but do not modify `/app/run_pretrain.sh` and leave `config.yaml` unchanged.",
        );
        assert!(extracted.iter().any(|p| p.contains("run_pretrain.sh")));
        assert!(extracted.iter().any(|p| p.contains("config.yaml")));

        engine.set_protected_paths(extracted);
        assert!(engine.write_file("run_pretrain.sh", "hacked", true).is_err());
        assert!(engine.delete_file("run_pretrain.sh").is_err());
        assert!(engine
            .run_command("echo 'bad' > run_pretrain.sh", 10)
            .is_err());

        // Test `strip_read_file_line_prefixes` and closest match preview
        engine
            .write_file(
                "src/calc.rs",
                "pub fn compute(a: i32) -> i32 {\n    let step = a + 1;\n    step * 2\n}\n",
                true,
            )
            .unwrap();
        let diff = engine
            .edit_file(
                "src/calc.rs",
                "   2:     let step = a + 1;\n   3:     step * 2",
                "   2:     let step = a + 10;\n   3:     step * 3",
            )
            .unwrap();
        assert_eq!(diff.status, "modified");
        assert!(fs::read_to_string(ws.root().join("src/calc.rs"))
            .unwrap()
            .contains("a + 10"));

        let err = engine
            .edit_file("src/calc.rs", "let step = a + 999;", "let step = 0;")
            .unwrap_err();
        assert!(err.to_string().contains("Closest match in file"));

        // Test Frontier tools: `todo_write`, `ast_edit_symbol`, `shadow_sandbox`
        let (todo_out, todo_ok, _) = engine.execute_tool(
            "todo_write",
            &serde_json::json!({
                "todos": [
                    { "id": "1", "content": "Audit symbols", "status": "completed" },
                    { "id": "2", "content": "Patch calc.rs", "status": "in_progress" }
                ]
            }),
        );
        assert!(todo_ok);
        assert!(todo_out.contains("1/2 completed"));

        let (ast_out, ast_ok, ast_diff) = engine.execute_tool(
            "ast_edit_symbol",
            &serde_json::json!({
                "path": "src/calc.rs",
                "symbol": "compute",
                "new_code": "pub fn compute(a: i32) -> i32 {\n    a * 42\n}"
            }),
        );
        assert!(ast_ok);
        assert!(ast_diff.is_some());
        assert!(ast_out.contains("[Auto-Verify"));

        let (_, sb_ok, _) = engine.execute_tool(
            "shadow_sandbox",
            &serde_json::json!({ "action": "create" }),
        );
        assert!(sb_ok);
        let (_, sb_disc_ok, _) = engine.execute_tool(
            "shadow_sandbox",
            &serde_json::json!({ "action": "discard" }),
        );
        assert!(sb_disc_ok);
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

    #[test]
    fn test_apply_patch_fuzzy_replacer_and_policy_matrix() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        let engine = ToolEngine::new(ws.clone());

        // 1. Test multi-file `apply_patch` (`*** Add File:` and `*** Update File:`)
        let patch_create = "*** Begin Patch\n*** Add File: src/lib.rs\n+pub fn greet() -> &'static str {\n+    \"hello\"\n+}\n*** End Patch\n";
        let (sum1, diffs1) = engine.apply_patch(patch_create, None).unwrap();
        assert!(sum1.contains("Added `src/lib.rs`"));
        assert_eq!(diffs1.len(), 1);

        let patch_update = "*** Begin Patch\n*** Update File: src/lib.rs\n@@ -1,3 +1,3 @@\n pub fn greet() -> &'static str {\n-    \"hello\"\n+    \"hello from patch\"\n }\n*** End Patch\n";
        let (sum2, diffs2) = engine.apply_patch(patch_update, None).unwrap();
        assert!(sum2.contains("Updated `src/lib.rs`"));
        assert_eq!(diffs2.len(), 1);
        assert!(fs::read_to_string(ws.root().join("src/lib.rs"))
            .unwrap()
            .contains("hello from patch"));

        // 2. Test 9-stage fuzzy block replacer (Stage 4 whitespace-normalized & Stage 8 block-anchor match)
        engine
            .write_file(
                "src/fuzzy.rs",
                "pub fn complex_block(a: i32, b: i32) -> i32 {\n    let alpha   =   a + 1;\n    let beta = b + 2;\n    let gamma = alpha * beta;\n    gamma\n}\n",
                true,
            )
            .unwrap();
        let _ = engine
            .edit_file(
                "src/fuzzy.rs",
                "pub fn complex_block(a: i32, b: i32) -> i32 {\n    let alpha = a + 1;\n    let beta = b + 2;\n    let gamma = alpha * beta;\n    gamma\n}",
                "pub fn complex_block(a: i32, b: i32) -> i32 {\n    (a + 1) * (b + 20)\n}",
            )
            .unwrap();
        assert!(fs::read_to_string(ws.root().join("src/fuzzy.rs"))
            .unwrap()
            .contains("b + 20"));

        // 3. Test Granular Tool & Command Policy Matrix
        fs::write(ws.root().join(".env"), "SECRET_KEY=topsecret\n").unwrap();
        assert!(engine.write_file(".env", "OVERWRITTEN=1", true).is_err());
        assert!(engine.delete_file(".env").is_err());
        assert!(engine.run_command("git push --force origin main", 10).is_err());
        assert!(engine.run_command("git reset --hard HEAD~1", 10).is_err());
    }

    #[test]
    fn test_view_image_and_vision_drain() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::new(tmp.path()).unwrap();
        let mut engine = ToolEngine::new(ws.clone());

        let img_path = tmp.path().join("test_shot.png");
        fs::write(&img_path, b"\x89PNG\r\n\x1a\nfake_image_bytes").unwrap();

        let (out, ok, diff) = engine.execute_tool("view_image", &serde_json::json!({ "path": "test_shot.png" }));
        assert!(ok);
        assert!(out.contains("Image file loaded and attached to your Vision input"));
        assert!(diff.is_none());

        let drained = engine.drain_vision_images();
        assert_eq!(drained.len(), 1);
        assert!(!drained[0].is_empty());
        assert!(engine.drain_vision_images().is_empty());

        let (out2, ok2, _) = engine.execute_tool("read_file", &serde_json::json!({ "path": "test_shot.png" }));
        assert!(ok2);
        assert!(out2.contains("Image file loaded and attached to your Vision input"));
        let drained2 = engine.drain_vision_images();
        assert_eq!(drained2.len(), 1);
    }

    #[test]
    fn test_optimize_image_for_vision_downscales_large_image() {
        // Create an uncompressed 2000x2000 image in memory
        let img = image::RgbImage::from_fn(2000, 2000, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, ((x + y) % 255) as u8])
        });
        let mut raw_png = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut raw_png);
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        assert!(!raw_png.is_empty());

        let (optimized_bytes, mime) = ToolEngine::optimize_image_for_vision(&raw_png, "png");
        assert_eq!(mime, "image/jpeg");
        // Verify output is significantly smaller than raw high-res image
        assert!(optimized_bytes.len() < raw_png.len());

        // Verify decoded dimensions are <= 1568px
        let decoded = image::load_from_memory(&optimized_bytes).unwrap();
        assert!(decoded.width() <= 1568);
        assert!(decoded.height() <= 1568);

        // Verify that invalid/mock bytes safely return raw bytes without panicking
        let mock_bytes = b"not_a_real_image_data";
        let (fallback_bytes, fallback_mime) = ToolEngine::optimize_image_for_vision(mock_bytes, "png");
        assert_eq!(fallback_bytes, mock_bytes);
        assert_eq!(fallback_mime, "image/png");
    }
}
