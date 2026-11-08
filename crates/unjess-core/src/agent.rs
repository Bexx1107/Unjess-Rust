//! Autonomous Multi-Turn Agent Engine & Persistent Harness Manager (`agent.rs`).
//!
//! Manages workspaces, conversations, custom subagents, multi-step workflows,
//! 4-way execution modes (`Autonomous`, `Plan`, `Supervised`, `Duel`),
//! `Caveman` & `Ponytail` persona modifiers, slash-command execution, stuck-loop
//! recovery, budget circuit-breakers, provider fallback chains, and the asynchronous
//! multi-turn tool loop.

use crate::codeintel::generate_repo_map;
use crate::config::Settings;
use crate::llm::{
    complete_llm_turn, complete_llm_turn_filtered, model_context_window_tokens, ChatMessage,
};
use crate::mcp::McpManager;
use crate::memory::MemoryStore;
use crate::security::scan_workspace;
use crate::skills::SkillsManager;
use crate::tools::{FileDiff, ToolCallRecord, ToolEngine};
use crate::workspace::Workspace;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: String,
    pub autonomous: bool,
    pub workspace_path: String,
    pub context_mode: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "qwen2.5-coder:7b".to_string(),
            autonomous: true,
            workspace_path: ".".to_string(),
            context_mode: "Auto".to_string(),
        }
    }
}

/// Workspace entry registered in the Harness state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub id: String,
    pub name: String,
    pub path: String,
}

fn default_conv_mode() -> String {
    "workspace".to_string()
}

/// Persistent conversation thread with messages, token usage, and USD cost tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub workspace_id: String,
    #[serde(default = "default_conv_mode")]
    pub mode: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: String,
    #[serde(default)]
    pub total_tokens: usize,
    #[serde(default)]
    pub total_input_tokens: usize,
    #[serde(default)]
    pub total_output_tokens: usize,
    #[serde(default)]
    pub total_cost_usd: f64,
}

fn deserialize_log_time<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    match val {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(format!("{:.0}", n.as_f64().unwrap_or(0.0))),
        _ => Ok(Utc::now().format("%H:%M:%S").to_string()),
    }
}

/// Timestamped log entry for subagent activity inspection in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentLogEntry {
    #[serde(deserialize_with = "deserialize_log_time")]
    pub time: String,
    pub event: String,
}

/// Result of a multi-turn subagent task execution, including rolled-up diffs and token/cost metrics.
#[derive(Debug, Clone, Default)]
pub struct SubagentTurnResult {
    pub output: String,
    pub diffs: Vec<FileDiff>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub tokens: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub cost_usd: f64,
}

/// Custom or built-in autonomous subagent definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentDef {
    pub id: String,
    pub name: String,
    pub role: String,
    pub system_prompt: String,
    pub model: String,
    pub status: String,
    #[serde(default)]
    pub last_output: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub conv_id: String,
    #[serde(default)]
    pub current_task: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub diffs: Vec<FileDiff>,
    #[serde(default)]
    pub logs: Vec<SubagentLogEntry>,
    #[serde(default)]
    pub total_tokens: usize,
    #[serde(default)]
    pub total_cost_usd: f64,
}

fn default_wf_category() -> String {
    "general".to_string()
}

/// Multi-step reusable prompt workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_wf_category")]
    pub category: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub steps: Vec<String>,
}

impl WorkflowDef {
    pub fn normalize(&mut self) {
        if self.title.is_empty() && !self.name.is_empty() {
            self.title = self.name.clone();
        } else if self.name.is_empty() && !self.title.is_empty() {
            self.name = self.title.clone();
        }
        if self.prompt.is_empty() && !self.steps.is_empty() {
            self.prompt = self.steps.join("\n");
        } else if self.steps.is_empty() && !self.prompt.is_empty() {
            self.steps = self
                .prompt
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        }
        if self.category.is_empty() {
            self.category = "general".to_string();
        }
    }
}

/// Real-time snapshot of an in-progress autonomous agent turn.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveTurnSnapshot {
    pub running: bool,
    pub conv_id: String,
    pub current_step: String,
    pub thinking: String,
    pub streaming_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pub diffs: Vec<FileDiff>,
    pub iteration: usize,
    #[serde(default)]
    pub started_at: f64,
    #[serde(default)]
    pub elapsed_s: f64,
    #[serde(default)]
    pub tokens: usize,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedHarnessState {
    workspaces: Vec<WorkspaceEntry>,
    active_workspace_id: String,
    conversations: Vec<Conversation>,
    active_conversation_id: String,
    subagents: Vec<SubagentDef>,
    workflows: Vec<WorkflowDef>,
    agent_mode: String,
    caveman_mode: bool,
    ponytail_mode: bool,
    #[serde(default)]
    tree_snapshots: std::collections::HashMap<String, Vec<String>>,
}

/// Thread-safe central state and autonomous execution manager for Unjess 4.0.
pub struct HarnessManager {
    settings: RwLock<Settings>,
    state: RwLock<PersistedHarnessState>,
    live_turn: RwLock<LiveTurnSnapshot>,
    abort_flag: Arc<AtomicBool>,
}

impl Default for HarnessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessManager {
    fn state_path() -> PathBuf {
        Settings::config_dir().join("harness_state.json")
    }

    fn now_epoch_secs() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Initializes `HarnessManager`, loading settings and persistent state from `~/.unjess-rust`.
    pub fn new() -> Self {
        let settings = Settings::load();
        let state_file = Self::state_path();

        let mut state = if let Ok(raw) = fs::read_to_string(&state_file) {
            serde_json::from_str::<PersistedHarnessState>(&raw)
                .unwrap_or_else(|_| Self::default_state(&settings))
        } else {
            Self::default_state(&settings)
        };

        for wf in &mut state.workflows {
            wf.normalize();
        }
        // Reset any stale "running" subagent statuses left from a prior process exit
        // and ensure built-in role-scoped subagents have their full tool allowlists.
        for sub in &mut state.subagents {
            if sub.status == "running" {
                sub.status = "idle".to_string();
            }
            let default_role_tools: &[&str] = match sub.id.as_str() {
                "sub-architect" => &[
                    "read_file",
                    "outline_file",
                    "repo_map",
                    "find_callers",
                    "grep_search",
                    "find_by_name",
                    "list_dir",
                    "git_archeology",
                    "lsp_diagnostics",
                    "run_skill_plugin",
                    "call_mcp_tool",
                    "probe_mcp_server",
                    "mcp_status",
                ],
                "sub-coder" => &[
                    "read_file",
                    "write_file",
                    "edit_file",
                    "multi_edit_file",
                    "apply_patch",
                    "ast_edit_symbol",
                    "ast_rename_symbol",
                    "delete_file",
                    "grep_search",
                    "find_by_name",
                    "list_dir",
                    "outline_file",
                    "find_callers",
                    "lsp_diagnostics",
                    "debug_trace",
                    "shadow_sandbox",
                    "run_command",
                    "run_skill_plugin",
                    "call_mcp_tool",
                    "probe_mcp_server",
                    "mcp_status",
                ],
                "sub-security" => &[
                    "grep_search",
                    "find_by_name",
                    "list_dir",
                    "read_file",
                    "outline_file",
                    "find_callers",
                    "git_archeology",
                    "lsp_diagnostics",
                    "security_scan",
                    "run_skill_plugin",
                    "call_mcp_tool",
                ],
                _ => &[],
            };
            for &t in default_role_tools {
                if !sub.tools.iter().any(|existing| existing == t) {
                    sub.tools.push(t.to_string());
                }
            }
        }

        let mgr = Self {
            settings: RwLock::new(settings),
            state: RwLock::new(state),
            live_turn: RwLock::new(LiveTurnSnapshot::default()),
            abort_flag: Arc::new(AtomicBool::new(false)),
        };
        mgr.save_state();
        mgr
    }

    fn default_state(settings: &Settings) -> PersistedHarnessState {
        let ws_path = if settings.workspace.trim().is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        } else {
            settings.workspace.clone()
        };
        let ws_name = PathBuf::from(&ws_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string());

        let ws_id = "ws-default".to_string();
        let conv_id = Uuid::new_v4().to_string();
        let now = Utc::now().format("%Y-%m-%d %H:%M").to_string();

        let mut wf1 = WorkflowDef {
            id: "wf-audit-fix".to_string(),
            name: "Full Security Audit & Patch".to_string(),
            title: "Full Security Audit & Patch".to_string(),
            description: "Scans the workspace for vulnerabilities, synthesizes White Team patches, and runs unit tests.".to_string(),
            category: "security".to_string(),
            prompt: "Inspect workspace structure and identify high-risk entrypoints.\nAudit authentication, command execution, and file handling for vulnerabilities.\nApply defensive patches and verify test suite passes.".to_string(),
            tools: vec!["grep_search".to_string(), "read_file".to_string(), "edit_file".to_string(), "run_command".to_string()],
            steps: vec![
                "Inspect workspace structure and identify high-risk entrypoints.".to_string(),
                "Audit authentication, command execution, and file handling for vulnerabilities.".to_string(),
                "Apply defensive patches and verify test suite passes.".to_string(),
            ],
        };
        wf1.normalize();

        let mut wf2 = WorkflowDef {
            id: "wf-test-hardening".to_string(),
            name: "Test Suite & Build Verification".to_string(),
            title: "Test Suite & Build Verification".to_string(),
            description: "Runs compiler checks and unit tests, fixing any regressions automatically.".to_string(),
            category: "testing".to_string(),
            prompt: "Run build and test suite across workspace.\nDiagnose any compiler warnings or failing tests and patch them.".to_string(),
            tools: vec!["run_command".to_string(), "read_file".to_string(), "edit_file".to_string()],
            steps: vec![
                "Run build and test suite across workspace.".to_string(),
                "Diagnose any compiler warnings or failing tests and patch them.".to_string(),
            ],
        };
        wf2.normalize();

