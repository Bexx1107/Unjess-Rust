//! Canonical runtime version and built-in release changelog for Unjess.

use serde::{Deserialize, Serialize};

/// Canonical semantic version string for Unjess 4.0.
pub const CURRENT_VERSION: &str = "4.0.1";

/// Represents a structured release entry in the Unjess version history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogEntry {
    pub version: String,
    pub date: String,
    pub title: String,
    pub tag: String,
    pub changes: Vec<String>,
}

/// Returns the built-in release history for Unjess.
pub fn builtin_changelog() -> Vec<ChangelogEntry> {
    vec![
        ChangelogEntry {
            version: "4.0.1".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.1 — Deep Reasoning Max Steps & GUI Polish".to_string(),
            tag: "LATEST".to_string(),
            changes: vec![
                "Extended autonomous agent loop max steps up to 600 with new --max-steps CLI flag.".to_string(),
                "Fixed header text layout overlap under action buttons in Updates & Changelog GUI tab.".to_string(),
                "Permanent anchor context retention preventing task prompt amnesia during deep reasoning runs.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.0".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0 — Native Rust Engine & Multi-File Skills".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Complete Rust workspace architecture across unjess-core, unjess-desktop (eframe/egui GPU), unjess-daemon (Tokio/Axum), unjess-cli, and native installer.".to_string(),
                "In-process Arc<HarnessManager> execution loop with real-time live turn streaming, reasoning effort controls, and instant file diff revert.".to_string(),
                "Multi-file Skills & companion scripts engine with shallow git clone support, nested .claude/skills discovery, and built-in script editor.".to_string(),
                "Model Context Protocol (2025-06-18) stdio and streamable HTTP client with dynamic tool schema injection.".to_string(),
                "November 2026 multi-provider model catalog (Gemini 4 Argon, Claude Opus/Sonnet 5.5, GPT-5, Kimi K3, Qwen 3.8 Max, DeepSeek V4 Pro, Ollama, llama.cpp, LM Studio).".to_string(),
            ],
        },
        ChangelogEntry {
            version: "3.0.2".to_string(),
            date: "2026-11-06".to_string(),
            title: "Privacy & Offline-First Hardening".to_string(),
            tag: "SECURITY".to_string(),
            changes: vec![
                "Removed all remote killswitch polling and external telemetry phones-home.".to_string(),
                "Replaced external passive update checks with GitHub Releases manifest verification.".to_string(),
                "Removed remote QR image beacon generation in favor of local companion URLs.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "3.0.0".to_string(),
            date: "2026-11-04".to_string(),
            title: "Unjess 3.0 Autonomous Engineering Harness".to_string(),
            tag: "MILESTONE".to_string(),
            changes: vec![
                "Introduced Red Team vs. White Team Security Duel static analyzer and auto-remediation engine.".to_string(),
                "Added 4-Way Agent Execution Modes: Autonomous, Plan, Supervised, and Duel.".to_string(),
                "Added Caveman Mode (ultra-concise token compression) and Ponytail Mode (expressive pair-programming persona).".to_string(),
                "Unified local GGUF routing for llama.cpp and LM Studio alongside Ollama.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "2.9.9".to_string(),
            date: "2026-11-01".to_string(),
            title: "Dynamic Prompt Dock & Model Discovery Upgrade".to_string(),
            tag: "RELEASE".to_string(),
            changes: vec![
                "Auto-raising multi-line prompt input dock with reasoning effort selector.".to_string(),
                "Built-in Reverse Engineer Anything (REA) skill and 1-click MCP presets.".to_string(),
                "Cross-session persistent MemoryStore with semantic recall.".to_string(),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_and_changelog() {
        assert_eq!(CURRENT_VERSION, "4.0.1");
        let log = builtin_changelog();
        assert!(!log.is_empty());
        assert_eq!(log[0].version, "4.0.1");
    }
}
