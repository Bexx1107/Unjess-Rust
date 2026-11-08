//! Unjess 4.0 Core Engine (`unjess-core`) — 100% Native Rust Autonomous AI & Security Harness.
//!
//! Exports all core subsystems:
//! - `version`: Canonical version (`4.0.0`) and built-in release changelog.
//! - `config`: Settings, API key storage (`~/.unjess-rust`), automatic key/config import from `~/.unjess`, and model discovery.
//! - `workspace`: Sandboxed workspace path resolution, Git status inspection, and recursive file indexing.
//! - `tools`: Sandboxed file read/write/edit with unified diff generation & 1-click revert, regex grep, glob search, stateful shell execution, Frontier AST/sandbox tools, and web search.
//! - `memory`: Persistent cross-session memory store (`~/.unjess-rust/memory.json`).
//! - `mcp`: Model Context Protocol server registry & 1-click presets (`rea`, `filesystem`, `github`, `fetch`).
//! - `skills`: Built-in `reverse-engineer-anything` (REA) skill and custom skill manager.
//! - `scheduler`: Persistent cron/interval task scheduler.
//! - `codeintel`: Multi-language AST symbol outline, caller search, symbol editing/renaming, git archeology, debug trace, and post-edit syntax verification.
//! - `security`: Red Team vs. White Team Security Duel static vulnerability scanner.
//! - `updater`: Passive GitHub Releases update checker.
//! - `llm`: Multi-provider async LLM client (Ollama, llama.cpp, LM Studio, Google Gemini, Anthropic Claude, OpenAI, Groq, Mistral, xAI, OpenRouter, Cerebras, Kimi, Qwen, Custom Providers).
//! - `agent`: Thread-safe `HarnessManager` and autonomous multi-turn tool execution loop.
//! - `api`: Embedded Axum REST API server (`127.0.0.1:9758`).

#![recursion_limit = "512"]

pub mod agent;
pub mod api;
pub mod codeintel;
pub mod config;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod scheduler;
pub mod security;
pub mod skills;
pub mod tools;
pub mod updater;
pub mod version;
pub mod workspace;

pub use agent::{
    AgentConfig, Conversation, HarnessManager, LiveTurnSnapshot, SubagentDef, WorkflowDef,
    WorkspaceEntry,
};
pub use api::start_api_server;
pub use codeintel::{
    debug_trace_command, edit_symbol, find_callers, git_archeology, outline_file,
    rename_symbol_across_repo, run_post_edit_check, run_workspace_diagnostics, AstSymbol,
};
pub use config::{CustomProvider, ModelInfo, Settings};
pub use llm::{model_context_window_tokens, ChatMessage};
pub use mcp::{McpManager, McpServerConfig};
pub use memory::{MemoryEntry, MemoryStore};
pub use scheduler::{LiveBackgroundTask, ScheduledTask, SchedulerManager};
pub use security::{scan_workspace, SecurityDuelReport, SecurityFinding};
pub use skills::{SkillInfo, SkillsManager};
pub use tools::{
    extract_protected_paths_from_prompt, DirEntryInfo, FileDiff, TodoItem, ToolCallRecord,
    ToolEngine, ToolError,
};
pub use updater::{check_for_updates, UpdateCheckResult};
pub use version::{builtin_changelog, ChangelogEntry, CURRENT_VERSION};
pub use workspace::{GitInfo, Workspace, WorkspaceError};

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