        PersistedHarnessState {
            workspaces: vec![WorkspaceEntry {
                id: ws_id.clone(),
                name: ws_name,
                path: ws_path,
            }],
            active_workspace_id: ws_id.clone(),
            conversations: vec![Conversation {
                id: conv_id.clone(),
                title: "Main Engineering Session".to_string(),
                workspace_id: ws_id,
                mode: "workspace".to_string(),
                messages: Vec::new(),
                created_at: now,
                total_tokens: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: 0.0,
            }],
            active_conversation_id: conv_id.clone(),
            subagents: vec![
                SubagentDef {
                    id: "sub-architect".to_string(),
                    name: "Code Architect".to_string(),
                    role: "Systems & Rust Architecture Specialist".to_string(),
                    system_prompt: "You are the Unjess Code Architect. Design clean, modular, memory-safe systems and verify module boundaries.".to_string(),
                    model: "inherit".to_string(),
                    status: "idle".to_string(),
                    last_output: String::new(),
                    output: String::new(),
                    prompt: String::new(),
                    result: String::new(),
                    text: String::new(),
                    conv_id: conv_id.clone(),
                    current_task: String::new(),
                    tools: vec![
                        "read_file".to_string(),
                        "outline_file".to_string(),
                        "repo_map".to_string(),
                        "find_callers".to_string(),
                        "grep_search".to_string(),
                        "find_by_name".to_string(),
                        "list_dir".to_string(),
                        "git_archeology".to_string(),
                        "lsp_diagnostics".to_string(),
                        "run_skill_plugin".to_string(),
                        "call_mcp_tool".to_string(),
                        "probe_mcp_server".to_string(),
                        "mcp_status".to_string(),
                    ],
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    logs: Vec::new(),
                    total_tokens: 0,
                    total_cost_usd: 0.0,
                },
                SubagentDef {
                    id: "sub-coder".to_string(),
                    name: "Autonomous Coder".to_string(),
                    role: "Full-Stack & Native Implementation Engineer".to_string(),
                    system_prompt: "You are the Unjess Autonomous Coder. Write complete, production-ready code with zero stubs and run verification tests.".to_string(),
                    model: "inherit".to_string(),
                    status: "idle".to_string(),
                    last_output: String::new(),
                    output: String::new(),
                    prompt: String::new(),
                    result: String::new(),
                    text: String::new(),
                    conv_id: conv_id.clone(),
                    current_task: String::new(),
                    tools: vec![
                        "read_file".to_string(),
                        "write_file".to_string(),
                        "edit_file".to_string(),
                        "multi_edit_file".to_string(),
                        "apply_patch".to_string(),
                        "ast_edit_symbol".to_string(),
                        "ast_rename_symbol".to_string(),
                        "delete_file".to_string(),
                        "grep_search".to_string(),
                        "find_by_name".to_string(),
                        "list_dir".to_string(),
                        "outline_file".to_string(),
                        "find_callers".to_string(),
                        "lsp_diagnostics".to_string(),
                        "debug_trace".to_string(),
                        "shadow_sandbox".to_string(),
                        "run_command".to_string(),
                        "run_skill_plugin".to_string(),
                        "call_mcp_tool".to_string(),
                        "probe_mcp_server".to_string(),
                        "mcp_status".to_string(),
                    ],
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    logs: Vec::new(),
                    total_tokens: 0,
                    total_cost_usd: 0.0,
                },
                SubagentDef {
                    id: "sub-security".to_string(),
                    name: "Security Auditor".to_string(),
                    role: "Red Team / White Team Vulnerability Researcher".to_string(),
                    system_prompt: "You are the Unjess Security Auditor. Audit code for injection, secret leaks, path traversal, and unsafe memory patterns.".to_string(),
                    model: "inherit".to_string(),
                    status: "idle".to_string(),
                    last_output: String::new(),
                    output: String::new(),
                    prompt: String::new(),
                    result: String::new(),
                    text: String::new(),
                    conv_id: conv_id,
                    current_task: String::new(),
                    tools: vec![
                        "grep_search".to_string(),
                        "find_by_name".to_string(),
                        "list_dir".to_string(),
                        "read_file".to_string(),
                        "outline_file".to_string(),
                        "find_callers".to_string(),
                        "git_archeology".to_string(),
                        "lsp_diagnostics".to_string(),
                        "security_scan".to_string(),
                        "run_skill_plugin".to_string(),
                        "call_mcp_tool".to_string(),
                    ],
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    logs: Vec::new(),
                    total_tokens: 0,
                    total_cost_usd: 0.0,
                },
            ],
            workflows: vec![wf1, wf2],
            agent_mode: "Autonomous".to_string(),
            caveman_mode: false,
            ponytail_mode: false,
            tree_snapshots: std::collections::HashMap::new(),
        }
    }

    fn save_state(&self) {
        let Ok(guard) = self.state.read() else {
            return;
        };
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&*guard) {
            let tmp_path = path.with_extension("json.tmp");
            if fs::write(&tmp_path, &json).is_ok() {
                if fs::rename(&tmp_path, &path).is_err() {
                    let _ = fs::write(&path, &json);
                    let _ = fs::remove_file(&tmp_path);
                }
            } else {
                let _ = fs::write(&path, &json);
            }
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn update_settings(&self, mut s: Settings) {
        s.sync_provider_with_model();
        let _ = s.save();
        if let Ok(mut guard) = self.settings.write() {
            *guard = s;
        }
    }

    /// Updates settings in memory for the current process without overwriting `config.yaml` on disk.
    pub fn update_settings_in_memory(&self, mut s: Settings) {
        s.sync_provider_with_model();
        if let Ok(mut guard) = self.settings.write() {
            *guard = s;
        }
    }

    pub fn active_workspace_path(&self) -> PathBuf {
        if let Ok(guard) = self.state.read() {
            if let Some(ws) = guard
                .workspaces
                .iter()
                .find(|w| w.id == guard.active_workspace_id)
                .or_else(|| guard.workspaces.first())
            {
                return PathBuf::from(&ws.path);
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn workspaces(&self) -> Vec<WorkspaceEntry> {
        self.prune_missing_workspaces();
        self.state
            .read()
            .map(|g| g.workspaces.clone())
            .unwrap_or_default()
    }

    /// Automatically removes workspace entries whose directories were deleted in File Explorer
    /// (always keeping at least one fallback workspace).
    pub fn prune_missing_workspaces(&self) {
        let mut changed = false;
        if let Ok(mut g) = self.state.write() {
            if g.workspaces.len() > 1 {
                let mut removed_ids = Vec::new();
                let total = g.workspaces.len();
                let mut kept = Vec::with_capacity(total);
                for ws in g.workspaces.drain(..) {
                    let p = std::path::Path::new(&ws.path);
                    if ws.path == "." || p.exists() {
                        kept.push(ws);
                    } else {
                        removed_ids.push((ws.id.clone(), ws.path.clone()));
                        changed = true;
                    }
                }
                if kept.is_empty() {
                    let fallback_path = std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| ".".to_string());
                    let fallback_name = PathBuf::from(&fallback_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "workspace".to_string());
                    kept.push(WorkspaceEntry {
                        id: "ws-default".to_string(),
                        name: fallback_name,
                        path: fallback_path,
                    });
                }
                g.workspaces = kept;
                if !g.workspaces.iter().any(|w| w.id == g.active_workspace_id) {
                    if let Some(first) = g.workspaces.first() {
                        g.active_workspace_id = first.id.clone();
                    }
                }
                if !removed_ids.is_empty() {
                    g.conversations.retain(|c| {
                        !removed_ids
                            .iter()
                            .any(|(rid, _)| c.workspace_id.eq_ignore_ascii_case(rid))
                    });
                    if g.conversations.is_empty() {
                        let active_ws_id = g.active_workspace_id.clone();
                        let new_cid = Uuid::new_v4().to_string();
                        g.conversations.push(Conversation {
                            id: new_cid.clone(),
                            title: "Main Engineering Session".to_string(),
                            workspace_id: active_ws_id,
                            mode: "workspace".to_string(),
                            messages: Vec::new(),
                            created_at: Utc::now().format("%Y-%m-%d %H:%M").to_string(),
                            total_tokens: 0,
                            total_input_tokens: 0,
                            total_output_tokens: 0,
                            total_cost_usd: 0.0,
                        });
                        g.active_conversation_id = new_cid;
                    } else if !g
                        .conversations
                        .iter()
                        .any(|c| c.id == g.active_conversation_id)
                    {
                        if let Some(first_c) = g.conversations.first() {
                            g.active_conversation_id = first_c.id.clone();
                        }
                    }
                }
            }
        }
        if changed {
            self.save_state();
        }
    }

    /// Removes a workspace by `id`, `path`, or `name` from the harness state and cleans up its conversations.
    pub fn remove_workspace(&self, target: &str) -> bool {
        let clean = target.trim();
        if clean.is_empty() {
            return false;
        }
        let norm_target = clean.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        let mut removed_ids = Vec::new();
        if let Ok(mut g) = self.state.write() {
            let before = g.workspaces.len();
            g.workspaces.retain(|w| {
                let norm_path = w
                    .path
                    .replace('/', "\\")
                    .trim_end_matches('\\')
                    .to_lowercase();
                let matched = w.id.eq_ignore_ascii_case(clean)
                    || norm_path == norm_target
                    || w.name.eq_ignore_ascii_case(clean);
                if matched {
                    removed_ids.push(w.id.clone());
                }
                !matched
            });
            if g.workspaces.len() == before && removed_ids.is_empty() {
                return false;
            }
            if g.workspaces.is_empty() {
                let fallback_path = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string());
                let fallback_name = PathBuf::from(&fallback_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "workspace".to_string());
                g.workspaces.push(WorkspaceEntry {
                    id: "ws-default".to_string(),
                    name: fallback_name,
                    path: fallback_path,
                });
            }
            if !g.workspaces.iter().any(|w| w.id == g.active_workspace_id) {
                if let Some(first) = g.workspaces.first() {
                    g.active_workspace_id = first.id.clone();
                }
            }
            g.conversations
                .retain(|c| !removed_ids.iter().any(|rid| c.workspace_id == *rid));
            if g.conversations.is_empty() {
                let active_ws_id = g.active_workspace_id.clone();
                let new_cid = Uuid::new_v4().to_string();
                g.conversations.push(Conversation {
                    id: new_cid.clone(),
                    title: "Main Engineering Session".to_string(),
                    workspace_id: active_ws_id,
                    mode: "workspace".to_string(),
                    messages: Vec::new(),
                    created_at: Utc::now().format("%Y-%m-%d %H:%M").to_string(),
                    total_tokens: 0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cost_usd: 0.0,
                });
                g.active_conversation_id = new_cid;
            } else if !g
                .conversations
                .iter()
                .any(|c| c.id == g.active_conversation_id)
            {
                if let Some(first_c) = g.conversations.first() {
                    g.active_conversation_id = first_c.id.clone();
                }
            }
        }
        self.save_state();
        true
    }

    pub fn active_workspace_id(&self) -> String {
        self.state
            .read()
            .map(|g| g.active_workspace_id.clone())
            .unwrap_or_default()
    }

    pub fn set_active_workspace(&self, id: &str) {
        if let Ok(mut g) = self.state.write() {
            if let Some(ws) = g.workspaces.iter().find(|w| w.id == id).cloned() {
                g.active_workspace_id = ws.id;
                if let Ok(mut s) = self.settings.write() {
                    s.workspace = ws.path;
                    let _ = s.save();
                }
            }
        }
        self.save_state();
    }

    pub fn rename_workspace(&self, id: &str, new_name: &str) {
        let clean = new_name.trim();
        if clean.is_empty() {
            return;
        }
        if let Ok(mut g) = self.state.write() {
            if let Some(ws) = g.workspaces.iter_mut().find(|w| w.id == id) {
                ws.name = clean.to_string();
            }
        }
        self.save_state();
    }

    pub fn add_workspace(&self, path: &str) -> String {
        let clean_path = path.trim().to_string();
        let name = PathBuf::from(&clean_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| clean_path.clone());
        let id = format!("ws-{}", &Uuid::new_v4().to_string()[..8]);

        if let Ok(mut g) = self.state.write() {
            if let Some(existing) = g.workspaces.iter().find(|w| w.path == clean_path) {
                let existing_id = existing.id.clone();
                g.active_workspace_id = existing_id.clone();
                drop(g);
                self.save_state();
                return existing_id;
            }
            g.workspaces.push(WorkspaceEntry {
                id: id.clone(),
                name,
                path: clean_path.clone(),
            });
            g.active_workspace_id = id.clone();
        }
        if let Ok(mut s) = self.settings.write() {
            s.workspace = clean_path;
            let _ = s.save();
        }
        self.save_state();
        id
    }

    pub fn conversations(&self) -> Vec<Conversation> {
        self.state
            .read()
            .map(|g| g.conversations.clone())
            .unwrap_or_default()
    }

    pub fn active_conversation_id(&self) -> String {
        self.state
            .read()
            .map(|g| g.active_conversation_id.clone())
            .unwrap_or_default()
    }

    pub fn active_conversation(&self) -> Conversation {
        if let Ok(g) = self.state.read() {
            if let Some(c) = g
                .conversations
                .iter()
                .find(|c| c.id == g.active_conversation_id)
                .or_else(|| g.conversations.first())
            {
                return c.clone();
            }
        }
        Conversation {
            id: "default".to_string(),
            title: "New Chat".to_string(),
            workspace_id: "ws-default".to_string(),
            mode: "workspace".to_string(),
            messages: Vec::new(),
            created_at: Utc::now().format("%Y-%m-%d %H:%M").to_string(),
            total_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
        }
    }

    pub fn conversation_by_id(&self, conv_id: &str) -> Conversation {
        if !conv_id.trim().is_empty() {
            if let Ok(g) = self.state.read() {
                if let Some(c) = g.conversations.iter().find(|c| c.id == conv_id) {
                    return c.clone();
                }
            }
        }
        self.active_conversation()
    }

    pub fn set_active_conversation(&self, id: &str) {
        if let Ok(mut g) = self.state.write() {
            if g.conversations.iter().any(|c| c.id == id) {
                g.active_conversation_id = id.to_string();
            }
        }
        self.save_state();
    }

    pub fn new_conversation(&self, title: Option<&str>) -> String {
        self.new_conversation_with_mode(title, None)
    }

    pub fn new_conversation_with_mode(&self, title: Option<&str>, mode: Option<&str>) -> String {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().format("%Y-%m-%d %H:%M").to_string();
        let conv_mode = mode
            .filter(|m| !m.trim().is_empty())
            .unwrap_or("workspace")
            .to_string();
        if let Ok(mut g) = self.state.write() {
            let ws_id = g.active_workspace_id.clone();
            let chat_title = title
                .filter(|t| !t.trim().is_empty())
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| {
                    if conv_mode == "quick" {
                        "Quick Chat".to_string()
                    } else {
                        format!("Session {}", g.conversations.len() + 1)
                    }
                });
            g.conversations.insert(
                0,
                Conversation {
                    id: id.clone(),
                    title: chat_title,
                    workspace_id: ws_id,
                    mode: conv_mode,
                    messages: Vec::new(),
                    created_at: now,
                    total_tokens: 0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cost_usd: 0.0,
                },
            );
            g.active_conversation_id = id.clone();
        }
        self.save_state();
        id
    }

    pub fn rename_conversation(&self, id: &str, new_title: &str) {
        if new_title.trim().is_empty() {
            return;
        }
        if let Ok(mut g) = self.state.write() {
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == id) {
                conv.title = new_title.trim().to_string();
            }
        }
        self.save_state();
    }

    pub fn delete_conversation(&self, id: &str) {
        if let Ok(mut g) = self.state.write() {
            g.conversations.retain(|c| c.id != id);
            if g.conversations.is_empty() {
                let new_id = Uuid::new_v4().to_string();
                let ws_id = g.active_workspace_id.clone();
                g.conversations.push(Conversation {
                    id: new_id.clone(),
                    title: "Main Engineering Session".to_string(),
                    workspace_id: ws_id,
                    mode: "workspace".to_string(),
                    messages: Vec::new(),
                    created_at: Utc::now().format("%Y-%m-%d %H:%M").to_string(),
                    total_tokens: 0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cost_usd: 0.0,
                });
                g.active_conversation_id = new_id;
            } else if g.active_conversation_id == id {
                g.active_conversation_id = g.conversations[0].id.clone();
            }
        }
        self.save_state();
    }

    pub fn clear_active_conversation(&self) {
        if let Ok(mut g) = self.state.write() {
            let active_id = g.active_conversation_id.clone();
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == active_id) {
                conv.messages.clear();
                conv.total_tokens = 0;
                conv.total_input_tokens = 0;
                conv.total_output_tokens = 0;
                conv.total_cost_usd = 0.0;
            }
        }
        self.save_state();
    }

    /// Deterministically summarizes older messages into a structured digest preserving
    /// user requests, modified files, tool invocations, and key assistant conclusions.
    pub fn compact_messages(messages: &[ChatMessage], keep_recent: usize) -> Vec<ChatMessage> {
        if messages.len() <= keep_recent {
            return messages.to_vec();
        }

        // Always preserve initial user prompt if present at index 0 (Task Specification Anchor)
        let has_initial_user = !messages.is_empty() && messages[0].role == "user";
        let (root_user, rest) = if has_initial_user {
            (Some(messages[0].clone()), &messages[1..])
        } else {
            (None, messages)
        };

        if rest.len() <= keep_recent {
            let mut out = Vec::new();
            if let Some(root) = root_user {
                out.push(root);
            }
            out.extend_from_slice(rest);
            return out;
        }

        let mut split_at = rest.len().saturating_sub(keep_recent);
        // Avoid splitting an assistant tool call from its tool result:
        if split_at > 0 && rest[split_at].role == "tool" {
            split_at = split_at.saturating_sub(1);
        }

        let head = &rest[..split_at];
        let tail = &rest[split_at..];

        let mut digest_lines = Vec::new();
        let mut touched_files = Vec::new();
        let mut head_tokens = 0usize;
        let mut head_in_tokens = 0usize;
        let mut head_out_tokens = 0usize;
        let mut head_cost_usd = 0.0f64;

        for m in head {
            head_tokens += m.tokens;
            head_in_tokens += m.input_tokens;
            head_out_tokens += m.output_tokens;
            head_cost_usd += m.cost_usd;
            for d in &m.diffs {
                if !touched_files.contains(&d.path) {
                    touched_files.push(d.path.clone());
                }
            }
            let snippet: String = m
                .content
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(140)
                .collect();
            if m.role == "user" || m.role == "assistant" {
                let tools_note = if m.tool_calls.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = m.tool_calls.iter().map(|t| t.name.clone()).collect();
                    format!(" [tools: {}]", names.join(", "))
                };
                digest_lines.push(format!("- [{}]{}: {}", m.role, tools_note, snippet));
            }
        }

        let mut summary_text = format!(
            "[Context Compacted — Summarized {} earlier intermediate messages]:\n{}",
            head.len(),
            digest_lines
                .into_iter()
                .take(25)
                .collect::<Vec<_>>()
                .join("\n")
        );
        if !touched_files.is_empty() {
            summary_text.push_str(&format!(
                "\nFiles modified in earlier turns: {}",
                touched_files.join(", ")
            ));
        }

        let tok_est = (summary_text.len() / 4).max(16);
        let head_model = head.iter().rev().find_map(|m| m.model.clone());

        let mut new_msgs = Vec::new();
        if let Some(root) = root_user {
            new_msgs.push(root);
        }
        if !head.is_empty() {
            new_msgs.push(ChatMessage {
                role: "system".to_string(),
                content: summary_text,
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: Utc::now().format("%H:%M:%S").to_string(),
                model: head_model,
                tokens: head_tokens.max(tok_est),
                input_tokens: head_in_tokens.max(tok_est),
                output_tokens: head_out_tokens,
                cost_usd: head_cost_usd,
            });
        }
        new_msgs.extend_from_slice(tail);
        new_msgs
    }

    /// Two-stage KV-cache-friendly intra-turn compaction (128K default baseline):
    /// 1. Deduplicates superseded `read_file` outputs that share the exact same `[File: ...]` header.
    /// 2. Stage-1 (above 100,000 chars): prunes older tool outputs (>5,500 chars) to 4,096 head + 1,024 tail chars.
    /// 3. Stage-2 (above 160,000 chars): summarizes older messages via `compact_messages`.
    pub fn compact_messages_smartly(messages: &[ChatMessage]) -> Vec<ChatMessage> {
        Self::compact_messages_for_model(messages, "gpt-4o")
    }

    /// Model-aware dynamic context compaction with a 20% headroom buffer:
    /// Scales Stage-1 and Stage-2 character thresholds to the active model's token context window
    /// (e.g. 32K local GGUF vs. 128K/200K/1M cloud models) and prunes both `m.content` and
    /// structured `m.tool_calls` outputs for native multi-turn tool replay.
    pub fn compact_messages_for_model(messages: &[ChatMessage], model: &str) -> Vec<ChatMessage> {
        let mut msgs = messages.to_vec();
        let ctx_tokens = model_context_window_tokens(model);
        // Reserve 20% headroom for system prompt, tool schemas, and output generation
        let usable_tokens = (ctx_tokens.saturating_mul(80)) / 100;
        let (stage1_threshold, stage2_threshold) = if usable_tokens <= 30_000 {
            (40_000usize, 72_000usize)
        } else if usable_tokens <= 60_000 {
            (70_000usize, 115_000usize)
        } else if usable_tokens <= 120_000 {
            (100_000usize, 160_000usize)
        } else {
            (160_000usize, 260_000usize)
        };

        // Pass 1: Deduplicate superseded `read_file` outputs with identical `[File: ...]` headers
        let mut latest_file_header_idx: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (idx, m) in msgs.iter().enumerate() {
            if m.role == "tool" {
                for line in m.content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("[File: ") && trimmed.ends_with(']') {
                        latest_file_header_idx.insert(trimmed.to_string(), idx);
                    }
                }
                for tc in &m.tool_calls {
                    for line in tc.result.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("[File: ") && trimmed.ends_with(']') {
                            latest_file_header_idx.insert(trimmed.to_string(), idx);
                        }
                    }
                }
            }
        }
        for (idx, m) in msgs.iter_mut().enumerate() {
            if m.role == "tool" {
                let mut first_header = None;
                for line in m.content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("[File: ") && trimmed.ends_with(']') {
                        first_header = Some(trimmed.to_string());
                        break;
                    }
                }
                if let Some(hdr) = first_header {
                    if let Some(&latest_idx) = latest_file_header_idx.get(&hdr) {
                        if idx < latest_idx && m.content.len() > 240 {
                            m.content = format!(
                                "{}\n[Superseded read_file output — see newer read of this file in step #{}]",
                                hdr, latest_idx
                            );
                        }
                    }
                }
                for tc in &mut m.tool_calls {
                    let mut tc_hdr = None;
                    for line in tc.result.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("[File: ") && trimmed.ends_with(']') {
                            tc_hdr = Some(trimmed.to_string());
                            break;
                        }
                    }
                    if let Some(hdr) = tc_hdr {
                        if let Some(&latest_idx) = latest_file_header_idx.get(&hdr) {
                            if idx < latest_idx && tc.result.len() > 240 {
                                tc.result = format!(
                                    "{}\n[Superseded read_file output — see newer read of this file in step #{}]",
                                    hdr, latest_idx
                                );
                            }
                        }
                    }
                }
            }
        }

        // Pass 2: Stage-1 head+tail pruning when total context exceeds `stage1_threshold`
        let total_chars: usize = msgs.iter().map(|m| m.content.len()).sum();
        if total_chars > stage1_threshold && msgs.len() > 4 {
            let protect_from = msgs.len().saturating_sub(4);
            for m in &mut msgs[..protect_from] {
                if m.role == "tool" {
                    if m.content.chars().count() > 5500 {
                        let total_c = m.content.chars().count();
                        let head: String = m.content.chars().take(4096).collect();
                        let tail: String = m
                            .content
                            .chars()
                            .rev()
                            .take(1024)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        m.content = format!(
                            "{}\n\n... [Stage-1 Intra-Turn Pruned {} chars (kept 4096 head + 1024 tail)] ...\n\n{}",
                            head,
                            total_c.saturating_sub(5120),
                            tail
                        );
                    }
                    for tc in &mut m.tool_calls {
                        if tc.result.chars().count() > 5500 {
                            let total_c = tc.result.chars().count();
                            let head: String = tc.result.chars().take(4096).collect();
                            let tail: String = tc
                                .result
                                .chars()
                                .rev()
                                .take(1024)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect();
                            tc.result = format!(
                                "{}\n\n... [Stage-1 Intra-Turn Pruned {} chars (kept 4096 head + 1024 tail)] ...\n\n{}",
                                head,
                                total_c.saturating_sub(5120),
                                tail
                            );
                        }
                    }
                }
            }
        }

        // Pass 3: Stage-2 structural digest compaction when context still exceeds `stage2_threshold`
        let post_stage1_chars: usize = msgs.iter().map(|m| m.content.len()).sum();
        if post_stage1_chars > stage2_threshold && msgs.len() > 16 {
            msgs = Self::compact_messages(&msgs, 16);
        }

        msgs
    }

    /// Returns `true` if `name` is a bookkeeping tool that should be transparent to stuck-loop detection.
    pub fn is_transparent_bookkeeping_tool(name: &str) -> bool {
        matches!(
            name,
            "todo_write"
                | "mcp_status"
                | "manage_background_tasks"
                | "recall_memory"
                | "save_memory"
        )
    }

    /// Computes a deterministic canonical signature for a tool call, normalizing path separators
    /// and sorting JSON object keys so equivalent calls map to the exact same key.
    pub fn canonicalize_tool_call_key(name: &str, args: &serde_json::Value) -> String {
        fn norm_val(k_hint: &str, v: &serde_json::Value) -> String {
            match v {
                serde_json::Value::Object(map) => {
                    let mut keys: Vec<&String> = map.keys().collect();
                    keys.sort();
                    let parts: Vec<String> = keys
                        .into_iter()
                        .map(|k| format!("{}:{}", k, norm_val(k, &map[k])))
                        .collect();
                    format!("{{{}}}", parts.join(","))
                }
                serde_json::Value::Array(arr) => {
                    let parts: Vec<String> = arr.iter().map(|x| norm_val(k_hint, x)).collect();
                    format!("[{}]", parts.join(","))
                }
                serde_json::Value::String(s) => {
                    let trimmed = s.trim();
                    if matches!(k_hint, "path" | "file_path" | "rel_path") {
                        trimmed
                            .replace('\\', "/")
                            .trim_start_matches("./")
                            .to_lowercase()
                    } else {
                        trimmed.to_string()
                    }
                }
                other => other.to_string(),
            }
        }
        format!("{}:{}", name.trim().to_lowercase(), norm_val("", args))
    }

    /// Escalating `[3, 5, 8]` `<mentor_analysis>` stuck-loop recovery reminder.
    pub fn format_repeat_tool_reminder(tool_name: &str, repeat_count: usize) -> Option<String> {
        if repeat_count == 3 {
            Some(format!(
                "<mentor_analysis>\nNotice: You have called `{}` with the exact same arguments 3 times. The output is already in your conversation history above. Do not repeat this identical call; use the existing output to proceed to your next implementation or verification step.\n</mentor_analysis>",
                tool_name
            ))
        } else if repeat_count == 5 {
            Some(format!(
                "<mentor_analysis>\nWARNING (Escalation Level 2): You have repeated `{}` 5 times with identical arguments. You are in a reasoning loop. Change your strategy immediately: either edit the target code, run a diagnostic command, or synthesize your final answer.\n</mentor_analysis>",
                tool_name
            ))
        } else if repeat_count >= 8 {
            Some(format!(
                "<mentor_analysis>\nCRITICAL CIRCUIT BREAKER (Level 3): `{}` was invoked {} times identically. Loop terminated. Summarize your current findings and stop.\n</mentor_analysis>",
                tool_name, repeat_count
            ))
        } else {
            None
        }
    }

    /// Detects when an LLM outputs transitional narration ("I will now check...", "Let me inspect...")
    /// without invoking any tool so the loop can auto-continue instead of terminating prematurely.
    pub fn is_transitional_narration(text: &str) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty()
            || trimmed.len() > 800
            || trimmed.contains("<!-- GOAL_COMPLETE -->")
            || trimmed.contains("```")
        {
            return false;
        }
        let lower = trimmed.to_lowercase();

        // If it asks a question to the user, it is not an uncalled action
        if trimmed.ends_with('?')
            || lower.contains("could you clarify")
            || lower.contains("please provide")
        {
            return false;
        }

        // If it explicitly states the task is complete, it is not transitional
        let completion_markers = [
            "task is complete",
            "task is completed",
            "all requirements have been met",
            "i have completed",
            "here is the final",
            "completed the task",
            "all tests pass",
            "finished the implementation",
            "work is complete",
            "successfully completed",
        ];
        if completion_markers.iter().any(|m| lower.contains(m)) {
            return false;
        }

        if lower.ends_with(':') {
            return true;
        }

        if lower.starts_with("let me ")
            || lower.starts_with("let's ")
            || lower.contains(". let me ")
            || lower.contains(". let's ")
            || lower.contains("\nlet me ")
            || lower.contains("\nlet's ")
        {
            return true;
        }

        let phrases = [
            "let me check",
            "let's check",
            "let me build",
            "let's build",
            "let me write",
            "let's write",
            "let me create",
            "let's create",
            "let me implement",
            "let's implement",
            "let me compare",
            "let's compare",
            "let me construct",
            "let's construct",
            "let me fit",
            "let's fit",
            "let me try",
            "let's try",
            "let me inspect",
            "let's inspect",
            "let me read",
            "let's read",
            "let me look",
            "let's look",
            "let me run",
            "let's run",
            "let me search",
            "let's search",
            "let me examine",
            "let's examine",
            "let me verify",
            "let's verify",
            "let me validate",
            "let's validate",
            "let me test",
            "let's test",
            "let me explore",
            "let's explore",
            "let me see",
            "let me make sure",
            "let's make sure",
            "let me double check",
            "let's double check",
            "i will now",
            "i'll now",
            "now i will",
            "now i'll",
            "now let's",
            "now let me",
            "now we need to",
            "now i need to",
            "i'll start by",
            "i will start by",
            "i'll begin by",
            "i will begin by",
            "i'll first",
            "i will first",
            "first, let's",
            "first, i'll",
            "first, i will",
            "first i'll",
            "first i will",
            "next, i'll",
            "next, i will",
            "next i'll",
            "next i will",
            "i'm going to",
            "i am going to",
            "i plan to",
            "to start, i'll",
            "to start, i will",
            "to begin, i'll",
            "to begin, i will",
            "i need to check",
            "i need to inspect",
            "i need to examine",
            "i need to read",
            "i need to run",
            "i need to test",
            "i need to verify",
            "i will validate",
            "i'll validate",
            "i will test",
            "i'll test",
            "i will investigate",
            "i'll investigate",
            "i will build",
            "i'll build",
            "i will write",
            "i'll write",
            "i will create",
            "i'll create",
            "i will implement",
            "i'll implement",
            "i will compare",
            "i'll compare",
            "i should check",
            "i should verify",
            "i should test",
            "i should run",
        ];
        phrases.iter().any(|p| lower.contains(p))
    }

    /// Fallback parser that extracts structured tool calls when a model emits `<tool_call>{...}</tool_call>`
    /// or a ```tool_call / ```json block in plain text instead of native `tool_calls`.
    pub fn extract_tool_calls_from_text(
        text: &str,
    ) -> Vec<(String, String, serde_json::Value)> {
        let mut extracted = Vec::new();
        let mut candidates = Vec::new();

        if let Ok(xml_re) = regex::Regex::new(r"(?is)<tool_call>\s*(\{.*?\})\s*</tool_call>") {
            for cap in xml_re.captures_iter(text) {
                if let Some(m) = cap.get(1) {
                    candidates.push(m.as_str().to_string());
                }
            }
        }
        if candidates.is_empty() {
            if let Ok(fence_re) =
                regex::Regex::new(r"(?is)```(?:tool_call|json)\s*(\{[\s\S]*?\})\s*```")
            {
                for cap in fence_re.captures_iter(text) {
                    if let Some(m) = cap.get(1) {
                        candidates.push(m.as_str().to_string());
                    }
                }
            }
        }

        const KNOWN_TOOLS: &[&str] = &[
            "read_file",
            "write_file",
            "edit_file",
            "multi_edit_file",
            "delete_file",
            "list_dir",
            "find_by_name",
            "grep_search",
            "find_symbol",
            "outline_file",
            "find_callers",
            "repo_map",
            "todo_write",
            "ast_edit_symbol",
            "ast_rename_symbol",
            "git_archeology",
            "debug_trace",
            "synthesize_tool",
            "shadow_sandbox",
            "run_command",
            "web_search",
            "read_url",
            "save_memory",
            "recall_memory",
            "delete_memory",
            "git_status",
            "git_diff",
        ];

        for raw_json in candidates {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw_json) {
                let name_opt = val
                    .get("name")
                    .or_else(|| val.get("tool"))
                    .and_then(|v| v.as_str());
                if let Some(name) = name_opt {
                    if KNOWN_TOOLS.contains(&name) || name.starts_with("mcp__") {
                        let args = val
                            .get("arguments")
                            .or_else(|| val.get("args"))
                            .or_else(|| val.get("parameters"))
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        extracted.push((
                            format!("call_txt_{}", extracted.len() + 1),
                            name.to_string(),
                            args,
                        ));
                    }
                }
            }
        }
        extracted
    }

    /// Detects temporary debug/repro/scratch files left behind in the workspace at the end of a turn.
    pub fn detect_leftover_scratch_files(
        ws_root: &std::path::Path,
        created_paths: &[String],
        user_prompt: &str,
    ) -> Vec<String> {
        let lower_prompt = user_prompt.to_lowercase();
        let mut leftovers = Vec::new();
        let is_scratch_name = |fname: &str| -> bool {
            let l = fname.to_lowercase();
            l.starts_with("test_debug")
                || l.starts_with("debug_")
                || l.starts_with("repro_")
                || l.starts_with("reproduce_")
                || l.starts_with("scratch_")
                || l.starts_with("tmp_")
                || l.starts_with("temp_test")
                || l.contains("_scratch.")
                || l.contains("_debug_tmp.")
        };

        for rel in created_paths {
            let norm = rel.replace('\\', "/");
            let fname = std::path::Path::new(&norm)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if is_scratch_name(&fname)
                && !lower_prompt.contains(&fname.to_lowercase())
                && ws_root.join(&norm).is_file()
                && !leftovers.contains(&norm)
            {
                leftovers.push(norm);
            }
        }

        if let Ok(entries) = fs::read_dir(ws_root) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if is_scratch_name(&fname)
                        && !lower_prompt.contains(&fname.to_lowercase())
                        && !leftovers.contains(&fname)
                    {
                        leftovers.push(fname);
                    }
                }
            }
        }
        leftovers
    }

    pub fn compact_active_conversation(&self) {
        if let Ok(mut g) = self.state.write() {
            let active_id = g.active_conversation_id.clone();
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == active_id) {
                if conv.messages.len() > 3 {
                    conv.messages = Self::compact_messages(&conv.messages, 3);
                }
            }
        }
        self.save_state();
    }

    pub fn subagents(&self) -> Vec<SubagentDef> {
        self.state
            .read()
            .map(|g| g.subagents.clone())
            .unwrap_or_default()
    }

    pub fn add_subagent(&self, name: &str, role: &str, system_prompt: &str, model: &str) {
        let _ = self.upsert_subagent(None, name, role, system_prompt, model, Vec::new());
    }

    pub fn upsert_subagent(
        &self,
        id_opt: Option<&str>,
        name: &str,
        role: &str,
        system_prompt: &str,
        model: &str,
        tools: Vec<String>,
    ) -> Option<SubagentDef> {
        if name.trim().is_empty() {
            return None;
        }
        let clean_model = if model.trim().is_empty() {
            "inherit".to_string()
        } else {
            model.trim().to_string()
        };
        let mut result_sub = None;
        if let Ok(mut g) = self.state.write() {
            let active_cid = g.active_conversation_id.clone();
            if let Some(target_id) = id_opt.filter(|s| !s.trim().is_empty()) {
                if let Some(existing) = g.subagents.iter_mut().find(|s| s.id == target_id) {
                    existing.name = name.trim().to_string();
                    existing.role = role.trim().to_string();
                    existing.system_prompt = system_prompt.trim().to_string();
                    existing.model = clean_model.clone();
                    if !tools.is_empty() {
                        existing.tools = tools.clone();
                    }
                    result_sub = Some(existing.clone());
                }
            }
            if result_sub.is_none() {
                let id = id_opt
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| format!("sub-{}", &Uuid::new_v4().to_string()[..8]));
                let sub = SubagentDef {
                    id,
                    name: name.trim().to_string(),
                    role: role.trim().to_string(),
                    system_prompt: system_prompt.trim().to_string(),
                    model: clean_model,
                    status: "idle".to_string(),
                    last_output: String::new(),
                    output: String::new(),
                    prompt: String::new(),
                    result: String::new(),
                    text: String::new(),
                    conv_id: active_cid,
                    current_task: String::new(),
                    tools,
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    logs: Vec::new(),
                    total_tokens: 0,
                    total_cost_usd: 0.0,
                };
                g.subagents.push(sub.clone());
                result_sub = Some(sub);
            }
        }
        self.save_state();
        result_sub
    }

    pub fn abort_subagent(&self, subagent_id: &str) {
        if let Ok(mut g) = self.state.write() {
            if let Some(sub) = g
                .subagents
                .iter_mut()
                .find(|s| s.id == subagent_id || s.name.eq_ignore_ascii_case(subagent_id))
            {
                sub.status = "aborted".to_string();
                sub.last_output = "Subagent aborted by user.".to_string();
                sub.output = sub.last_output.clone();
                sub.result = sub.last_output.clone();
                sub.text = sub.last_output.clone();
                sub.logs.push(SubagentLogEntry {
                    time: Utc::now().format("%H:%M:%S").to_string(),
                    event: "Aborted by user".to_string(),
                });
            }
        }
        self.save_state();
    }

    /// Executes a multi-turn tool-enabled subagent task and returns its final output string.
    pub async fn execute_subagent_task(&self, subagent_id: &str, task: &str) -> Result<String, String> {
        self.execute_subagent_task_detailed(subagent_id, task, None)
            .await
            .map(|r| r.output)
    }

    /// Executes a multi-turn tool-enabled subagent task with full abort, budget, fallback,
    /// stuck-loop protection, tool-allowlist enforcement, and diff/token/cost rollup.
    pub async fn execute_subagent_task_detailed(
        &self,
        subagent_id: &str,
        task: &str,
        parent_conv_id: Option<&str>,
    ) -> Result<SubagentTurnResult, String> {
        let mut settings = self.settings();
        let ws_path = self.active_workspace_path();
        let workspace = Workspace::new(&ws_path).map_err(|e| e.to_string())?;
        let mut tool_engine =
            ToolEngine::new(workspace).with_abort_flag(Arc::clone(&self.abort_flag));

        let mut resolved_id = if subagent_id.trim().is_empty() {
            "sub-coder".to_string()
        } else {
            subagent_id.trim().to_string()
        };
        let mut allowed_tools: Vec<String> = Vec::new();
        let mut sys_prompt = format!(
            "You are a specialized Unjess autonomous subagent operating in workspace {}. Use tools to inspect and complete your assigned task.",
            ws_path.display()
        );

        let target_conv_id = parent_conv_id
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.active_conversation_id());

        if let Ok(mut g) = self.state.write() {
            if !g.subagents.iter().any(|s| {
                s.id.eq_ignore_ascii_case(&resolved_id) || s.name.eq_ignore_ascii_case(&resolved_id)
            }) {
                g.subagents.push(SubagentDef {
                    id: resolved_id.clone(),
                    name: resolved_id.clone(),
                    role: "Dynamic Autonomous Subagent".to_string(),
                    system_prompt: sys_prompt.clone(),
                    model: "inherit".to_string(),
                    status: "idle".to_string(),
                    last_output: String::new(),
                    output: String::new(),
                    prompt: String::new(),
                    result: String::new(),
                    text: String::new(),
                    conv_id: target_conv_id.clone(),
                    current_task: String::new(),
                    tools: Vec::new(),
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    logs: Vec::new(),
                    total_tokens: 0,
                    total_cost_usd: 0.0,
                });
            }
            if let Some(sub) = g.subagents.iter_mut().find(|s| {
                s.id.eq_ignore_ascii_case(&resolved_id) || s.name.eq_ignore_ascii_case(&resolved_id)
            }) {
                resolved_id = sub.id.clone();
                allowed_tools = sub.tools.clone();
                sub.status = "running".to_string();
                sub.conv_id = target_conv_id.clone();
                sub.current_task = task.to_string();
                sub.prompt = task.to_string();
                sub.last_output = format!("Executing task: {}...", task);
                sub.output = sub.last_output.clone();
                sub.logs.push(SubagentLogEntry {
                    time: Utc::now().format("%H:%M:%S").to_string(),
                    event: format!("Started task: {}", task),
                });
                sys_prompt = format!(
                    "{}\nRole: {}\nWorkspace: {}\nAllowed Tools: {}",
                    sub.system_prompt,
                    sub.role,
                    ws_path.display(),
                    if sub.tools.is_empty() {
                        "all".to_string()
                    } else {
                        sub.tools.join(", ")
                    }
                );
                if sub.model != "inherit" && !sub.model.is_empty() {
                    settings.model = sub.model.clone();
                } else if settings.subagent_model != "inherit" && !settings.subagent_model.is_empty()
                {
                    settings.model = settings.subagent_model.clone();
                }
            }
        }
        self.save_state();

        let max_turns = settings.subagent_max_turns.clamp(1, 100);
        let inflight_parent_cost = {
            let ls = self.live_turn_snapshot();
            if ls.running && ls.conv_id == target_conv_id {
                ls.cost_usd
            } else {
                0.0
            }
        };
        let initial_session_cost =
            self.conversation_by_id(&target_conv_id).total_cost_usd + inflight_parent_cost;
        let mut history = vec![ChatMessage {
            role: "user".to_string(),
            content: task.to_string(),
            thinking: None,
            tool_calls: Vec::new(),
            diffs: Vec::new(),
            timestamp: Utc::now().format("%H:%M:%S").to_string(),
            model: None,
            tokens: task.len() / 4,
            input_tokens: task.len() / 4,
            output_tokens: 0,
            cost_usd: 0.0,
        }];

        let mut final_output = String::new();
        let mut status = "completed".to_string();
        let mut sub_diffs = Vec::new();
        let mut sub_tools = Vec::new();
        let mut sub_tokens = 0usize;
        let mut sub_in_tokens = 0usize;
        let mut sub_out_tokens = 0usize;
        let mut sub_cost = 0.0f64;
        let mut recent_sigs: Vec<String> = Vec::new();

        for turn in 1..=max_turns {
            if self.abort_flag.load(Ordering::SeqCst) {
                status = "aborted".to_string();
                final_output = "Subagent aborted.".to_string();
                break;
            }
            if let Ok(g) = self.state.read() {
                if g.subagents
                    .iter()
                    .any(|s| s.id == resolved_id && s.status == "aborted")
                {
                    status = "aborted".to_string();
                    final_output = "Subagent aborted by user.".to_string();
                    break;
                }
            }
            if settings.max_cost_per_session > 0.0
                && (initial_session_cost + sub_cost) >= settings.max_cost_per_session
            {
                final_output = format!(
                    "Subagent stopped: session budget limit (${:.4}) reached.",
                    settings.max_cost_per_session
                );
                break;
            }

            if settings.enable_compaction {
                history = Self::compact_messages_for_model(&history, &settings.model);
            }

            let allowed_slice = if allowed_tools.is_empty() {
                None
            } else {
                Some(allowed_tools.as_slice())
            };
            let mut llm_res = complete_llm_turn_filtered(
                &settings,
                &sys_prompt,
                &history,
                true,
                allowed_slice,
            )
            .await;
            if llm_res.is_err() && !settings.fallback_models.is_empty() {
                for fb_model in settings.fallback_models.clone() {
                    if fb_model.trim().is_empty() || fb_model == settings.model {
                        continue;
                    }
                    let mut fb_settings = settings.clone();
                    fb_settings.model = fb_model.trim().to_string();
                    if let Ok(ok_resp) = complete_llm_turn_filtered(
                        &fb_settings,
                        &sys_prompt,
                        &history,
                        true,
                        allowed_slice,
                    )
                    .await
                    {
                        settings.model = fb_settings.model;
                        llm_res = Ok(ok_resp);
                        break;
                    }
                }
            }

            match llm_res {
                Ok(mut resp) => {
                    sub_tokens += resp.tokens;
                    sub_in_tokens += resp.input_tokens;
                    sub_out_tokens += resp.output_tokens;
                    sub_cost += resp.cost_usd;

                    if resp.pending_tool_calls.is_empty() {
                        let fallback_calls = Self::extract_tool_calls_from_text(&resp.content);
                        if !fallback_calls.is_empty() {
                            resp.pending_tool_calls = fallback_calls;
                        }
                    }

                    if !resp.content.is_empty() {
                        final_output = resp.content.clone();
                    }
                    if resp.pending_tool_calls.is_empty() {
                        break;
                    }
                    let mut tool_outputs = Vec::new();
                    let mut step_tools = Vec::new();
                    let mut stuck = false;
                    for (tc_id, tc_name, tc_args) in resp.pending_tool_calls {
                        if self.abort_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        if tc_name == "delegate_subagent" || tc_name == "delegate_parallel_subagents" {
                            let msg = "[Tool `delegate_subagent` result]: Recursive subagent delegation is disabled inside subagents.".to_string();
                            let rec = ToolCallRecord {
                                id: tc_id,
                                name: tc_name,
                                arguments: tc_args,
                                result: msg.clone(),
                                status: "error".to_string(),
                                duration_ms: 0,
                            };
                            step_tools.push(rec.clone());
                            sub_tools.push(rec);
                            tool_outputs.push(msg);
                            continue;
                        }
                        if !allowed_tools.is_empty()
                            && !allowed_tools.iter().any(|t| {
                                t.eq_ignore_ascii_case(&tc_name)
                                    || (t.eq_ignore_ascii_case("call_mcp_tool")
                                        && tc_name.starts_with("mcp__"))
                            })
                        {
                            let blocked_msg = format!(
                                "[Tool `{}` blocked]: Subagent `{}` is restricted to tools: {}",
                                tc_name,
                                resolved_id,
                                allowed_tools.join(", ")
                            );
                            let rec = ToolCallRecord {
                                id: tc_id,
                                name: tc_name,
                                arguments: tc_args,
                                result: blocked_msg.clone(),
                                status: "error".to_string(),
                                duration_ms: 0,
                            };
                            step_tools.push(rec.clone());
                            sub_tools.push(rec);
                            tool_outputs.push(blocked_msg);
                            continue;
                        }
                        let sig = Self::canonicalize_tool_call_key(&tc_name, &tc_args);
                        recent_sigs.push(sig.clone());
                        let slen = recent_sigs.len();
                        if slen >= 3
                            && recent_sigs[slen - 1] == sig
                            && recent_sigs[slen - 2] == sig
                            && recent_sigs[slen - 3] == sig
                        {
                            stuck = true;
                            break;
                        }

                        if let Ok(mut g) = self.state.write() {
                            if let Some(sub) =
                                g.subagents.iter_mut().find(|s| s.id == resolved_id)
                            {
                                sub.logs.push(SubagentLogEntry {
                                    time: Utc::now().format("%H:%M:%S").to_string(),
                                    event: format!("Turn {}: running `{}`", turn, tc_name),
                                });
                            }
                        }

                        let started = Instant::now();
                        let (res_str, is_ok, diff_opt) =
                            tool_engine.execute_tool(&tc_name, &tc_args);
                        let duration_ms = started.elapsed().as_millis() as u64;
                        let clipped = ToolEngine::truncate_head_tail(&res_str, 10000);
                        if let Some(d) = diff_opt {
                            sub_diffs.push(d);
                        }
                        let rec = ToolCallRecord {
                            id: tc_id,
                            name: tc_name.clone(),
                            arguments: tc_args,
                            result: clipped.clone(),
                            status: if is_ok {
                                "ok".to_string()
                            } else {
                                "error".to_string()
                            },
                            duration_ms,
                        };
                        step_tools.push(rec.clone());
                        sub_tools.push(rec);
                        tool_outputs.push(format!("[Tool `{}` result]:\n{}", tc_name, clipped));
                    }
                    history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: resp.content.clone(),
                        thinking: resp.thinking.clone(),
                        tool_calls: step_tools.clone(),
                        diffs: Vec::new(),
                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                        model: Some(settings.model.clone()),
                        tokens: resp.tokens,
                        input_tokens: resp.input_tokens,
                        output_tokens: resp.output_tokens,
                        cost_usd: resp.cost_usd,
                    });
                    history.push(ChatMessage {
                        role: "tool".to_string(),
                        content: tool_outputs.join("\n\n"),
                        thinking: None,
                        tool_calls: step_tools,
                        diffs: Vec::new(),
                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                        model: None,
                        tokens: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                    });
                    if stuck {
                        break;
                    }
                }
                Err(e) => {
                    status = "error".to_string();
                    final_output = format!("Subagent error: {}", e);
                    break;
                }
            }
        }

        if let Ok(mut g) = self.state.write() {
            if let Some(sub) = g.subagents.iter_mut().find(|s| s.id == resolved_id) {
                sub.status = status.clone();
                sub.prompt = task.to_string();
                sub.last_output = final_output.clone();
                sub.output = final_output.clone();
                sub.result = final_output.clone();
                sub.text = final_output.clone();
                sub.tool_calls = sub_tools.clone();
                sub.diffs = sub_diffs.clone();
                sub.total_tokens += sub_tokens;
                sub.total_cost_usd += sub_cost;
                sub.logs.push(SubagentLogEntry {
                    time: Utc::now().format("%H:%M:%S").to_string(),
                    event: format!(
                        "Finished ({}) — {} tokens, ${:.4}",
                        status, sub_tokens, sub_cost
                    ),
                });
            }
        }
        self.save_state();

        let result = SubagentTurnResult {
            output: final_output.clone(),
            diffs: sub_diffs,
            tool_calls: sub_tools,
            tokens: sub_tokens,
            input_tokens: sub_in_tokens,
            output_tokens: sub_out_tokens,
            cost_usd: sub_cost,
        };

        if status == "error" {
            Err(final_output)
        } else {
            Ok(result)
        }
    }

    pub fn spawn_subagent_task(self: &Arc<Self>, subagent_id: &str, task: &str) {
        self.spawn_subagent_task_with_conv(subagent_id, task, None);
    }

    pub fn spawn_subagent_task_with_conv(
        self: &Arc<Self>,
        subagent_id: &str,
        task: &str,
        conv_id: Option<&str>,
    ) {
        let sub_id = subagent_id.to_string();
        let task_str = task.to_string();
        let cid = conv_id
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.active_conversation_id());
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            if let Ok(res) = mgr
                .execute_subagent_task_detailed(&sub_id, &task_str, Some(&cid))
                .await
            {
                let active_model = mgr.settings().model;
                if let Ok(mut g) = mgr.state.write() {
                    if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == cid) {
                        conv.total_tokens += res.tokens;
                        conv.total_input_tokens += res.input_tokens;
                        conv.total_output_tokens += res.output_tokens;
                        conv.total_cost_usd += res.cost_usd;
                        if !res.diffs.is_empty() || !res.tool_calls.is_empty() || !res.output.is_empty() {
                            conv.messages.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: format!(
                                    "**[Subagent `{}` Completed]**:\n{}",
                                    sub_id, res.output
                                ),
                                thinking: None,
                                tool_calls: res.tool_calls,
                                diffs: res.diffs,
                                timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                model: Some(active_model),
                                tokens: res.tokens,
                                input_tokens: res.input_tokens,
                                output_tokens: res.output_tokens,
                                cost_usd: res.cost_usd,
                            });
                        }
                    }
                }
                mgr.save_state();
            }
        });
    }

    pub fn workflows(&self) -> Vec<WorkflowDef> {
        self.state
            .read()
            .map(|g| g.workflows.clone())
            .unwrap_or_default()
    }

    pub fn add_workflow(&self, name: &str, desc: &str, steps: Vec<String>) {
        self.save_workflow(WorkflowDef {
            id: String::new(),
            name: name.trim().to_string(),
            title: name.trim().to_string(),
            description: desc.trim().to_string(),
            category: "general".to_string(),
            prompt: steps.join("\n"),
            tools: Vec::new(),
            steps,
        });
    }

    pub fn save_workflow(&self, mut wf: WorkflowDef) {
        wf.normalize();
        if wf.name.trim().is_empty() {
            return;
        }
        if wf.id.trim().is_empty() {
            wf.id = format!("wf-{}", &Uuid::new_v4().to_string()[..8]);
        }
        if let Ok(mut g) = self.state.write() {
            if let Some(existing) = g.workflows.iter_mut().find(|w| w.id == wf.id) {
                *existing = wf;
            } else {
                g.workflows.push(wf);
            }
        }
        self.save_state();
    }

    pub fn delete_workflow(&self, id: &str) {
        if let Ok(mut g) = self.state.write() {
            g.workflows.retain(|w| w.id != id);
        }
        self.save_state();
    }

    pub fn delete_subagent(&self, id: &str) {
        if let Ok(mut g) = self.state.write() {
            g.subagents.retain(|s| s.id != id);
        }
        self.save_state();
    }

    pub fn undo_last_turn(&self, conv_id: &str) -> (Vec<String>, Vec<String>) {
        let mut restored = Vec::new();
        let mut deleted = Vec::new();
        let ws_root = self.active_workspace_path();
        let mut tree_snapshot_to_restore: Option<String> = None;

        if let Ok(mut g) = self.state.write() {
            let target_id = if conv_id.is_empty() {
                g.active_conversation_id.clone()
            } else {
                conv_id.to_string()
            };
            if let Some(stack) = g.tree_snapshots.get_mut(&target_id) {
                tree_snapshot_to_restore = stack.pop();
            }
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == target_id) {
                if let Some(last_assistant_idx) =
                    conv.messages.iter().rposition(|m| m.role != "user")
                {
                    let diffs = conv.messages[last_assistant_idx].diffs.clone();
                    for d in diffs.iter().rev() {
                        if ToolEngine::revert_diff(&ws_root, d).is_ok() {
                            if d.status == "created" {
                                deleted.push(d.path.clone());
                            } else {
                                restored.push(d.path.clone());
                            }
                        }
                    }
                    let cut_from = if last_assistant_idx > 0
                        && conv.messages[last_assistant_idx - 1].role == "user"
                    {
                        last_assistant_idx - 1
                    } else {
                        last_assistant_idx
                    };
                    for removed in &conv.messages[cut_from..] {
                        conv.total_tokens = conv.total_tokens.saturating_sub(removed.tokens);
                        conv.total_input_tokens =
                            conv.total_input_tokens.saturating_sub(removed.input_tokens);
                        conv.total_output_tokens = conv
                            .total_output_tokens
                            .saturating_sub(removed.output_tokens);
                        conv.total_cost_usd = (conv.total_cost_usd - removed.cost_usd).max(0.0);
                    }
                    conv.messages.truncate(cut_from);
                }
            }
        }
        if let Some(tree_hash) = tree_snapshot_to_restore {
            if let Ok(ws) = Workspace::new(&ws_root) {
                if let Ok(restored_count) = ws.restore_git_tree_snapshot(&tree_hash) {
                    if restored_count > 0 && restored.is_empty() {
                        restored.push(format!("{} git-tracked file(s)", restored_count));
                    }
                }
            }
        }
        self.save_state();
        (restored, deleted)
    }

    /// Removes the most recent `FileDiff` matching `file_path` from the specified conversation after a revert.
    pub fn remove_diff_from_conversation(&self, conv_id: &str, file_path: &str) {
        let norm_target = file_path.replace('\\', "/");
        if let Ok(mut g) = self.state.write() {
            let target_id = if conv_id.trim().is_empty() {
                g.active_conversation_id.clone()
            } else {
                conv_id.to_string()
            };
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == target_id) {
                for msg in conv.messages.iter_mut().rev() {
                    if let Some(pos) = msg
                        .diffs
                        .iter()
                        .rposition(|d| d.path.replace('\\', "/") == norm_target)
                    {
                        msg.diffs.remove(pos);
                        break;
                    }
                }
            }
        }
        self.save_state();
    }

    pub fn agent_mode(&self) -> String {
        self.state
            .read()
            .map(|g| g.agent_mode.clone())
            .unwrap_or_else(|_| "Autonomous".to_string())
    }

    pub fn set_agent_mode(&self, mode: &str) {
        let normalized = match mode.trim().to_lowercase().as_str() {
            "plan" => "Plan",
            "supervised" => "Supervised",
            "duel" => "Duel",
            _ => "Autonomous",
        };
        if let Ok(mut g) = self.state.write() {
            g.agent_mode = normalized.to_string();
        }
        self.save_state();
    }

    pub fn caveman_mode(&self) -> bool {
        self.state.read().map(|g| g.caveman_mode).unwrap_or(false)
    }

    pub fn set_caveman_mode(&self, v: bool) {
        if let Ok(mut g) = self.state.write() {
            g.caveman_mode = v;
        }
        self.save_state();
    }

    pub fn ponytail_mode(&self) -> bool {
        self.state.read().map(|g| g.ponytail_mode).unwrap_or(false)
    }

    pub fn set_ponytail_mode(&self, v: bool) {
        if let Ok(mut g) = self.state.write() {
            g.ponytail_mode = v;
        }
        self.save_state();
    }

    pub fn live_turn_snapshot(&self) -> LiveTurnSnapshot {
        self.live_turn
            .read()
            .map(|l| {
                let mut snap = l.clone();
                if snap.running && snap.started_at > 0.0 {
                    snap.elapsed_s = (Self::now_epoch_secs() - snap.started_at).max(0.0);
                }
                snap
            })
            .unwrap_or_default()
    }

    pub fn abort_turn(&self) {
        self.abort_flag.store(true, Ordering::SeqCst);
        if let Ok(mut live) = self.live_turn.write() {
            live.running = false;
            live.current_step = "Aborted by user".to_string();
        }
    }

    pub fn revert_diff(&self, diff: &FileDiff) -> Result<(), String> {
        let ws_root = self.active_workspace_path();
        ToolEngine::revert_diff(&ws_root, diff)
    }

    /// Handles built-in slash commands (`/clear`, `/compact`, `/undo`, `/caveman`, `/ponytail`,
    /// `/plan`, `/duel`, `/supervised`, `/auto`, `/goal`, `/help`) directly if matched.
    /// Returns `Some(rewritten_prompt)` if the prompt should still proceed to the LLM (e.g., `/goal <task>`),
    /// or `None` if the slash command was handled instantaneously.
    fn try_handle_slash_command(&self, conv_id: &str, raw_prompt: &str) -> Option<String> {
        let trimmed = raw_prompt.trim();
        if !trimmed.starts_with('/') {
            return Some(trimmed.to_string());
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.next().unwrap_or("").trim();

        let append_instant_reply = |user_txt: &str, reply_txt: String| {
            let now_ts = Utc::now().format("%H:%M:%S").to_string();
            if let Ok(mut g) = self.state.write() {
                if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == conv_id) {
                    conv.messages.push(ChatMessage {
                        role: "user".to_string(),
                        content: user_txt.to_string(),
                        thinking: None,
                        tool_calls: Vec::new(),
                        diffs: Vec::new(),
                        timestamp: now_ts.clone(),
                        model: None,
                        tokens: user_txt.len() / 4,
                        input_tokens: user_txt.len() / 4,
                        output_tokens: 0,
                        cost_usd: 0.0,
                    });
                    conv.messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: reply_txt,
                        thinking: None,
                        tool_calls: Vec::new(),
                        diffs: Vec::new(),
                        timestamp: now_ts,
                        model: Some(self.settings().model),
                        tokens: 16,
                        input_tokens: 0,
                        output_tokens: 16,
                        cost_usd: 0.0,
                    });
                }
            }
            self.save_state();
        };

        match cmd.as_str() {
            "/clear" => {
                self.clear_active_conversation();
                None
            }
            "/compact" => {
                self.compact_active_conversation();
                append_instant_reply(
                    trimmed,
                    "**Context Compacted**: Summarized earlier messages into a structured digest to free context window tokens.".to_string(),
                );
                None
            }
            "/undo" => {
                let (restored, deleted) = self.undo_last_turn(conv_id);
                append_instant_reply(
                    trimmed,
                    format!(
                        "**Reverted Last Turn**: Restored {} file(s), removed {} newly created file(s).",
                        restored.len(),
                        deleted.len()
                    ),
                );
                None
            }
            "/caveman" => {
                match arg.to_lowercase().as_str() {
                    "" => {
                        let next = !self.caveman_mode();
                        self.set_caveman_mode(next);
                        append_instant_reply(
                            trimmed,
                            format!(
                                "**Caveman Mode {}**.",
                                if next { "ENABLED" } else { "DISABLED" }
                            ),
                        );
                        None
                    }
                    "on" | "true" | "1" => {
                        self.set_caveman_mode(true);
                        append_instant_reply(trimmed, "**Caveman Mode ENABLED**.".to_string());
                        None
                    }
                    "off" | "false" | "0" => {
                        self.set_caveman_mode(false);
                        append_instant_reply(trimmed, "**Caveman Mode DISABLED**.".to_string());
                        None
                    }
                    _ => {
                        self.set_caveman_mode(true);
                        Some(arg.to_string())
                    }
                }
            }
            "/ponytail" => {
                match arg.to_lowercase().as_str() {
                    "" => {
                        let next = !self.ponytail_mode();
                        self.set_ponytail_mode(next);
                        append_instant_reply(
                            trimmed,
                            format!(
                                "**Ponytail Mode {}**.",
                                if next { "ENABLED" } else { "DISABLED" }
                            ),
                        );
                        None
                    }
                    "on" | "true" | "1" => {
                        self.set_ponytail_mode(true);
                        append_instant_reply(trimmed, "**Ponytail Mode ENABLED**.".to_string());
                        None
                    }
                    "off" | "false" | "0" => {
                        self.set_ponytail_mode(false);
                        append_instant_reply(trimmed, "**Ponytail Mode DISABLED**.".to_string());
                        None
                    }
                    _ => {
                        self.set_ponytail_mode(true);
                        Some(arg.to_string())
                    }
                }
            }
            "/model" => {
                if arg.is_empty() {
                    let s = self.settings();
                    let prov = crate::llm::resolve_provider_for_model(&s, &s.model);
                    append_instant_reply(
                        trimmed,
                        format!("**Active Model**: `{}` (provider: `{}`)", s.model, prov),
                    );
                } else {
                    let mut s = self.settings();
                    s.model = arg.to_string();
                    s.sync_provider_with_model();
                    let prov = s.provider.clone();
                    self.update_settings(s);
                    append_instant_reply(
                        trimmed,
                        format!("Switched active model to **`{}`** (provider: `{}`).", arg, prov),
                    );
                }
                None
            }
            "/status" => {
                let s = self.settings();
                let conv = self.conversation_by_id(conv_id);
                let ws = self.active_workspace_path();
                append_instant_reply(
                    trimmed,
                    format!(
                        "### Unjess 4.0 Session Status\n\
                         - **Workspace**: `{}`\n\
                         - **Model**: `{}` (`{}`)\n\
                         - **Mode**: `{}` (Caveman: `{}`, Ponytail: `{}`)\n\
                         - **Session Tokens**: `{}` (in: `{}`, out: `{}`)\n\
                         - **Session Cost**: `${:.4}`",
                        ws.display(),
                        s.model,
                        s.provider,
                        self.agent_mode(),
                        self.caveman_mode(),
                        self.ponytail_mode(),
                        conv.total_tokens,
                        conv.total_input_tokens,
                        conv.total_output_tokens,
                        conv.total_cost_usd
                    ),
                );
                None
            }
            "/skill" => {
                if arg.is_empty() {
                    let ws = self.active_workspace_path();
                    let list: Vec<String> = SkillsManager::list_skills_for_workspace(Some(&ws))
                        .into_iter()
                        .map(|sk| format!("- `{}` ({}): {}", sk.id, sk.name, sk.description))
                        .collect();
                    append_instant_reply(
                        trimmed,
                        format!("**Installed Skills**:\n{}", list.join("\n")),
                    );
                    None
                } else {
                    Some(format!("/skill {}", arg))
                }
            }
            "/plan" => {
                self.set_agent_mode("Plan");
                if arg.is_empty() {
                    append_instant_reply(trimmed, "Switched execution mode to **Plan**.".to_string());
                    None
                } else {
                    Some(arg.to_string())
                }
            }
            "/supervised" => {
                self.set_agent_mode("Supervised");
                if arg.is_empty() {
                    append_instant_reply(
                        trimmed,
                        "Switched execution mode to **Supervised**.".to_string(),
                    );
                    None
                } else {
                    Some(arg.to_string())
                }
            }
            "/auto" | "/autonomous" => {
                self.set_agent_mode("Autonomous");
                if arg.is_empty() {
                    append_instant_reply(
                        trimmed,
                        "Switched execution mode to **Autonomous**.".to_string(),
                    );
                    None
                } else {
                    Some(arg.to_string())
                }
            }
            "/mcp" => {
                let servers = McpManager::load();
                let mut md = String::from("### Configured MCP Servers\n");
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
                    md.push_str(&format!(
                        "- **{}** (`{}`): {} — **{} tools** {}\n",
                        s.name,
                        status,
                        s.description,
                        tool_names.len(),
                        if tool_names.is_empty() {
                            String::new()
                        } else {
                            format!("(`[{}]`) ", tool_names.join(", "))
                        }
                    ));
                }
                append_instant_reply(trimmed, md);
                None
            }
            "/duel" => {
                self.set_agent_mode("Duel");
                if arg.is_empty() {
                    let ws = self.active_workspace_path();
                    let report = scan_workspace(&ws);
                    let mut md = format!(
                        "### Security Duel Scan — Score: **{}/100**\n{}\n",
                        report.score, report.summary
                    );
                    for f in report.red_team_findings.iter().take(15) {
                        md.push_str(&format!(
                            "- **[{}]** `{}` (`{}:{}`) — {}\n",
                            f.severity.to_uppercase(),
                            f.title,
                            f.file,
                            f.line,
                            f.remediation
                        ));
                    }
                    append_instant_reply(trimmed, md);
                    None
                } else {
                    Some(arg.to_string())
                }
            }
            "/goal" => {
                self.set_agent_mode("Autonomous");
                if arg.is_empty() {
                    append_instant_reply(
                        trimmed,
                        "Usage: `/goal <objective>` — Runs a thorough multi-step autonomous engineering loop until the objective is verified.".to_string(),
                    );
                    None
                } else {
                    Some(format!(
                        "[AUTONOMOUS GOAL MODE]: Complete the following goal thoroughly end-to-end, verifying all changes with tests before finishing:\n{}",
                        arg
                    ))
                }
            }
            "/budget" | "/cost" => {
                let mut s = self.settings();
                let conv = self.conversation_by_id(conv_id);
                if cmd == "/cost" || arg.is_empty() {
                    let limit_str = if s.max_cost_per_session > 0.0 {
                        format!("${:.4}", s.max_cost_per_session)
                    } else {
                        "unlimited".to_string()
                    };
                    append_instant_reply(
                        trimmed,
                        format!(
                            "### Session Cost & Budget\n\
                             - **Session Tokens**: `{}` (in: `{}`, out: `{}`)\n\
                             - **Session Cost**: `${:.4}`\n\
                             - **Session Budget Limit**: `{}`",
                            conv.total_tokens,
                            conv.total_input_tokens,
                            conv.total_output_tokens,
                            conv.total_cost_usd,
                            limit_str
                        ),
                    );
                } else if matches!(arg.to_lowercase().as_str(), "off" | "0" | "none" | "unlimited")
                {
                    s.max_cost_per_session = 0.0;
                    self.update_settings(s);
                    append_instant_reply(
                        trimmed,
                        "**Session Budget Limit disabled** (`unlimited`).".to_string(),
                    );
                } else if let Ok(val) = arg.trim_start_matches('$').parse::<f64>() {
                    s.max_cost_per_session = val.max(0.0);
                    self.update_settings(s.clone());
                    append_instant_reply(
                        trimmed,
                        format!(
                            "**Session Budget Limit** set to **`${:.4}`**.",
                            s.max_cost_per_session
                        ),
                    );
                } else {
                    append_instant_reply(
                        trimmed,
                        "Usage: `/budget [usd|off]` or `/cost`.".to_string(),
                    );
                }
                None
            }
            "/help" => {
                append_instant_reply(
                    trimmed,
                    "**Available Slash Commands**:\n\
                     - `/auto [prompt]` — Switch to Autonomous mode\n\
                     - `/plan [prompt]` — Switch to Plan mode\n\
                     - `/supervised [prompt]` — Switch to Supervised mode\n\
                     - `/duel [prompt]` — Run Red Team vs. White Team Security Duel\n\
                     - `/goal <objective>` — Execute a thorough end-to-end goal\n\
                     - `/model [name]` — View or switch the active LLM model\n\
                     - `/status` — Show workspace, model, mode, token, and cost telemetry\n\
                     - `/budget [usd|off]` / `/cost` — View or configure session USD budget limit\n\
                     - `/skill [id]` — List installed skills or invoke a skill\n\
                     - `/compact` — Deterministically compact conversation history\n\
                     - `/undo` — Revert the last turn and restore modified files\n\
                     - `/caveman [on|off|prompt]` — Toggle or set ultra-dense Caveman output mode\n\
                     - `/ponytail [on|off|prompt]` — Toggle or set high-energy Ponytail systems-architect mode\n\
                     - `/clear` — Clear the active conversation"
                        .to_string(),
                );
                None
            }
            _ => Some(trimmed.to_string()),
        }
    }

    /// Appends the user prompt to the active conversation and spawns the multi-step
    /// autonomous agent tool-execution loop on Tokio, updating `live_turn` in real time.
    pub fn submit_prompt(self: &Arc<Self>, prompt: String) {
        let trimmed = prompt.trim().to_string();
        if trimmed.is_empty() {
            return;
        }

        let conv_id = self.active_conversation_id();
        let Some(effective_prompt) = self.try_handle_slash_command(&conv_id, &trimmed) else {
            return;
        };

        self.abort_flag.store(false, Ordering::SeqCst);
        let now_ts = Utc::now().format("%H:%M:%S").to_string();
        let est_in = (effective_prompt.len() / 4).max(1);

        if let Ok(mut g) = self.state.write() {
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == conv_id) {
                if conv.messages.is_empty()
                    && (conv.title == "Main Engineering Session"
                        || conv.title.starts_with("Session")
                        || conv.title == "Quick Chat")
                {
                    let short: String = effective_prompt.chars().take(42).collect();
                    conv.title = short;
                }
                conv.messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: effective_prompt,
                    thinking: None,
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    timestamp: now_ts,
                    model: None,
                    tokens: est_in,
                    input_tokens: est_in,
                    output_tokens: 0,
                    cost_usd: 0.0,
                });
            }
        }
        self.save_state();

        if let Ok(mut live) = self.live_turn.write() {
            *live = LiveTurnSnapshot {
                running: true,
                conv_id: conv_id.clone(),
                current_step: "Initializing autonomous reasoning turn...".to_string(),
                thinking: String::new(),
                streaming_text: String::new(),
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                iteration: 1,
                started_at: Self::now_epoch_secs(),
                elapsed_s: 0.0,
                tokens: 0,
                cost_usd: 0.0,
            };
        }

        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.run_autonomous_loop(&conv_id).await;
        });
    }

    /// Appends the user prompt and awaits the multi-turn autonomous agent loop completion,
    /// returning the updated `Conversation` while updating `live_turn` in real time.
    pub async fn execute_prompt_turn(
        self: &Arc<Self>,
        conv_id_opt: Option<&str>,
        prompt: String,
    ) -> Conversation {
        let trimmed = prompt.trim().to_string();
        if let Some(cid) = conv_id_opt.filter(|s| !s.trim().is_empty()) {
            self.set_active_conversation(cid);
        }
        let conv_id = self.active_conversation_id();
        if trimmed.is_empty() {
            return self.active_conversation();
        }

        let Some(effective_prompt) = self.try_handle_slash_command(&conv_id, &trimmed) else {
            return self.active_conversation();
        };

        self.abort_flag.store(false, Ordering::SeqCst);
        let now_ts = Utc::now().format("%H:%M:%S").to_string();
        let est_in = (effective_prompt.len() / 4).max(1);

        if let Ok(mut g) = self.state.write() {
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == conv_id) {
                if conv.messages.is_empty()
                    && (conv.title == "Main Engineering Session"
                        || conv.title.starts_with("Session")
                        || conv.title == "Quick Chat")
                {
                    let short: String = effective_prompt.chars().take(42).collect();
                    conv.title = short;
                }
                conv.messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: effective_prompt,
                    thinking: None,
                    tool_calls: Vec::new(),
                    diffs: Vec::new(),
                    timestamp: now_ts,
                    model: None,
                    tokens: est_in,
                    input_tokens: est_in,
                    output_tokens: 0,
                    cost_usd: 0.0,
                });
            }
        }
        self.save_state();

        if let Ok(mut live) = self.live_turn.write() {
            *live = LiveTurnSnapshot {
                running: true,
                conv_id: conv_id.clone(),
                current_step: "Initializing autonomous reasoning turn...".to_string(),
                thinking: String::new(),
                streaming_text: String::new(),
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                iteration: 1,
                started_at: Self::now_epoch_secs(),
                elapsed_s: 0.0,
                tokens: 0,
                cost_usd: 0.0,
            };
        }

        self.run_autonomous_loop(&conv_id).await;
        self.conversation_by_id(&conv_id)
    }

    async fn run_autonomous_loop(&self, conv_id: &str) {
        let mut settings = self.settings();
        let ws_path = self.active_workspace_path();
        let workspace = Workspace::new(&ws_path).unwrap_or_else(|_| {
            Workspace::new(".").expect("Fallback workspace must resolve")
        });
        if let Some(tree_hash) = workspace.capture_git_tree_snapshot() {
            if let Ok(mut g) = self.state.write() {
                let stack = g.tree_snapshots.entry(conv_id.to_string()).or_default();
                stack.push(tree_hash);
                if stack.len() > 20 {
                    stack.remove(0);
                }
            }
        }
        let mut tool_engine =
            ToolEngine::new(workspace).with_abort_flag(Arc::clone(&self.abort_flag));

        let max_iters = settings.max_iterations.clamp(1, 600);
        let enable_tools = self.agent_mode() != "Plan";

        let active_conv = self.conversation_by_id(conv_id);
        let mut working_history = active_conv.messages.clone();
        let initial_session_cost = active_conv.total_cost_usd;
        let latest_user_query = working_history
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Extract negative file constraints from the user prompt and enforce them in ToolEngine
        let protected_paths =
            crate::tools::extract_protected_paths_from_prompt(&latest_user_query);
        if !protected_paths.is_empty() {
            tool_engine.set_protected_paths(protected_paths);
        }

        let mut collected_tools: Vec<ToolCallRecord> = Vec::new();
        let mut collected_diffs: Vec<FileDiff> = Vec::new();
        let mut collected_thinking = String::new();
        let mut final_text = String::new();
        let mut total_turn_tokens = 0usize;
        let mut total_turn_in_tokens = 0usize;
        let mut total_turn_out_tokens = 0usize;
        let mut total_turn_cost = 0.0f64;
        let mut recent_tool_signatures: Vec<String> = Vec::new();
        let mut tool_call_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut transitional_nudges = 0usize;
        let mut pre_exit_test_check_count = 0usize;
        let mut pre_exit_unverified_edit_count = 0usize;
        let mut scratch_cleanup_nudged = false;
        let mut system_prompt = self.build_system_prompt_for_query(&ws_path, &latest_user_query);
        let mut system_prompt_dirty = false;

        for iter in 1..=max_iters {
            if self.abort_flag.load(Ordering::SeqCst) {
                final_text = "*[Turn aborted by user]*".to_string();
                break;
            }

            // Session budget circuit-breaker
            if settings.max_cost_per_session > 0.0
                && (initial_session_cost + total_turn_cost) >= settings.max_cost_per_session
            {
                final_text = format!(
                    "**Session Budget Limit Reached**: Current session cost (`${:.4}`) reached `max_cost_per_session` (`${:.4}`). Increase the limit in Settings to continue.",
                    initial_session_cost + total_turn_cost,
                    settings.max_cost_per_session
                );
                break;
            }

            // Model-aware dynamic context compaction with 20% headroom buffer
            if settings.enable_compaction {
                working_history =
                    Self::compact_messages_for_model(&working_history, &settings.model);
                let total_history_chars: usize =
                    working_history.iter().map(|m| m.content.len()).sum();
                if (working_history.len() > 28 || total_history_chars > 140_000)
                    && working_history.len() > 6
                {
                    working_history = Self::compact_messages(&working_history, 6);
                }
            }

            if system_prompt_dirty {
                system_prompt = self.build_system_prompt_for_query(&ws_path, &latest_user_query);
                system_prompt_dirty = false;
            }

            if let Ok(mut live) = self.live_turn.write() {
                live.iteration = iter;
                live.tokens = total_turn_tokens;
                live.cost_usd = total_turn_cost;
                live.current_step = format!(
                    "Calling {} (step {}/{})...",
                    settings.model, iter, max_iters
                );
            }

            // Call primary model, with automatic fallback to `settings.fallback_models` if configured
            let mut turn_result =
                complete_llm_turn(&settings, &system_prompt, &working_history, enable_tools).await;
            if turn_result.is_err() && !settings.fallback_models.is_empty() {
                for fb_model in settings.fallback_models.clone() {
                    if fb_model.trim().is_empty() || fb_model == settings.model {
                        continue;
                    }
                    let mut fb_settings = settings.clone();
                    fb_settings.model = fb_model.trim().to_string();
                    if let Ok(ok_resp) = complete_llm_turn(
                        &fb_settings,
                        &system_prompt,
                        &working_history,
                        enable_tools,
                    )
                    .await
                    {
                        settings.model = fb_settings.model;
                        turn_result = Ok(ok_resp);
                        break;
                    }
                }
            }

            match turn_result {
                Ok(mut resp) => {
                    total_turn_tokens += resp.tokens;
                    total_turn_in_tokens += resp.input_tokens;
                    total_turn_out_tokens += resp.output_tokens;
                    total_turn_cost += resp.cost_usd;

                    let step_thinking = resp.thinking.clone().filter(|t| !t.is_empty());
                    if let Some(ref t) = step_thinking {
                        if !collected_thinking.is_empty() {
                            collected_thinking.push_str("\n\n");
                        }
                        collected_thinking.push_str(t);
                    }

                    // Fallback: if structured tool_calls are empty, check if the model emitted raw JSON/XML tool calls in text
                    if resp.pending_tool_calls.is_empty() && enable_tools {
                        let fallback_calls = Self::extract_tool_calls_from_text(&resp.content);
                        if !fallback_calls.is_empty() {
                            resp.pending_tool_calls = fallback_calls;
                        }
                    }

                    if !resp.content.trim().is_empty() {
                        final_text = resp.content.trim().to_string();
                    }

                    if let Ok(mut live) = self.live_turn.write() {
                        live.thinking = collected_thinking.clone();
                        live.streaming_text = final_text.clone();
                        live.tokens = total_turn_tokens;
                        live.cost_usd = total_turn_cost;
                    }

                    if resp.pending_tool_calls.is_empty() {
                        let lower_resp = resp.content.to_lowercase();
                        let is_explicit_completion = [
                            "task is complete",
                            "task is completed",
                            "all requirements have been met",
                            "i have completed",
                            "here is the final",
                            "completed the task",
                            "all tests pass",
                            "finished the implementation",
                            "work is complete",
                            "successfully completed",
                            "<!-- goal_complete -->",
                        ]
                        .iter()
                        .any(|m| lower_resp.contains(m));

                        let is_user_question = resp.content.trim().ends_with('?')
                            || lower_resp.contains("could you clarify")
                            || lower_resp.contains("please provide");

                        // 1. Transitional narration auto-continuation & empty turn recovery (up to 5 nudges per phase)
                        if enable_tools
                            && transitional_nudges < 5
                            && iter < max_iters
                            && !is_user_question
                            && !is_explicit_completion
                        {
                            let is_empty_turn = resp.content.trim().is_empty()
                                && step_thinking
                                    .as_ref()
                                    .map_or(false, |t| !t.trim().is_empty());
                            let is_transitional = Self::is_transitional_narration(&resp.content)
                                || !is_explicit_completion
                                || (!resp.content.contains("<!-- GOAL_COMPLETE -->")
                                    && resp.content.len() < 800
                                    && (lower_resp.contains("start by")
                                        || lower_resp.contains("proceed to")
                                        || lower_resp.contains("next step")
                                        || lower_resp.contains("continue by")
                                        || lower_resp.contains("now let")
                                        || lower_resp.contains("validate")
                                        || lower_resp.contains("inspect")
                                        || lower_resp.contains("examine")
                                        || lower_resp.contains("double check")
                                        || lower_resp.contains("need to")
                                        || lower_resp.contains("will now")
                                        || lower_resp.contains("let me")
                                        || lower_resp.contains("let's")));

                            if is_transitional || is_empty_turn {
                                transitional_nudges += 1;
                                let nudge_msg = if is_empty_turn {
                                    "[System Guidance]: You completed your reasoning but did not invoke any tools or output a final answer. Please invoke the appropriate tool to proceed with the task or provide your final response.".to_string()
                                } else {
                                    "[System Guidance]: You provided intermediate commentary without invoking any tools. In autonomous mode, you must not narrate your intent without acting. If you need to inspect files, execute commands, or edit code, CALL THE RELEVANT TOOL IMMEDIATELY now. If you are completely finished, provide your final response summarizing what you accomplished and stating the task is complete.".to_string()
                                };
                                working_history.push(ChatMessage {
                                    role: "assistant".to_string(),
                                    content: resp.content.clone(),
                                    thinking: step_thinking,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: Some(settings.model.clone()),
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                working_history.push(ChatMessage {
                                    role: "user".to_string(),
                                    content: nudge_msg,
                                    thinking: None,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: None,
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                continue;
                            }
                        }

                        // 2. Pre-Exit Verification & Pass@k Trajectory Pivot Protocol (Ported from Go agent.go:5550-5620)
                        if enable_tools && iter < max_iters && !is_user_question {
                            // Check (a): Did the most recent test/verification command fail?
                            if pre_exit_test_check_count < 3 && !collected_diffs.is_empty() {
                                let mut has_failed_test = false;
                                let mut last_failed_output = String::new();
                                for tc in collected_tools.iter().rev() {
                                    if tc.name == "run_command" {
                                        let cmd_str = tc
                                            .arguments
                                            .get("command")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let lower_cmd = cmd_str.to_lowercase();
                                        if lower_cmd.contains("test")
                                            || lower_cmd.contains("pytest")
                                            || lower_cmd.contains("check")
                                            || lower_cmd.contains("verify")
                                            || lower_cmd.contains("cargo test")
                                            || lower_cmd.contains("npm test")
                                        {
                                            let res_str = &tc.result;
                                            if tc.status == "error"
                                                || res_str.contains("FAILED")
                                                || res_str.contains("AssertionError")
                                                || res_str.contains("FAIL:")
                                                || res_str.contains("[FAIL]")
                                                || res_str.contains("error:")
                                            {
                                                has_failed_test = true;
                                                last_failed_output = res_str.clone();
                                            }
                                            break;
                                        }
                                    }
                                }

                                if has_failed_test {
                                    pre_exit_test_check_count += 1;
                                    let pivot_advice = if pre_exit_test_check_count >= 2 {
                                        "\n\n<trajectory_pivot_recovery>\n[Pass@k Trajectory Pivot Activated]\nYour previous fix attempt still left tests failing. Do NOT repeat minor surface tweaks on a flawed hypothesis:\n1. Re-read the exact failing test and traceback above.\n2. Identify what assumption in your current approach is wrong.\n3. Implement a clean alternative root-cause fix.\n</trajectory_pivot_recovery>"
                                    } else {
                                        ""
                                    };
                                    let max_snip = if last_failed_output.len() > 1500 {
                                        &last_failed_output[..1500]
                                    } else {
                                        &last_failed_output
                                    };
                                    let rejection_msg = format!(
                                        "<pre_exit_verification_failure>\n[Pre-Exit Test Verification FAILED (Attempt {}/3)]\nYour last verification/test command failed with errors:\n{}\nPlease resolve the root cause and re-run verification before declaring task completion.{}\n</pre_exit_verification_failure>",
                                        pre_exit_test_check_count, max_snip, pivot_advice
                                    );
                                    working_history.push(ChatMessage {
                                        role: "assistant".to_string(),
                                        content: resp.content.clone(),
                                        thinking: step_thinking,
                                        tool_calls: Vec::new(),
                                        diffs: Vec::new(),
                                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                        model: Some(settings.model.clone()),
                                        tokens: 0,
                                        input_tokens: 0,
                                        output_tokens: 0,
                                        cost_usd: 0.0,
                                    });
                                    working_history.push(ChatMessage {
                                        role: "user".to_string(),
                                        content: rejection_msg,
                                        thinking: None,
                                        tool_calls: Vec::new(),
                                        diffs: Vec::new(),
                                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                        model: None,
                                        tokens: 0,
                                        input_tokens: 0,
                                        output_tokens: 0,
                                        cost_usd: 0.0,
                                    });
                                    continue;
                                }
                            }

                            // Check (b): Pre-Exit Unverified Edit Guard
                            // If files were created or modified after the last run_command, do not allow exit without running verification!
                            let last_edit_idx = collected_tools.iter().rposition(|tc| {
                                tc.name == "write_file"
                                    || tc.name == "edit_file"
                                    || tc.name == "apply_patch"
                            });
                            let last_run_cmd_idx = collected_tools
                                .iter()
                                .rposition(|tc| tc.name == "run_command");

                            let edits_unverified = match (last_edit_idx, last_run_cmd_idx) {
                                (Some(e_idx), Some(c_idx)) => e_idx > c_idx,
                                (Some(_), None) => true,
                                (None, _) => false,
                            };

                            if pre_exit_unverified_edit_count < 2
                                && edits_unverified
                                && !is_explicit_completion
                            {
                                pre_exit_unverified_edit_count += 1;
                                let rejection_msg = "<pre_exit_verification_failure>\n[Pre-Exit Unverified Edit Guard]\nYou modified or created source files after your last command execution (or have not executed a verification command yet). Do not assume code works just because it was edited or generated — execute a verification/test command via `run_command` to confirm all tests and tolerance thresholds pass before concluding.\n</pre_exit_verification_failure>".to_string();
                                working_history.push(ChatMessage {
                                    role: "assistant".to_string(),
                                    content: resp.content.clone(),
                                    thinking: step_thinking,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: Some(settings.model.clone()),
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                working_history.push(ChatMessage {
                                    role: "user".to_string(),
                                    content: rejection_msg,
                                    thinking: None,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: None,
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                continue;
                            }
                        }

                        // 3. Pre-exit leftover scratch file detection
                        if enable_tools && !scratch_cleanup_nudged && iter < max_iters {
                            let created_paths: Vec<String> = collected_diffs
                                .iter()
                                .filter(|d| d.status == "created")
                                .map(|d| d.path.clone())
                                .collect();
                            let leftovers = Self::detect_leftover_scratch_files(
                                &ws_path,
                                &created_paths,
                                &latest_user_query,
                            );
                            if !leftovers.is_empty() {
                                scratch_cleanup_nudged = true;
                                working_history.push(ChatMessage {
                                    role: "assistant".to_string(),
                                    content: resp.content.clone(),
                                    thinking: step_thinking,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: Some(settings.model.clone()),
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                working_history.push(ChatMessage {
                                    role: "user".to_string(),
                                    content: format!(
                                        "[System Pre-Exit Check]: Temporary scratch/debug file(s) were left in the workspace: `{}`. Please remove them with `delete_file` before concluding.",
                                        leftovers.join("`, `")
                                    ),
                                    thinking: None,
                                    tool_calls: Vec::new(),
                                    diffs: Vec::new(),
                                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                                    model: None,
                                    tokens: 0,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cost_usd: 0.0,
                                });
                                continue;
                            }
                        }

                        break;
                    }

                    // Execute requested tool calls
                    transitional_nudges = 0;
                    let mut tool_results_summary = Vec::new();
                    let mut step_tools: Vec<ToolCallRecord> = Vec::new();
                    let mut hard_circuit_break = false;

                    for (tc_id, tc_name, tc_args) in resp.pending_tool_calls {
                        if self.abort_flag.load(Ordering::SeqCst) {
                            break;
                        }

                        // Bookkeeping-transparent canonical stuck-loop detection with escalating [3, 5, 8] <mentor_analysis>
                        let mut mentor_reminder: Option<String> = None;
                        if !Self::is_transparent_bookkeeping_tool(&tc_name) {
                            let canonical_sig = Self::canonicalize_tool_call_key(&tc_name, &tc_args);
                            recent_tool_signatures.push(canonical_sig.clone());
                            let count = tool_call_counts
                                .entry(canonical_sig.clone())
                                .and_modify(|c| *c += 1)
                                .or_insert(1);
                            let repeat_count = *count;

                            let sig_len = recent_tool_signatures.len();
                            let ping_pong = sig_len >= 6
                                && recent_tool_signatures[sig_len - 1]
                                    == recent_tool_signatures[sig_len - 3]
                                && recent_tool_signatures[sig_len - 3]
                                    == recent_tool_signatures[sig_len - 5]
                                && recent_tool_signatures[sig_len - 2]
                                    == recent_tool_signatures[sig_len - 4]
                                && recent_tool_signatures[sig_len - 4]
                                    == recent_tool_signatures[sig_len - 6];

                            if repeat_count >= 8 {
                                if let Some(msg) = Self::format_repeat_tool_reminder(&tc_name, repeat_count) {
                                    tool_results_summary.push(msg.clone());
                                    step_tools.push(ToolCallRecord {
                                        id: tc_id,
                                        name: tc_name,
                                        arguments: tc_args,
                                        result: msg,
                                        status: "error".to_string(),
                                        duration_ms: 0,
                                    });
                                }
                                hard_circuit_break = true;
                                break;
                            } else if let Some(msg) = Self::format_repeat_tool_reminder(&tc_name, repeat_count) {
                                mentor_reminder = Some(msg);
                            } else if ping_pong {
                                mentor_reminder = Some(format!(
                                    "<mentor_analysis>\nAlternating ping-pong tool cycle detected involving `{}`. Break out of this cycle and try a different approach.\n</mentor_analysis>",
                                    tc_name
                                ));
                            }
                        }

                        if let Ok(mut live) = self.live_turn.write() {
                            live.current_step = format!("Executing tool `{}`...", tc_name);
                        }

                        if matches!(
                            tc_name.as_str(),
                            "write_file"
                                | "edit_file"
                                | "multi_edit_file"
                                | "apply_patch"
                                | "ast_edit_symbol"
                                | "ast_rename_symbol"
                                | "delete_file"
                                | "save_memory"
                                | "delete_memory"
                                | "create_skill_plugin"
                                | "register_mcp_plugin"
                                | "synthesize_tool"
                                | "delegate_subagent"
                                | "delegate_parallel_subagents"
                        ) {
                            system_prompt_dirty = true;
                        }

                        let started = Instant::now();
                        let (result_str, is_ok, diff_opt) = if tc_name == "delegate_subagent" {
                            let sub_id = tc_args
                                .get("subagent_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("sub-coder");
                            let sub_task = tc_args
                                .get("task")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            match self
                                .execute_subagent_task_detailed(sub_id, sub_task, Some(conv_id))
                                .await
                            {
                                Ok(sub_res) => {
                                    total_turn_tokens += sub_res.tokens;
                                    total_turn_in_tokens += sub_res.input_tokens;
                                    total_turn_out_tokens += sub_res.output_tokens;
                                    total_turn_cost += sub_res.cost_usd;
                                    for mut tc in sub_res.tool_calls {
                                        tc.name = format!("{}:{}", sub_id, tc.name);
                                        collected_tools.push(tc);
                                    }
                                    for d in sub_res.diffs {
                                        collected_diffs.push(d);
                                    }
                                    (sub_res.output, true, None)
                                }
                                Err(e) => (e, false, None),
                            }
                        } else if tc_name == "delegate_parallel_subagents" {
                            let specs: Vec<(String, String)> = tc_args
                                .get("tasks")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .take(4)
                                        .filter_map(|item| {
                                            let sid = item
                                                .get("subagent_id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("sub-coder")
                                                .to_string();
                                            let t = item
                                                .get("task")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .trim()
                                                .to_string();
                                            if t.is_empty() {
                                                None
                                            } else {
                                                Some((sid, t))
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            if specs.is_empty() {
                                (
                                    "Missing non-empty `tasks` array for `delegate_parallel_subagents`."
                                        .to_string(),
                                    false,
                                    None,
                                )
                            } else {
                                let results = match specs.len() {
                                    1 => {
                                        let r0 = self
                                            .execute_subagent_task_detailed(
                                                &specs[0].0,
                                                &specs[0].1,
                                                Some(conv_id),
                                            )
                                            .await;
                                        vec![(specs[0].0.clone(), r0)]
                                    }
                                    2 => {
                                        let (r0, r1) = tokio::join!(
                                            self.execute_subagent_task_detailed(
                                                &specs[0].0,
                                                &specs[0].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[1].0,
                                                &specs[1].1,
                                                Some(conv_id)
                                            ),
                                        );
                                        vec![(specs[0].0.clone(), r0), (specs[1].0.clone(), r1)]
                                    }
                                    3 => {
                                        let (r0, r1, r2) = tokio::join!(
                                            self.execute_subagent_task_detailed(
                                                &specs[0].0,
                                                &specs[0].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[1].0,
                                                &specs[1].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[2].0,
                                                &specs[2].1,
                                                Some(conv_id)
                                            ),
                                        );
                                        vec![
                                            (specs[0].0.clone(), r0),
                                            (specs[1].0.clone(), r1),
                                            (specs[2].0.clone(), r2),
                                        ]
                                    }
                                    _ => {
                                        let (r0, r1, r2, r3) = tokio::join!(
                                            self.execute_subagent_task_detailed(
                                                &specs[0].0,
                                                &specs[0].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[1].0,
                                                &specs[1].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[2].0,
                                                &specs[2].1,
                                                Some(conv_id)
                                            ),
                                            self.execute_subagent_task_detailed(
                                                &specs[3].0,
                                                &specs[3].1,
                                                Some(conv_id)
                                            ),
                                        );
                                        vec![
                                            (specs[0].0.clone(), r0),
                                            (specs[1].0.clone(), r1),
                                            (specs[2].0.clone(), r2),
                                            (specs[3].0.clone(), r3),
                                        ]
                                    }
                                };

                                let mut sections = Vec::new();
                                let mut any_ok = false;
                                for (sid, res) in results {
                                    match res {
                                        Ok(sub_res) => {
                                            any_ok = true;
                                            total_turn_tokens += sub_res.tokens;
                                            total_turn_in_tokens += sub_res.input_tokens;
                                            total_turn_out_tokens += sub_res.output_tokens;
                                            total_turn_cost += sub_res.cost_usd;
                                            for mut tc in sub_res.tool_calls {
                                                tc.name = format!("{}:{}", sid, tc.name);
                                                collected_tools.push(tc);
                                            }
                                            for d in sub_res.diffs {
                                                collected_diffs.push(d);
                                            }
                                            sections.push(format!(
                                                "### Subagent `{}`\n{}",
                                                sid, sub_res.output
                                            ));
                                        }
                                        Err(e) => {
                                            sections.push(format!(
                                                "### Subagent `{}` (Error)\n{}",
                                                sid, e
                                            ));
                                        }
                                    }
                                }
                                (sections.join("\n\n"), any_ok, None)
                            }
                        } else {
                            tool_engine.execute_tool(&tc_name, &tc_args)
                        };
                        let duration_ms = started.elapsed().as_millis() as u64;
                        let mut clipped_result = ToolEngine::truncate_head_tail(&result_str, 12000);
                        if let Some(rem) = mentor_reminder {
                            clipped_result.push_str("\n\n");
                            clipped_result.push_str(&rem);
                        }

                        let record = ToolCallRecord {
                            id: tc_id,
                            name: tc_name.clone(),
                            arguments: tc_args,
                            result: clipped_result.clone(),
                            status: if is_ok {
                                "ok".to_string()
                            } else {
                                "error".to_string()
                            },
                            duration_ms,
                        };
                        step_tools.push(record.clone());
                        collected_tools.push(record);
                        if let Some(d) = diff_opt {
                            collected_diffs.push(d);
                        }

                        if let Ok(mut live) = self.live_turn.write() {
                            live.tool_calls = collected_tools.clone();
                            live.diffs = collected_diffs.clone();
                            live.tokens = total_turn_tokens;
                            live.cost_usd = total_turn_cost;
                        }

                        tool_results_summary.push(format!(
                            "[Tool `{}` result]:\n{}",
                            tc_name, clipped_result
                        ));
                    }

                    working_history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: resp.content.clone(),
                        thinking: step_thinking,
                        tool_calls: step_tools.clone(),
                        diffs: Vec::new(),
                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                        model: Some(settings.model.clone()),
                        tokens: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                    });
                    working_history.push(ChatMessage {
                        role: "tool".to_string(),
                        content: tool_results_summary.join("\n\n"),
                        thinking: None,
                        tool_calls: step_tools,
                        diffs: Vec::new(),
                        timestamp: Utc::now().format("%H:%M:%S").to_string(),
                        model: None,
                        tokens: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                    });

                    if hard_circuit_break || iter >= max_iters {
                        break;
                    }
                }
                Err(err) => {
                    final_text = format!(
                        "**LLM Provider Error (`{}`)**:\n```\n{}\n```\n\n*Tip: Verify that Ollama / llama.cpp / LM Studio is running locally or configure a cloud API key in the **Settings** tab.*",
                        settings.model, err
                    );
                    break;
                }
            }
        }

        if final_text.trim().is_empty() && !collected_tools.is_empty() {
            final_text = format!(
                "Completed {} tool operation(s) and applied {} file modification(s).",
                collected_tools.len(),
                collected_diffs.len()
            );
        }

        // Commit assistant turn to conversation
        if let Ok(mut g) = self.state.write() {
            if let Some(conv) = g.conversations.iter_mut().find(|c| c.id == conv_id) {
                conv.total_tokens += total_turn_tokens;
                conv.total_input_tokens += total_turn_in_tokens;
                conv.total_output_tokens += total_turn_out_tokens;
                conv.total_cost_usd += total_turn_cost;
                conv.messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: final_text,
                    thinking: if collected_thinking.is_empty() {
                        None
                    } else {
                        Some(collected_thinking)
                    },
                    tool_calls: collected_tools,
                    diffs: collected_diffs,
                    timestamp: Utc::now().format("%H:%M:%S").to_string(),
                    model: Some(settings.model),
                    tokens: total_turn_tokens,
                    input_tokens: total_turn_in_tokens,
                    output_tokens: total_turn_out_tokens,
                    cost_usd: total_turn_cost,
                });
            }
        }
        self.save_state();

        if let Ok(mut live) = self.live_turn.write() {
            if live.started_at > 0.0 {
                live.elapsed_s = (Self::now_epoch_secs() - live.started_at).max(0.0);
            }
            live.running = false;
            live.current_step = "Ready".to_string();
        }
    }

    pub fn build_system_prompt(&self, ws_path: &std::path::Path) -> String {
        self.build_system_prompt_for_query(ws_path, "")
    }

    pub fn build_system_prompt_for_query(
        &self,
        ws_path: &std::path::Path,
        user_query: &str,
    ) -> String {
        let mode = self.agent_mode();
        let caveman = self.caveman_mode();
        let ponytail = self.ponytail_mode();
        let skills = SkillsManager::list_skills_for_workspace(Some(ws_path));
        let triggered_skills = SkillsManager::match_triggered_skills(user_query, Some(ws_path));
        let mcp_servers = McpManager::load();
        let subagents = self.subagents();

        let mut prompt = format!(
            "You are Unjess 4.0 (100% Native Rust Autonomous AI Coding & Security Engineering Harness).\n\
             Active Workspace: {}\n\
             Execution Mode: {}\n",
            ws_path.display(),
            mode
        );

        match mode.as_str() {
            "Plan" => {
                prompt.push_str(
                    "MODE DIRECTIVE (PLAN): Do not modify files yet. Produce a rigorous, step-by-step architectural & implementation plan.\n",
                );
            }
            "Supervised" => {
                prompt.push_str(
                    "MODE DIRECTIVE (SUPERVISED): Explain each file edit or command clearly before or during execution.\n",
                );
            }
            "Duel" => {
                prompt.push_str(
                    "MODE DIRECTIVE (SECURITY DUEL): Operate as both Red Team (offensive vulnerability discovery) and White Team (defensive hardening & patching).\n",
                );
            }
            _ => {
                prompt.push_str(
                    "MODE DIRECTIVE (AUTONOMOUS): Use tools proactively to inspect, implement, test, and verify changes end-to-end.\n\
                     AUTONOMOUS ACTION & TASK CONTINUITY INVARIANT: NEVER output transitional narration or announce what you intend to do next without invoking the tool in the same turn. Before ending your turn, check your last paragraph: if it is a plan, a question, or a promise about work you have not done yet, execute that work immediately with tool calls. Finish the whole requested scope before producing your final answer.\n\
                     ADVERSARIAL SELF-VERIFICATION PROTOCOL: Never assume generated artifacts or code are correct without empirical proof. Before declaring completion or stopping, you MUST write and execute an independent verification script (or compare against reference outputs/ground-truth files such as logits, expected keys, return codes, schemas) to verify that all invariants and requirements are 100% satisfied. If verification reveals discrepancies, iterate and fix them immediately.\n",
                );
            }
        }

        if caveman {
            prompt.push_str(
                "PERSONA (CAVEMAN MODE): Respond with ultra-dense, minimal words. Skip pleasantries; output pure code and direct facts.\n",
            );
        }
        if ponytail {
            prompt.push_str(
                "PERSONA (PONYTAIL MODE): Bring high-energy, sharp systems-engineering enthusiasm and crystal-clear architectural insight.\n",
            );
        }

        let protected = crate::tools::extract_protected_paths_from_prompt(user_query);
        if !protected.is_empty() {
            prompt.push_str(&format!(
                "\nPROTECTED FILE CONSTRAINTS (DO NOT MODIFY OR DELETE): `{}`\n",
                protected.join("`, `")
            ));
        }

        let learned_hints = ToolEngine::load_learned_error_hints();
        if !learned_hints.is_empty() {
            prompt.push_str("\nLearned Shell & Platform Hints:\n");
            for h in learned_hints.into_iter().rev().take(5) {
                prompt.push_str(&format!("- {}\n", h));
            }
        }

        let repo_map = generate_repo_map(ws_path, 30, 3200);
        if !repo_map.is_empty() {
            prompt.push_str("\nWorkspace Structural Repo Map (top files & symbols):\n");
            prompt.push_str(&repo_map);
        }

        if !subagents.is_empty() {
            prompt.push_str("\nAvailable Autonomous Subagents (use `delegate_subagent` to delegate specialized work):\n");
            for sub in &subagents {
                prompt.push_str(&format!("- `{}` ({}): {}\n", sub.id, sub.name, sub.role));
            }
        }

        if !skills.is_empty() {
            prompt.push_str("\nInstalled Skill Plugins (use `run_skill_plugin` to read instructions, read `references/`, or execute `scripts/`):\n");
            for s in &skills {
                let scripts_meta = if s.scripts.is_empty() {
                    String::new()
                } else {
                    format!(" | scripts: [{}]", s.scripts.join(", "))
                };
                let refs_meta = if s.references.is_empty() {
                    String::new()
                } else {
                    format!(" | references: [{}]", s.references.join(", "))
                };
                let dir_meta = if s.dir_path.is_empty() {
                    String::new()
                } else {
                    format!(" [dir: `{}`]", s.dir_path)
                };
                prompt.push_str(&format!(
                    "- `{}` ({}): {}{}{}{}\n",
                    s.id, s.name, s.description, dir_meta, scripts_meta, refs_meta
                ));
            }
        }

        if !triggered_skills.is_empty() {
            prompt.push_str("\nActive Triggered Skill Instructions:\n");
            for ts in triggered_skills.into_iter().take(3) {
                prompt.push_str(&format!("--- SKILL: {} ({}) ---\n", ts.name, ts.id));
                if !ts.dir_path.is_empty() {
                    prompt.push_str(&format!("Skill Directory: `{}`\n", ts.dir_path));
                }
                if !ts.scripts.is_empty() {
                    prompt.push_str(&format!(
                        "Bundled Scripts (execute via `run_skill_plugin` with `script_name`): `{}`\n",
                        ts.scripts.join("`, `")
                    ));
                }
                if !ts.references.is_empty() {
                    prompt.push_str(&format!(
                        "Bundled References (read via `run_skill_plugin` with `reference_name`): `{}`\n",
                        ts.references.join("`, `")
                    ));
                }
                prompt.push_str(&format!("{}\n", ts.body));
            }
        }

        if !mcp_servers.is_empty() {
            prompt.push_str("\nConfigured MCP Servers (use `mcp__<server>__<tool>`, `call_mcp_tool`, or `probe_mcp_server`):\n");
            for srv in mcp_servers.iter().filter(|s| s.enabled) {
                let tnames: Vec<String> = srv.cached_tools.iter().map(|t| t.name.clone()).collect();
                let tools_meta = if tnames.is_empty() {
                    String::new()
                } else {
                    format!(" | tools: [{}]", tnames.join(", "))
                };
                prompt.push_str(&format!(
                    "- `{}`: {}{}\n",
                    srv.name, srv.description, tools_meta
                ));
            }
        }

        prompt.push_str(
            "\nSELF-PLUGIN AUTHORING CAPABILITY:\n\
             - When asked to code or create a skill or plugin for yourself, call `create_skill_plugin` (writes SKILL.md + optional executable script into ~/.unjess-rust/skills/<id>/ and hot-loads it immediately in the UI) or `register_mcp_plugin` (writes & registers an MCP server in ~/.unjess-rust/mcp.json).\n",
        );

        // Combine query-relevant recalled memories with recent memories (deduplicated)
        let mut selected_memories = Vec::new();
        let mut seen_mem_ids = std::collections::HashSet::new();
        if !user_query.trim().is_empty() {
            for m in MemoryStore::recall(user_query).into_iter().take(12) {
                if seen_mem_ids.insert(m.id.clone()) {
                    selected_memories.push(m);
                }
            }
        }
        for m in MemoryStore::load().into_iter().rev().take(15) {
            if selected_memories.len() >= 20 {
                break;
            }
            if seen_mem_ids.insert(m.id.clone()) {
                selected_memories.push(m);
            }
        }

        if !selected_memories.is_empty() {
            prompt.push_str("\nPersistent Cross-Session Memories:\n");
            for m in &selected_memories {
                prompt.push_str(&format!("- [{}] {}: {}\n", m.category, m.key, m.content));
            }
        }

        // Auto-inject @<file_path> mentions from the user query (up to 4 workspace files)
        if !user_query.trim().is_empty() {
            if let Ok(ws) = Workspace::new(ws_path) {
                let mut pinned_files = Vec::new();
                let mut seen_paths = std::collections::HashSet::new();
                for token in user_query.split_whitespace() {
                    if let Some(raw_mention) = token.strip_prefix('@') {
                        let cleaned = raw_mention.trim_matches(|c: char| {
                            matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\'')
                        });
                        if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("rea") {
                            continue;
                        }
                        if let Ok(resolved) = ws.resolve(cleaned) {
                            if resolved.is_file() && seen_paths.insert(resolved.clone()) {
                                if let Ok(content) = fs::read_to_string(&resolved) {
                                    let clipped = ToolEngine::truncate_head_tail(&content, 6000);
                                    pinned_files.push((cleaned.to_string(), clipped));
                                    if pinned_files.len() >= 4 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if !pinned_files.is_empty() {
                    prompt.push_str("\nPinned @file Context:\n");
                    for (rel_path, content) in pinned_files {
                        prompt.push_str(&format!(
                            "--- FILE: {} ---\n{}\n",
                            rel_path, content
                        ));
                    }
                }
            }
        }

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_manager_state_lifecycle_and_compaction() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("UNJESS_RUST_CONFIG_DIR", tmp.path());
            std::env::set_var("UNJESS_SKIP_LOCAL_PROBE", "1");
        }

        let hm = HarnessManager::new();
        assert!(!hm.workspaces().is_empty());
        assert!(!hm.subagents().is_empty());
        assert!(!hm.workflows().is_empty());
        let subs = hm.subagents();
        let arch = subs.iter().find(|s| s.id == "sub-architect").unwrap();
        assert!(arch.tools.contains(&"grep_search".to_string()));
        assert!(arch.tools.contains(&"lsp_diagnostics".to_string()));
        let coder = subs.iter().find(|s| s.id == "sub-coder").unwrap();
        assert!(coder.tools.contains(&"apply_patch".to_string()));
        assert!(coder.tools.contains(&"ast_edit_symbol".to_string()));

        let conv_id = hm.new_conversation_with_mode(Some("Rust GPU Test"), Some("quick"));
        assert_eq!(hm.active_conversation().title, "Rust GPU Test");
        assert_eq!(hm.active_conversation().mode, "quick");

        hm.rename_conversation(&conv_id, "Renamed Session");
        assert_eq!(hm.active_conversation().title, "Renamed Session");

        hm.set_caveman_mode(true);
        assert!(hm.caveman_mode());

        // Verify /budget slash command updates max_cost_per_session
        let res = hm.try_handle_slash_command(&conv_id, "/budget 2.50");
        assert!(res.is_none());
        assert!((hm.settings().max_cost_per_session - 2.50).abs() < 1e-6);

        // Verify compact_messages preserves dominant/last assistant model
        let msgs = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "First prompt".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: "12:00:00".to_string(),
                model: None,
                tokens: 50,
                input_tokens: 50,
                output_tokens: 0,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "First answer".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: "12:00:01".to_string(),
                model: Some("claude-4-sonnet".to_string()),
                tokens: 150,
                input_tokens: 50,
                output_tokens: 100,
                cost_usd: 0.002,
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Second prompt".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: "12:00:02".to_string(),
                model: None,
                tokens: 30,
                input_tokens: 30,
                output_tokens: 0,
                cost_usd: 0.0,
            },
        ];
        let compacted = HarnessManager::compact_messages(&msgs, 1);
        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[0].content, "First prompt");
        assert_eq!(compacted[1].model.as_deref(), Some("claude-4-sonnet"));
        assert!((compacted[1].cost_usd - 0.002).abs() < 1e-6);
        assert_eq!(compacted[2].content, "Second prompt");

        // Verify @file context injection in build_system_prompt_for_query
        let sample_file = tmp.path().join("pinned_note.txt");
        fs::write(&sample_file, "PINNED_FILE_SECRET_MARKER_42").unwrap();
        let sys = hm.build_system_prompt_for_query(tmp.path(), "Please inspect @pinned_note.txt");
        assert!(sys.contains("Pinned @file Context:"));
        assert!(sys.contains("PINNED_FILE_SECRET_MARKER_42"));
    }

    #[test]
    fn test_smart_compaction_transitional_narration_and_loop_escalation() {
        // 1. Superseded read_file deduplication in `compact_messages_smartly`
        let msgs = vec![
            ChatMessage {
                role: "tool".to_string(),
                content: format!(
                    "[File: src/lib.rs (100 lines total, showing lines 1..=100)]\n{}",
                    "line content\n".repeat(40)
                ),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: "12:00:01".to_string(),
                model: None,
                tokens: 100,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: format!(
                    "[File: src/lib.rs (100 lines total, showing lines 1..=100)]\n{}",
                    "updated line content\n".repeat(40)
                ),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                timestamp: "12:00:02".to_string(),
                model: None,
                tokens: 100,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            },
        ];
        let smart = HarnessManager::compact_messages_smartly(&msgs);
        assert!(smart[0].content.contains("[Superseded read_file output"));
        assert!(smart[1].content.contains("updated line content"));

        // 2. Canonical tool call key & transparent bookkeeping tools
        assert!(HarnessManager::is_transparent_bookkeeping_tool("todo_write"));
        assert!(!HarnessManager::is_transparent_bookkeeping_tool("read_file"));
        let k1 = HarnessManager::canonicalize_tool_call_key(
            "read_file",
            &serde_json::json!({ "path": "./src\\main.rs", "start_line": 1 }),
        );
        let k2 = HarnessManager::canonicalize_tool_call_key(
            "read_file",
            &serde_json::json!({ "start_line": 1, "path": "src/main.rs" }),
        );
        assert_eq!(k1, k2);

        // 3. Escalating [3, 5, 8] <mentor_analysis>
        assert!(HarnessManager::format_repeat_tool_reminder("read_file", 2).is_none());
        assert!(HarnessManager::format_repeat_tool_reminder("read_file", 3)
            .unwrap()
            .contains("<mentor_analysis>"));
        assert!(HarnessManager::format_repeat_tool_reminder("read_file", 5)
            .unwrap()
            .contains("Escalation Level 2"));
        assert!(HarnessManager::format_repeat_tool_reminder("read_file", 8)
            .unwrap()
            .contains("CRITICAL CIRCUIT BREAKER"));

        // 4. Transitional narration & text tool call extraction
        assert!(HarnessManager::is_transitional_narration(
            "Let me inspect the configuration file first:"
        ));
        assert!(HarnessManager::is_transitional_narration(
            "I'll start by examining the development sample to understand the cipher."
        ));
        assert!(HarnessManager::is_transitional_narration(
            "I'll start by exploring the workspace structure and understanding what I'm working with."
        ));
        assert!(!HarnessManager::is_transitional_narration(
            "All tests pass and the build is complete."
        ));

        let extracted = HarnessManager::extract_tool_calls_from_text(
            "I will read the file now:\n<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"Cargo.toml\"}}</tool_call>",
        );
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].1, "read_file");
    }
}
