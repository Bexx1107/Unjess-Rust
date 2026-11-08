//! Unjess 4.0 Core Engine (`unjess-core`) — 100% Native Rust Autonomous AI & Security Harness.
//!
//! Exports all core subsystems:
//! - `version`: Canonical version (`4.0.0`) and built-in release changelog.
//! - `config`: Settings, API key storage (`~/.unjess-rust`), automatic key/config import from `~/.unjess`, and model discovery.
//! - `workspace`: Sandboxed workspace path resolution, Git status inspection, and recursive file indexing.
//! - `tools`: Sandboxed file read/write/edit with unified diff generation & 1-click revert, regex grep, glob search, shell execution, and web search.
//! - `memory`: Persistent cross-session memory store (`~/.unjess-rust/memory.json`).
//! - `mcp`: Model Context Protocol server registry & 1-click presets (`mashina-collab`, `rea`, `filesystem`, `github`, `fetch`).
//! - `skills`: Built-in `reverse-engineer-anything` (REA) skill and custom skill manager.
//! - `scheduler`: Persistent cron/interval task scheduler.
//! - `codeintel`: Multi-language AST symbol outline & caller search.
//! - `security`: Red Team vs. White Team Security Duel static vulnerability scanner.
//! - `updater`: Passive GitHub Releases update checker.
//! - `llm`: Multi-provider async LLM client (Ollama, llama.cpp, LM Studio, Google Gemini, Anthropic Claude, OpenAI, Groq, Mistral, xAI, OpenRouter, Cerebras, Kimi, Qwen, Custom Providers).
//! - `agent`: Thread-safe `HarnessManager` and autonomous multi-turn tool execution loop.
//! - `api`: Embedded Axum REST API server (`127.0.0.1:9758`).

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
pub use codeintel::{find_callers, outline_file, AstSymbol};
pub use config::{CustomProvider, ModelInfo, Settings};
pub use llm::ChatMessage;
pub use mcp::{McpManager, McpServerConfig};
pub use memory::{MemoryEntry, MemoryStore};
pub use scheduler::{ScheduledTask, SchedulerManager};
pub use security::{scan_workspace, SecurityDuelReport, SecurityFinding};
pub use skills::{SkillInfo, SkillsManager};
pub use tools::{DirEntryInfo, FileDiff, ToolCallRecord, ToolEngine, ToolError};
pub use updater::{check_for_updates, UpdateCheckResult};
pub use version::{builtin_changelog, ChangelogEntry, CURRENT_VERSION};
pub use workspace::{GitInfo, Workspace, WorkspaceError};

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

