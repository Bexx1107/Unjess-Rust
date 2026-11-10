//! Canonical runtime version and built-in release changelog for Unjess.

use serde::{Deserialize, Serialize};

/// Canonical semantic version string for Unjess 4.0.
pub const CURRENT_VERSION: &str = "4.1.8";

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
            version: "4.1.8".to_string(),
            date: "2026-11-10".to_string(),
            title: "Unjess 4.1.8 — Pasted screenshots reach the model as real pixels; DeepSeek no longer mislabeled text-only".to_string(),
            tag: "LATEST".to_string(),
            changes: vec![
                "Fixed: the 'text-only model' override from 4.1.6/4.1.7 never actually removed DeepSeek from the text-only list, so the system prompt told DeepSeek it was blind and it refused to look at images. DeepSeek flash was verified to describe a test image correctly.".to_string(),
                "Composer screenshots ('[Attached Screenshot: `path`]') are now loaded from disk, downscaled for vision and attached to the message as pixels automatically; previously only a text note with the path was sent.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.7".to_string(),
            date: "2026-11-10".to_string(),
            title: "Unjess 4.1.7 — Vision continuity: no more false 'I cannot see images'".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Images viewed in earlier turns are not kept in context; their saved tool results used to still claim 'you can now see this image', so the model concluded it was blind. They are now rewritten to tell it to call view_image again.".to_string(),
                "System prompt now states images from earlier turns are not retained and instructs the model to re-open the file instead of claiming it cannot see.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.6".to_string(),
            date: "2026-11-10".to_string(),
            title: "Unjess 4.1.6 — Session Ledger: fixes mid-session amnesia & hallucinated history".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Root cause of 'I never generated anything / I don't know where we are': context compaction kept only the oldest 25 messages as 140-char first lines and listed hundreds of bare tool names, wiping what actually happened. Replaced with a factual Session Ledger.".to_string(),
                "Ledger keeps every user instruction near-verbatim, per-turn tool counts, files written/edited, shell commands run, and every MCP/unusual tool call (e.g. generate_image) with args, status and result.".to_string(),
                "Honest vision: the harness no longer tells text-only models (e.g. deepseek flash) that they can see images. view_image now returns a clear [NO VISION] result and the system prompt says so, instead of a forced false claim that made the model fake it with coordinate math.".to_string(),
                "Ledger always retains the most recent activity (older entries are omitted instead) and instructs the model never to contradict recorded history.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.5".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.5 — First-Class MCP Tool Injection (DMD Studio, DaVinci, Blender), Payload Interceptor & Anti-Stall Engine".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Full MCP Parity with Antigravity: automatically discovers, syncs, and injects all 132 MCP tools (including DMD Studio 44 tools, Blender, DaVinci Resolve, Google Flow, Chrome DevTools, Unity Bridge) directly into the LLM function schema.".to_string(),
                "Native Tool Calling: models can now invoke `generate_image`, `restyle_image`, `dmd_pipeline_generate_image` directly in 1 step without reverse-engineering shell/python scripts across 240+ steps.".to_string(),
                "Automatic Base64 Payload Interceptor: binary images and artifacts returned by MCP servers are automatically decoded and saved to disk (`.unjess/attachments/`), replacing multi-megabyte base64 strings with clean file links to completely eliminate Ollama/Cloud HTTP 400 context overflow errors.".to_string(),
                "Autonomous Loop Tripwire Elimination: raised default autonomous step ceiling to 1,000+ iterations and replaced silent loop break tripwires with persistent continuation and transparent step budget reporting.".to_string(),
                "Extended 240s MCP Timeout: heavy image generation and 3D rendering jobs no longer time out after 25 seconds.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.4".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.4 — Cross-Drive Absolute Path Freedom & Antigravity Skills Sync".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Eliminated workspace sandbox boundary errors on absolute paths: tools (outline_file, read_file, write_file, edit_file) can now freely access files on any drive (C:, E:, etc.) without 'escapes workspace boundary' errors.".to_string(),
                "Full Antigravity & Gemini Skills Discovery: automatically discovers and runs skills and plugins from ~/.gemini/config/skills and ~/.gemini/config/plugins (including asset-factory, dmd-studio, hyperframes).".to_string(),
                "Sanitized Compaction Task Anchor: opening greetings ('jou', 'yo', 'hi') are no longer pinned as the root task specification, preventing agent amnesia and confusion.".to_string(),
                "Autonomous Action Enforcement: intercepted hesitant multiple-choice option menus ('Want me to (a)... (b)... or (c)...?') when explicit generation/action commands were given, forcing the agent to execute directly.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.3".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.3 — Autonomous Continuation Directive & Anti-Stall Guard".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Eliminated mid-run stalls caused by intermediate LLM observations: expanded `is_transitional_narration` to recognize bug diagnoses, ongoing code additions, and progress notes (e.g. 'Found the real bug', 'Also adding...').".to_string(),
                "Added Autonomous Continuation Directive: in Auto mode, if an agent provides observational text without tool calls and without declaring completion (`<!-- GOAL_COMPLETE -->`) or asking a user question, the engine automatically commands it to execute rather than prematurely stopping.".to_string(),
                "Dynamic Guard Resets: `pre_exit_unverified_edit_count` and `pre_exit_test_check_count` now reset dynamically upon running verification commands, keeping edit and verification guards active through long runs (200+ steps).".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.2".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.2 — Multi-Conversation Concurrency, Per-Chat Abort & Context Compaction Overflow Fix".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "True concurrent multi-conversation and multi-project background execution: chats run independently in the background without halting when switching tabs, workspaces, or starting other sessions.".to_string(),
                "Per-conversation abort isolation: clicking Stop only halts the active conversation while other background agent loops continue unimpeded.".to_string(),
                "Visual concurrency indicators: active background conversations display live emerald run pulses and status tags in the sidebar.".to_string(),
                "Eliminated Ollama HTTP 400 'The prompt is too long' (1M+ token limit) errors: multi-stage context compaction now accurately weighs tool results and historical reasoning tokens, auto-pruning stale multi-megabyte command outputs.".to_string(),
                "Auto-renaming & state isolation for new conversations: creating a new conversation resets live state, generates numbered titles ('Quick Chat N', 'Session N'), and dynamically auto-titles on the first user message.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.1".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.1 — Multimodal Image Downscaling, Compression & Historical Turn Pruning".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Eliminated Ollama HTTP 400 'request body too large' error during long multi-step vision workflows by auto-downscaling large images to 1568px and re-encoding as compressed JPEG/PNG.".to_string(),
                "Added active vision windowing (`filter_active_vision_images`): only the 2 most recent vision images retain raw Base64 strings, safely pruning multi-megabyte payloads from earlier turns.".to_string(),
                "Applied vision payload pruning across all providers: Ollama (/api/chat), OpenAI (/v1/chat/completions), Anthropic, and Gemini.".to_string(),
                "Integrated image retention compaction directly into the agent execution loop and `compact_messages_for_model`.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.1.0".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.1.0 — Multi-Turn Tool History Integrity & Autonomous Action Enforcement".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Fixed multi-turn tool history serialization in Ollama and OpenAI providers so historical tool calls & results are preserved across subsequent turns without amnesia.".to_string(),
                "Added Autonomous Action Directive in `agent.rs`: prevents the model from prematurely exiting the loop by asking hesitant questions when the user gave an explicit command (e.g. generate, fix, run, build).".to_string(),
                "Added Inspection Saturation Guard: warns and re-orients the agent to execution if it executes 14+ consecutive read/search operations without making any changes or running commands.".to_string(),
                "Updated Autonomous system prompt directives to prioritize direct task execution over exhaustive whole-workspace file reconnaissance.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.9".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.0.9 — Multimodal Vision Engine & Auto Attachment Ingestion".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Restored native multimodal Vision engine with `view_image` tool schema and execution handler.".to_string(),
                "Added transparent image delegation in `read_file` so .png, .jpg, .jpeg, .webp, .gif, .bmp auto-queue for vision input.".to_string(),
                "Added RFC 4648 Base64 serialization across Ollama (`images: []`), OpenAI (`image_url`), Anthropic (`type: 'image'`), and Gemini (`inline_data`).".to_string(),
                "Integrated automatic Discord attachment parser to ingest disk attachments directly into turn 1 visual context.".to_string(),
                "Injected Multimodal Vision Directive into the system prompt to eradicate vision amnesia and false capability denials.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.8".to_string(),
            date: "2026-11-09".to_string(),
            title: "Unjess 4.0.8 — Context Window Integrity & DeepSeek 1M Context Parity".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Eliminated artificial 6-message compaction clamp in autonomous execution loop, restoring full multi-turn context retention matching Go benchmark baseline.".to_string(),
                "Updated DeepSeek v4 context window capacity in llm.rs to 1,000,000 tokens for full repository-level retention.".to_string(),
                "Scaled smart model-aware compaction thresholds up to 1.2M chars with generous recent history protection.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.7".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.7 — Deductive Ingestion, Autonomous Solver Protocol & Anti-Thrashing Guardrails".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Added Deductive Architectural Ingestion directive: agents must thoroughly inspect specs, configs, and models upfront rather than guessing.".to_string(),
                "Added Systematic Optimization directive: forbids endless single-offset diagnostic probes in favor of modular automated solver scripts.".to_string(),
                "Implemented Anti-Thrashing Runtime Guardrail: detects repetitive diagnostic probe loops and injects autonomous strategy pivot reminders.".to_string(),
                "Expanded scratch file detector to clean up probe/diagnostic files and prevent in-context repetition thrashing.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.6".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.6 — Native Win32 Hotkey Image Paste & Sentinel Tag Cleanup".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Bypassed egui-winit event swallow via native Win32 GetAsyncKeyState hotkey detection so Ctrl+V and Shift+Insert reliably attach clipboard screenshots.".to_string(),
                "Added multi-attempt retry loop for OS clipboard image reading to eliminate race conditions with Windows Snipping Tool.".to_string(),
                "Eliminated false-positive autonomous loop nudges on conversational replies and questions, stopping unwanted 'goal complete' loop stalls.".to_string(),
                "Sanitized internal <!-- GOAL_COMPLETE --> and HTML comments from Markdown renderer and chat history display.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.5".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.5 — Zero-Interference Clipboard Screenshot Pasting".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Eliminated text-clipboard interference check so Windows Snipping Tool OCR and clipboard URLs never block image pasting.".to_string(),
                "Expanded paste detection across Ctrl+V, Cmd+V, Shift+Insert, and OS paste events.".to_string(),
                "Filtered duplicate text injection when attaching screenshots directly from clipboard into the composer.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.4".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.4 — Autonomous Step Budget Warning & 2000-Step Limit".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Raised autonomous execution step ceiling from 600 to 2000 steps with customizable CLI --max-steps flag.".to_string(),
                "Added Pre-Expiration Autonomous Budget Notice (<budget_warning>) when remaining steps <= 30 to force final solution export and validation.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.3".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.3 — Clipboard Image Paste & Resizable GUI Sidepanels".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Added direct clipboard image paste (Ctrl+V) from OS snipping tools into composer with PNG attachment chips.".to_string(),
                "Added draggable resizable sidepanels for chat session sidebar and diff inspector.".to_string(),
                "Turn-level exponential backoff and transient LLM network drop resilience for autonomous evaluations.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.2".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.2 — SOTA Loop Interception & Non-Destructive Circuit Breaker".to_string(),
            tag: "STABLE".to_string(),
            changes: vec![
                "Replaced destructive hard circuit breaker with non-destructive in-loop interception warnings aligned with SWE-agent and OpenHands.".to_string(),
                "Consecutive window repetition tracking preventing false-positive aborts on distributed read-only inspection commands.".to_string(),
            ],
        },
        ChangelogEntry {
            version: "4.0.1".to_string(),
            date: "2026-11-08".to_string(),
            title: "Unjess 4.0.1 — Deep Reasoning Max Steps & GUI Polish".to_string(),
            tag: "STABLE".to_string(),
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
        assert_eq!(CURRENT_VERSION, "4.1.8");
        let log = builtin_changelog();
        assert!(!log.is_empty());
        assert_eq!(log[0].version, "4.1.8");
    }
}
