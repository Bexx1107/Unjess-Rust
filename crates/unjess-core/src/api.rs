//! Embedded Axum HTTP/REST & Stream API Server (`api.rs`).
//!
//! Serves the Unjess Desktop UI (`index.html`) and all `/api/*` endpoints in-process
//! on `127.0.0.1:9758` so it never conflicts with the Go daemon (`9757`).

use crate::agent::{Conversation, HarnessManager, WorkflowDef, WorkspaceEntry};
use crate::llm::resolve_provider_for_model;
use crate::mcp::{McpManager, McpServerConfig};
use crate::memory::MemoryStore;
use crate::scheduler::{ScheduledTask, SchedulerManager};
use crate::security::scan_workspace;
use crate::skills::SkillsManager;
use crate::tools::{FileDiff, ToolEngine};
use crate::updater::{check_for_updates, verify_file_sha256, verify_sha256_bytes};
use crate::version::CURRENT_VERSION;
use crate::workspace::Workspace;
use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

static START_INSTANT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn server_start_instant() -> &'static std::time::Instant {
    START_INSTANT.get_or_init(std::time::Instant::now)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ChatPayload {
    #[serde(default, alias = "conv_id")]
    pub conversation_id: Option<String>,
    #[serde(default, alias = "message")]
    pub prompt: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub reasoning_effort: Option<String>,
    pub workspace: Option<String>,
    pub agent_mode: Option<String>,
    pub caveman_mode: Option<bool>,
    pub ponytail_mode: Option<bool>,
    pub subagent_id: Option<String>,
    pub images: Option<Vec<serde_json::Value>>,
    pub attachments: Option<Vec<serde_json::Value>>,
    pub tools: Option<serde_json::Value>,
}

/// Starts the Axum HTTP API server bound to `host:port`.
pub async fn start_api_server(
    hm: Arc<HarnessManager>,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let _ = server_start_instant();
    SchedulerManager::start_background_ticker(Arc::clone(&hm));
    let app = build_router(hm);
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind {}: {}", addr, e))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| format!("API server error: {}", e))
}

/// Builds the Axum `Router` with all Unjess 4.0 UI & REST endpoints.
pub fn build_router(hm: Arc<HarnessManager>) -> Router {
    Router::new()
        // Desktop & Companion HTML UI routes
        .route("/", get(handle_index_html))
        .route("/desktop", get(handle_index_html))
        .route("/mobile", get(handle_index_html))
        .route("/app", get(handle_index_html))
        .route("/openclaw", get(handle_index_html))
        .route("/studio", get(handle_index_html))
        // Core Health & State
        .route("/api/health", get(handle_health))
        .route("/api/harness/state", get(handle_harness_state))
        .route("/api/harness/live", get(handle_harness_live))
        .route("/api/harness/stream", get(handle_harness_stream))
        // Models & Settings
        .route("/api/models", get(handle_models))
        .route("/api/studio/models", get(handle_models))
        .route("/api/harness/models", get(handle_models))
        .route(
            "/api/settings",
            get(handle_get_settings).post(handle_post_settings),
        )
        .route(
            "/api/studio/settings",
            get(handle_get_settings).post(handle_post_settings),
        )
        // Conversations
        .route(
            "/api/harness/conversations",
            post(handle_create_conversation),
        )
        .route(
            "/api/harness/conversations/{id}",
            delete(handle_delete_conversation).patch(handle_rename_conversation),
        )
        .route(
            "/api/harness/conversations/{id}/clear",
            post(handle_clear_conversation),
        )
        .route(
            "/api/harness/conversations/{id}/rename",
            post(handle_rename_conversation),
        )
        // Workspaces
        .route(
            "/api/harness/workspaces",
            post(handle_add_workspace).delete(handle_remove_workspace),
        )
        .route(
            "/api/harness/workspaces/remove",
            post(handle_remove_workspace),
        )
        .route(
            "/api/harness/workspaces/select",
            post(handle_select_workspace),
        )
        .route("/api/harness/workspace", put(handle_select_workspace))
        .route(
            "/api/harness/workspaces/browse",
            post(handle_browse_workspace),
        )
        // Workspace Files & Preview
        .route("/api/harness/files", get(handle_list_files))
        .route(
            "/api/harness/file",
            get(handle_read_file)
                .post(handle_write_file)
                .delete(handle_delete_file),
        )
        .route("/api/harness/raw", get(handle_raw_file))
        .route("/api/harness/web-proxy", get(handle_web_proxy))
        .route("/api/harness/browser/action", post(handle_browser_action))
        // Chat, Execution & Turn Controls
        .route("/api/harness/chat", post(handle_harness_chat))
        .route("/api/chat", post(handle_harness_chat))
        .route("/api/harness/exec", post(handle_harness_exec))
        .route("/api/harness/abort", post(handle_harness_abort))
        .route("/api/harness/undo", post(handle_harness_undo))
        .route("/api/harness/compact", post(handle_harness_compact))
        .route("/api/harness/mode", post(handle_harness_mode))
        .route("/api/harness/agent-mode", post(handle_harness_mode))
        .route(
            "/api/harness/modifiers",
            get(handle_get_modifiers).post(handle_post_modifiers),
        )
        .route("/api/harness/diff/revert-hunk", post(handle_revert_diff))
        // Interactive Approvals
        .route("/api/harness/command/approve", post(handle_command_approve))
        .route("/api/harness/question/answer", post(handle_question_answer))
        .route("/api/harness/plan/approve", post(handle_plan_approve))
        .route(
            "/api/harness/subagents/approve",
            post(handle_subagents_approve),
        )
        // Workflows & Agents / Subagents
        .route(
            "/api/harness/workflows",
            get(handle_get_workflows).post(handle_post_workflow),
        )
        .route(
            "/api/harness/workflows/{id}",
            delete(handle_delete_workflow),
        )
        .route(
            "/api/harness/agents",
            get(handle_get_agents).post(handle_post_agent),
        )
        .route("/api/harness/agents/{id}", delete(handle_delete_agent))
        .route("/api/harness/agents/{id}/run", post(handle_run_agent))
        .route("/api/harness/subagents", get(handle_get_subagents))
        .route("/api/harness/subagents/spawn", post(handle_spawn_subagent))
        .route("/api/harness/subagents/{id}", get(handle_get_subagent_one))
        .route(
            "/api/harness/subagents/{id}/message",
            post(handle_message_subagent),
        )
        .route(
            "/api/harness/subagents/{id}/abort",
            post(handle_abort_subagent),
        )
        // MCP, Skills, Memories & Schedules
        .route(
            "/api/harness/mcp",
            get(handle_get_mcp).post(handle_post_mcp),
        )
        .route("/api/harness/mcp/refresh", post(handle_refresh_mcp))
        .route("/api/harness/mcp/{name}", delete(handle_delete_mcp))
        .route("/api/harness/mcp/{name}/test", post(handle_test_mcp))
        .route("/api/skills", get(handle_get_skills))
        .route(
            "/api/harness/skills",
            get(handle_get_skills)
                .post(handle_post_skill)
                .delete(handle_delete_skill_query),
        )
        .route(
            "/api/harness/skills/install-github",
            post(handle_install_github_skill),
        )
        .route(
            "/api/harness/skills/file",
            get(handle_get_skill_file)
                .post(handle_post_skill_file)
                .delete(handle_delete_skill_file),
        )
        .route(
            "/api/harness/skills/open-folder",
            post(handle_open_skill_folder),
        )
        .route(
            "/api/harness/skills/{name}",
            get(handle_get_skill_one).delete(handle_delete_skill),
        )
        .route(
            "/api/harness/memories",
            get(handle_get_memories)
                .post(handle_post_memory)
                .delete(handle_delete_memory),
        )
        .route(
            "/api/schedules",
            get(handle_get_schedules).post(handle_post_schedule),
        )
        .route(
            "/api/schedules/{id}",
            put(handle_put_schedule).delete(handle_delete_schedule),
        )
        .route("/api/schedules/{id}/toggle", post(handle_toggle_schedule))
        .route("/api/schedules/{id}/run", post(handle_run_schedule))
        .route(
            "/api/harness/schedules",
            get(handle_get_schedules).post(handle_post_schedule),
        )
        .route(
            "/api/harness/schedules/{id}",
            put(handle_put_schedule).delete(handle_delete_schedule),
        )
        .route(
            "/api/harness/schedules/{id}/toggle",
            post(handle_toggle_schedule),
        )
        .route("/api/harness/schedules/{id}/run", post(handle_run_schedule))
        // Updates, Security Duel, Usage, Context & Network
        .route("/api/update/check", get(handle_update_check))
        .route("/api/harness/updates", get(handle_update_check))
        .route("/api/harness/updates/check", post(handle_update_check_post))
        .route("/api/harness/updates/apply", post(handle_update_apply))
        .route("/api/security/duel", post(handle_security_duel))
        .route("/api/harness/security/duel", post(handle_security_duel))
        .route("/api/harness/watchdog", get(handle_watchdog))
        .route("/api/harness/usage", get(handle_usage))
        .route("/api/harness/context", get(handle_context))
        .route("/api/harness/network-info", get(handle_network_info))
        .layer(CorsLayer::permissive())
        .with_state(hm)
}

async fn handle_index_html() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "engine": "unjess-rust-native-gpu",
        "version": CURRENT_VERSION,
    }))
}

async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "engine": "unjess-rust-native",
        "version": CURRENT_VERSION,
    }))
}

fn format_conversation_for_ui(
    conv: &Conversation,
    workspaces: &[WorkspaceEntry],
) -> serde_json::Value {
    let ws_path = workspaces
        .iter()
        .find(|w| w.id == conv.workspace_id)
        .or_else(|| workspaces.first())
        .map(|w| w.path.clone())
        .unwrap_or_else(|| ".".to_string());

    let mut turns = Vec::new();
    for (idx, msg) in conv.messages.iter().enumerate() {
        if msg.role == "system" || msg.role == "tool" {
            continue;
        }
        let ui_role = if msg.role == "user" { "user" } else { "agent" };
        let tool_calls: Vec<serde_json::Value> = msg
            .tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "name": tc.name,
                    "arguments": tc.arguments,
                    "result": tc.result,
                    "output": tc.result,
                    "status": if tc.status == "ok" { "completed" } else { &tc.status },
                    "duration_s": (tc.duration_ms as f64) / 1000.0,
                })
            })
            .collect();

        let mut planning_steps = Vec::new();
        if let Some(ref th) = msg.thinking {
            if !th.trim().is_empty() {
                planning_steps.push(serde_json::json!({
                    "text": "Synthesized architectural reasoning",
                    "status": "completed",
                    "type": "thinking"
                }));
            }
        }
        for tc in &msg.tool_calls {
            planning_steps.push(serde_json::json!({
                "text": format!("Executed `{}` ({} ms)", tc.name, tc.duration_ms),
                "status": if tc.status == "ok" { "completed" } else { "error" },
                "type": "tool"
            }));
        }

        let files_created: Vec<String> = msg
            .diffs
            .iter()
            .filter(|d| d.status == "created")
            .map(|d| d.path.clone())
            .collect();
        let mut files_touched: Vec<String> = Vec::new();
        for d in &msg.diffs {
            if !files_touched.contains(&d.path) {
                files_touched.push(d.path.clone());
            }
        }

        turns.push(serde_json::json!({
            "id": format!("turn_{}_{}", conv.id, idx),
            "role": ui_role,
            "text": msg.content,
            "thinking": msg.thinking.clone().unwrap_or_default(),
            "planningSteps": planning_steps,
            "toolCalls": tool_calls,
            "diffs": msg.diffs,
            "filesCreated": files_created,
            "files_touched": files_touched,
            "timestamp": msg.timestamp,
            "model": msg.model,
            "tokens": msg.tokens,
            "input_tokens": msg.input_tokens,
            "output_tokens": msg.output_tokens,
            "cost_usd": msg.cost_usd,
        }));
    }

    let conv_mode = if conv.mode.trim().is_empty() {
        "workspace"
    } else {
        &conv.mode
    };

    serde_json::json!({
        "id": conv.id,
        "title": conv.title,
        "workspace": ws_path,
        "workspace_id": conv.workspace_id,
        "mode": conv_mode,
        "created_at": conv.created_at,
        "updated_at": Utc::now().timestamp() as f64,
        "total_tokens": conv.total_tokens,
        "total_input_tokens": conv.total_input_tokens,
        "total_output_tokens": conv.total_output_tokens,
        "total_cost_usd": conv.total_cost_usd,
        "turns": turns,
        "messages": conv.messages,
    })
}

fn collect_workspace_files_json(ws_path: &Path) -> serde_json::Value {
    let mut files = Vec::new();
    let mut html_files = Vec::new();
    if let Ok(ws) = Workspace::new(ws_path) {
        for rel in ws.list_all_files(1500) {
            if rel.ends_with(".html") || rel.ends_with(".htm") {
                html_files.push(rel.clone());
            }
            let size = ws
                .resolve(&rel)
                .ok()
                .and_then(|p| fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            files.push(serde_json::json!({
                "path": rel,
                "size": size,
            }));
        }
    }
    let git_info = collect_git_info_json(ws_path);
    serde_json::json!({
        "status": "ok",
        "workspace": ws_path.to_string_lossy(),
        "files": files,
        "html_files": html_files,
        "git_info": git_info,
    })
}

fn collect_git_info_json(ws_path: &Path) -> serde_json::Value {
    let info = Workspace::new(ws_path)
        .map(|ws| ws.git_info())
        .unwrap_or_default();

    if info.branch.is_empty() {
        return serde_json::json!({
            "is_git": false,
            "is_repo": false,
            "is_dirty": false,
            "branch": "",
            "dirty_count": 0,
            "changed_count": 0,
        });
    }

    serde_json::json!({
        "is_git": true,
        "is_repo": true,
        "is_dirty": info.dirty,
        "branch": info.branch,
        "dirty_count": info.changed_files,
        "changed_count": info.changed_files,
    })
}

async fn handle_harness_state(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let workspaces = hm.workspaces();
    let ws_path = hm.active_workspace_path();
    let ws_path_str = ws_path.to_string_lossy().to_string();
    let convs: Vec<serde_json::Value> = hm
        .conversations()
        .iter()
        .map(|c| format_conversation_for_ui(c, &workspaces))
        .collect();
    let ws_files = collect_workspace_files_json(&ws_path);
    let git_info = ws_files
        .get("git_info")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "is_git": false, "is_repo": false }));
    let settings = hm.settings();
    let skills_count = SkillsManager::list_skills_for_workspace(Some(&ws_path)).len();
    let mcp_count = McpManager::load().len();

    Json(serde_json::json!({
        "status": "ok",
        "version": CURRENT_VERSION,
        "state": {
            "active_workspace": ws_path_str,
            "active_workspace_id": hm.active_workspace_id(),
            "active_conversation_id": hm.active_conversation_id(),
            "workspaces": workspaces,
            "conversations": convs,
            "workflows": hm.workflows(),
            "agents": hm.subagents(),
        },
        "workspaces": workspaces,
        "active_workspace_id": hm.active_workspace_id(),
        "conversations": convs,
        "active_conversation_id": hm.active_conversation_id(),
        "subagents": hm.subagents(),
        "workflows": hm.workflows(),
        "workspace_files": ws_files,
        "git_info": git_info,
        "skills_count": skills_count,
        "mcp_count": mcp_count,
        "default_model": settings.model,
        "provider": settings.provider,
        "agent_mode": hm.agent_mode(),
        "caveman_mode": hm.caveman_mode(),
        "ponytail_mode": hm.ponytail_mode(),
        "live_turn": hm.live_turn_snapshot(),
    }))
}

async fn handle_harness_live(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let snap = hm.live_turn_snapshot();
    let tool_calls: Vec<serde_json::Value> = snap
        .tool_calls
        .iter()
        .map(|tc| {
            serde_json::json!({
                "id": tc.id,
                "name": tc.name,
                "arguments": tc.arguments,
                "result": tc.result,
                "output": tc.result,
                "status": if tc.status == "ok" { "completed" } else { &tc.status },
                "duration_s": (tc.duration_ms as f64) / 1000.0,
            })
        })
        .collect();
    let planning_steps = if snap.current_step.is_empty() {
        Vec::new()
    } else {
        vec![serde_json::json!({
            "text": snap.current_step,
            "status": if snap.running { "running" } else { "completed" },
            "type": "step",
        })]
    };
    let files_created: Vec<String> = snap
        .diffs
        .iter()
        .filter(|d| d.status == "created")
        .map(|d| d.path.clone())
        .collect();
    let mut files_touched: Vec<String> = Vec::new();
    for d in &snap.diffs {
        if !files_touched.contains(&d.path) {
            files_touched.push(d.path.clone());
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "running": snap.running,
        "conv_id": snap.conv_id,
        "started_at": snap.started_at,
        "elapsed_s": snap.elapsed_s,
        "tokens": snap.tokens,
        "cost_usd": snap.cost_usd,
        "agent_mode": hm.agent_mode(),
        "reasoning_effort": hm.settings().reasoning_effort,
        "caveman_mode": hm.caveman_mode(),
        "ponytail_mode": hm.ponytail_mode(),
        "thinking": snap.thinking,
        "text": snap.streaming_text,
        "planningSteps": planning_steps,
        "planning_steps": planning_steps,
        "toolCalls": tool_calls,
        "tool_calls": tool_calls,
        "filesCreated": files_created,
        "files_touched": files_touched,
        "diffs": snap.diffs,
        "subagents": hm.subagents(),
    }))
}

async fn handle_harness_stream(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "live_turn": hm.live_turn_snapshot(),
        "active_conversation": hm.active_conversation(),
    }))
}

async fn handle_harness_chat(
    State(hm): State<Arc<HarnessManager>>,
    Json(payload): Json<ChatPayload>,
) -> Json<serde_json::Value> {
    if let Some(ws) = payload.workspace.as_deref().filter(|s| !s.trim().is_empty()) {
        hm.add_workspace(ws);
    }
    if let Some(m) = payload.model.filter(|s| !s.trim().is_empty()) {
        let mut s = hm.settings();
        s.model = m;
        if let Some(p) = payload.provider.filter(|s| !s.trim().is_empty()) {
            s.provider = p;
        }
        if let Some(eff) = payload.reasoning_effort {
            s.reasoning_effort = eff;
        }
        hm.update_settings(s);
    }
    if let Some(mode) = payload.agent_mode.filter(|s| !s.trim().is_empty()) {
        hm.set_agent_mode(&mode);
    }
    if let Some(caveman) = payload.caveman_mode {
        hm.set_caveman_mode(caveman);
    }
    if let Some(ponytail) = payload.ponytail_mode {
        hm.set_ponytail_mode(ponytail);
    }

    let mut effective_prompt = payload.prompt;
    if let Some(atts) = payload.attachments {
        for att in atts {
            let name = att
                .get("name")
                .or_else(|| att.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("attachment");
            if let Some(content) = att.get("content").or_else(|| att.get("text")).and_then(|v| v.as_str()) {
                effective_prompt.push_str(&format!("\n\n[Attached File `{}`]:\n{}", name, content));
            } else {
                effective_prompt.push_str(&format!("\n\n[Attached File: `{}`]", name));
            }
        }
    }
    if let Some(imgs) = payload.images {
        for (idx, img) in imgs.iter().enumerate() {
            let label = img
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("image_{}", idx + 1));
            effective_prompt.push_str(&format!("\n\n[Attached Image: `{}`]", label));
        }
    }
    if let Some(tools_val) = payload.tools {
        match tools_val {
            serde_json::Value::Array(arr) => {
                let names: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect();
                if !names.is_empty() {
                    effective_prompt.push_str(&format!(
                        "\n\n[Preferred Tools for this turn: {}]",
                        names.join(", ")
                    ));
                }
            }
            serde_json::Value::Object(map) => {
                let disabled: Vec<String> = map
                    .iter()
                    .filter_map(|(k, v)| {
                        if v.as_bool() == Some(false) {
                            Some(k.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !disabled.is_empty() {
                    let enabled: Vec<String> = map
                        .iter()
                        .filter_map(|(k, v)| {
                            if v.as_bool().unwrap_or(true) {
                                Some(k.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !enabled.is_empty() {
                        effective_prompt.push_str(&format!(
                            "\n\n[Preferred Tools for this turn: {}]",
                            enabled.join(", ")
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(sub_id) = payload.subagent_id.filter(|s| !s.trim().is_empty()) {
        effective_prompt = format!(
            "[Subagent `{}` Task]: {}",
            sub_id.trim(),
            effective_prompt
        );
    }

    let updated_conv = hm
        .execute_prompt_turn(payload.conversation_id.as_deref(), effective_prompt)
        .await;
    let workspaces = hm.workspaces();
    let conv_json = format_conversation_for_ui(&updated_conv, &workspaces);

    Json(serde_json::json!({
        "status": "ok",
        "conversation_id": updated_conv.id,
        "conversation": conv_json,
        "subagents": hm.subagents(),
    }))
}

async fn handle_harness_exec(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let cmd_str = body
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let ws_path = body
        .get("workspace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    if cmd_str.is_empty() {
        return Json(serde_json::json!({
            "status": "ok",
            "output": "",
            "returncode": 0,
        }));
    }

    let timeout_secs = hm.settings().command_timeout.clamp(5, 3600);
    let res = tokio::task::spawn_blocking(move || {
        let ws = Workspace::new(&ws_path).or_else(|_| Workspace::new("."))?;
        let te = ToolEngine::new(ws);
        te.run_command_with_code(&cmd_str, timeout_secs)
    })
    .await;

    match res {
        Ok(Ok((output, code))) => Json(serde_json::json!({
            "status": if code == 0 { "ok" } else { "error" },
            "output": output,
            "returncode": code,
        })),
        Ok(Err(e)) => Json(serde_json::json!({
            "status": "error",
            "output": format!("Command execution failed: {}", e),
            "returncode": 1,
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "output": format!("Join error: {}", e),
            "returncode": 1,
        })),
    }
}

async fn handle_harness_abort(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    hm.abort_turn();
    Json(serde_json::json!({ "status": "aborted" }))
}

async fn handle_harness_undo(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let conv_id = body
        .get("conv_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let (restored, deleted) = hm.undo_last_turn(&conv_id);
    let workspaces = hm.workspaces();
    let conv = hm.active_conversation();
    let ws_files = collect_workspace_files_json(&hm.active_workspace_path());

    Json(serde_json::json!({
        "status": "ok",
        "conversation": format_conversation_for_ui(&conv, &workspaces),
        "workspace_files": ws_files,
        "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
        "restored_files": restored,
        "deleted_files": deleted,
    }))
}

async fn handle_harness_compact(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    hm.compact_active_conversation();
    let workspaces = hm.workspaces();
    let conv = hm.active_conversation();
    Json(serde_json::json!({
        "status": "ok",
        "conversation": format_conversation_for_ui(&conv, &workspaces),
    }))
}

async fn handle_harness_mode(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(m) = body
        .get("agent_mode")
        .or_else(|| body.get("mode"))
        .and_then(|v| v.as_str())
    {
        hm.set_agent_mode(m);
    }
    if let Some(c) = body.get("caveman_mode").and_then(|v| v.as_bool()) {
        hm.set_caveman_mode(c);
    }
    if let Some(p) = body.get("ponytail_mode").and_then(|v| v.as_bool()) {
        hm.set_ponytail_mode(p);
    }
    Json(serde_json::json!({
        "status": "ok",
        "agent_mode": hm.agent_mode(),
        "caveman_mode": hm.caveman_mode(),
        "ponytail_mode": hm.ponytail_mode(),
    }))
}

async fn handle_get_modifiers(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "caveman_mode": hm.caveman_mode(),
        "ponytail_mode": hm.ponytail_mode(),
    }))
}

async fn handle_post_modifiers(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(c) = body.get("caveman_mode").and_then(|v| v.as_bool()) {
        hm.set_caveman_mode(c);
    }
    if let Some(p) = body.get("ponytail_mode").and_then(|v| v.as_bool()) {
        hm.set_ponytail_mode(p);
    }
    Json(serde_json::json!({
        "status": "ok",
        "caveman_mode": hm.caveman_mode(),
        "ponytail_mode": hm.ponytail_mode(),
    }))
}

async fn handle_revert_diff(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Support both direct `FileDiff` payload AND `{ conv_id, turn_id, file, hunk_idx }` from the UI
    if let Ok(direct_diff) = serde_json::from_value::<FileDiff>(body.clone()) {
        return match hm.revert_diff(&direct_diff) {
            Ok(()) => {
                hm.remove_diff_from_conversation("", &direct_diff.path);
                let workspaces = hm.workspaces();
                let conv = hm.active_conversation();
                let ws_files = collect_workspace_files_json(&hm.active_workspace_path());
                Json(serde_json::json!({
                    "status": "ok",
                    "reverted_file": direct_diff.path,
                    "updated_diff": "",
                    "conversation": format_conversation_for_ui(&conv, &workspaces),
                    "workspace_files": ws_files,
                    "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
                }))
            }
            Err(e) => Json(serde_json::json!({ "status": "error", "error": e })),
        };
    }

    let file_target = body
        .get("file")
        .or_else(|| body.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .replace('\\', "/");
    let conv_id = body
        .get("conv_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let convs = hm.conversations();
    let target_conv = convs
        .iter()
        .find(|c| !conv_id.is_empty() && c.id == conv_id)
        .cloned()
        .unwrap_or_else(|| hm.active_conversation());

    let mut found_diff: Option<FileDiff> = None;
    for msg in target_conv.messages.iter().rev() {
        for d in msg.diffs.iter().rev() {
            if d.path.replace('\\', "/") == file_target || file_target.is_empty() {
                found_diff = Some(d.clone());
                break;
            }
        }
        if found_diff.is_some() {
            break;
        }
    }

    if let Some(diff) = found_diff {
        match hm.revert_diff(&diff) {
            Ok(()) => {
                hm.remove_diff_from_conversation(conv_id, &diff.path);
                let workspaces = hm.workspaces();
                let conv = hm.active_conversation();
                let ws_files = collect_workspace_files_json(&hm.active_workspace_path());
                Json(serde_json::json!({
                    "status": "ok",
                    "reverted_file": diff.path,
                    "updated_diff": "",
                    "conversation": format_conversation_for_ui(&conv, &workspaces),
                    "workspace_files": ws_files,
                    "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
                }))
            }
            Err(e) => Json(serde_json::json!({ "status": "error", "error": e })),
        }
    } else {
        Json(serde_json::json!({
            "status": "error",
            "error": format!("No matching diff found for file '{}'", file_target),
        }))
    }
}

async fn handle_command_approve(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let approved = body
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !approved {
        hm.abort_turn();
        return Json(serde_json::json!({ "status": "rejected" }));
    }
    if body
        .get("switch_to_autonomous")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        hm.set_agent_mode("Autonomous");
    } else if let Some(mode) = body.get("agent_mode").and_then(|v| v.as_str()) {
        hm.set_agent_mode(mode);
    }
    if let Some(cmd) = body.get("command").and_then(|v| v.as_str()) {
        if !cmd.trim().is_empty() {
            let ws_path = hm.active_workspace_path();
            let cmd_owned = cmd.trim().to_string();
            let out = tokio::task::spawn_blocking(move || {
                let ws = Workspace::new(&ws_path).or_else(|_| Workspace::new("."))?;
                ToolEngine::new(ws).run_command(&cmd_owned, 120)
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
            return Json(serde_json::json!({
                "status": "ok",
                "output": out,
                "agent_mode": hm.agent_mode(),
            }));
        }
    }
    Json(serde_json::json!({
        "status": "ok",
        "agent_mode": hm.agent_mode(),
    }))
}

async fn handle_question_answer(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut formatted = String::new();
    if let Some(ans) = body.get("answer").and_then(|v| v.as_str()) {
        formatted = ans.trim().to_string();
    } else if let Some(ans_val) = body.get("answers") {
        if let Some(s) = ans_val.as_str() {
            formatted = s.trim().to_string();
        } else if let Some(arr) = ans_val.as_array() {
            let items: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            formatted = items.join("\n");
        } else if let Some(obj) = ans_val.as_object() {
            let items: Vec<String> = obj
                .iter()
                .map(|(k, v)| {
                    let val_str = v
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v.to_string());
                    format!("{}: {}", k, val_str.trim())
                })
                .collect();
            formatted = items.join("\n");
        }
    }
    if !formatted.is_empty() {
        hm.submit_prompt(formatted);
    }
    Json(serde_json::json!({ "status": "ok" }))
}

async fn handle_plan_approve(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let approved = body
        .get("approved")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let feedback_opt = body
        .get("feedback")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if approved {
        hm.set_agent_mode("Autonomous");
        let prompt = feedback_opt.unwrap_or_else(|| {
            "Plan approved. Proceed with full autonomous implementation and verification.".to_string()
        });
        hm.submit_prompt(prompt);
    } else if let Some(feedback) = feedback_opt {
        hm.set_agent_mode("Plan");
        hm.submit_prompt(format!(
            "Please revise the implementation plan based on this feedback:\n{}",
            feedback
        ));
    }
    Json(serde_json::json!({
        "status": "ok",
        "agent_mode": hm.agent_mode(),
    }))
}

async fn handle_subagents_approve(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let conv_id = body
        .get("conv_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(arr) = body
        .get("subagents")
        .or_else(|| body.get("specs"))
        .and_then(|v| v.as_array())
    {
        for item in arr {
            let sub_id = item
                .get("id")
                .or_else(|| item.get("subagent_id"))
                .or_else(|| item.get("role"))
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("sub-coder");
            let task = item
                .get("task")
                .or_else(|| item.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("Execute approved subagent task.");
            hm.spawn_subagent_task_with_conv(sub_id, task, conv_id.as_deref());
        }
    } else if let Some(sub_id) = body
        .get("subagent_id")
        .or_else(|| body.get("id"))
        .or_else(|| body.get("role"))
        .and_then(|v| v.as_str())
    {
        let task = body
            .get("task")
            .or_else(|| body.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("Execute approved subagent task.");
        hm.spawn_subagent_task_with_conv(sub_id, task, conv_id.as_deref());
    }
    Json(serde_json::json!({
        "status": "ok",
        "subagents": hm.subagents(),
    }))
}

async fn handle_create_conversation(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(ws) = body
        .get("workspace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        hm.add_workspace(ws);
    }
    let title = body.get("title").and_then(|v| v.as_str());
    let mode = body.get("mode").and_then(|v| v.as_str());
    let new_id = hm.new_conversation_with_mode(title, mode);
    let workspaces = hm.workspaces();
    let conv = hm
        .conversations()
        .into_iter()
        .find(|c| c.id == new_id)
        .unwrap_or_else(|| hm.active_conversation());
    let ws_path = hm.active_workspace_path();
    let ws_files = collect_workspace_files_json(&ws_path);

    Json(serde_json::json!({
        "status": "ok",
        "conversation": format_conversation_for_ui(&conv, &workspaces),
        "workspaces": workspaces,
        "active_workspace": ws_path.to_string_lossy(),
        "workspace_files": ws_files,
    }))
}

async fn handle_delete_conversation(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    hm.delete_conversation(&id);
    let workspaces = hm.workspaces();
    let convs: Vec<serde_json::Value> = hm
        .conversations()
        .iter()
        .map(|c| format_conversation_for_ui(c, &workspaces))
        .collect();
    let active_conv = hm.active_conversation();
    Json(serde_json::json!({
        "status": "ok",
        "active_conversation_id": hm.active_conversation_id(),
        "conversation": format_conversation_for_ui(&active_conv, &workspaces),
        "conversations": convs,
    }))
}

async fn handle_clear_conversation(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    hm.set_active_conversation(&id);
    hm.clear_active_conversation();
    let workspaces = hm.workspaces();
    let conv = hm.active_conversation();
    Json(serde_json::json!({
        "status": "ok",
        "conversation": format_conversation_for_ui(&conv, &workspaces),
    }))
}

async fn handle_rename_conversation(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(title) = body.get("title").and_then(|v| v.as_str()) {
        hm.rename_conversation(&id, title);
    }
    let workspaces = hm.workspaces();
    let conv = hm
        .conversations()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| hm.active_conversation());
    Json(serde_json::json!({
        "status": "ok",
        "conversation": format_conversation_for_ui(&conv, &workspaces),
    }))
}

async fn handle_add_workspace(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = body
        .get("path")
        .or_else(|| body.get("workspace"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let id = hm.add_workspace(path);
    let ws_path = hm.active_workspace_path();
    let ws_files = collect_workspace_files_json(&ws_path);
    Json(serde_json::json!({
        "status": "ok",
        "workspace_id": id,
        "active_workspace": ws_path.to_string_lossy(),
        "workspaces": hm.workspaces(),
        "workspace_files": ws_files,
        "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
    }))
}

async fn handle_remove_workspace(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
    body: Option<Json<serde_json::Value>>,
) -> Json<serde_json::Value> {
    let target = body
        .as_ref()
        .and_then(|b| {
            b.get("path")
                .or_else(|| b.get("workspace"))
                .or_else(|| b.get("id"))
                .or_else(|| b.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            params
                .get("path")
                .or_else(|| params.get("workspace"))
                .or_else(|| params.get("id"))
                .cloned()
        })
        .unwrap_or_default();

    let removed = hm.remove_workspace(&target);
    let workspaces = hm.workspaces();
    let ws_path = hm.active_workspace_path();
    let ws_files = collect_workspace_files_json(&ws_path);
    let convs: Vec<serde_json::Value> = hm
        .conversations()
        .iter()
        .map(|c| format_conversation_for_ui(c, &workspaces))
        .collect();

    Json(serde_json::json!({
        "status": if removed { "ok" } else { "not_found" },
        "removed": removed,
        "active_workspace": ws_path.to_string_lossy(),
        "workspaces": workspaces,
        "conversations": convs,
        "active_conversation_id": hm.active_conversation_id(),
        "workspace_files": ws_files,
        "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
    }))
}

async fn handle_select_workspace(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    if let Some(id) = body.get("id").and_then(|v| v.as_str()) {
        hm.set_active_workspace(id);
    } else if let Some(path) = body
        .get("path")
        .or_else(|| body.get("workspace"))
        .and_then(|v| v.as_str())
    {
        hm.add_workspace(path);
    }
    let ws_path = hm.active_workspace_path();
    let ws_files = collect_workspace_files_json(&ws_path);
    Json(serde_json::json!({
        "status": "ok",
        "active_workspace": ws_path.to_string_lossy(),
        "workspaces": hm.workspaces(),
        "workspace_files": ws_files,
        "git_info": ws_files.get("git_info").cloned().unwrap_or_default(),
    }))
}

async fn handle_browse_workspace(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .ok()
        .flatten();
    if let Some(folder) = picked {
        let path_str = folder.to_string_lossy().to_string();
        hm.add_workspace(&path_str);
        let ws_files = collect_workspace_files_json(&folder);
        Json(serde_json::json!({
            "status": "ok",
            "cancelled": false,
            "path": path_str,
            "active_workspace": path_str,
            "workspaces": hm.workspaces(),
            "workspace_files": ws_files,
        }))
    } else {
        Json(serde_json::json!({
            "status": "cancelled",
            "cancelled": true,
        }))
    }
}

async fn handle_list_files(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let ws_path = params
        .get("workspace")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());
    Json(collect_workspace_files_json(&ws_path))
}

async fn handle_read_file(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let rel_path = params.get("path").cloned().unwrap_or_default();
    let ws_path = params
        .get("workspace")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    let res = Workspace::new(&ws_path)
        .and_then(|ws| ws.resolve(&rel_path))
        .and_then(|abs| fs::read_to_string(abs).map_err(Into::into));

    match res {
        Ok(content) => Json(serde_json::json!({
            "status": "ok",
            "path": rel_path,
            "content": content,
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e.to_string(),
        })),
    }
}

/// Serves raw file bytes with proper `Content-Type` header so the UI Preview iframe
/// can render HTML, CSS, JS, SVG, and image files directly.
async fn handle_raw_file(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let rel_path = params.get("path").cloned().unwrap_or_default();
    let ws_path = params
        .get("workspace")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    let res = Workspace::new(&ws_path)
        .and_then(|ws| ws.resolve(&rel_path))
        .and_then(|abs| fs::read(&abs).map(|bytes| (abs, bytes)).map_err(Into::into));

    match res {
        Ok((abs, bytes)) => {
            let ext = abs
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let content_type = match ext.as_str() {
                "html" | "htm" => "text/html; charset=utf-8",
                "css" => "text/css; charset=utf-8",
                "js" | "mjs" => "application/javascript; charset=utf-8",
                "json" => "application/json; charset=utf-8",
                "svg" => "image/svg+xml",
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "ico" => "image/x-icon",
                _ => "text/plain; charset=utf-8",
            };
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("File not found: {}", e),
        )
            .into_response(),
    }
}

async fn handle_web_proxy(Query(params): Query<HashMap<String, String>>) -> Response {
    let target_url = params.get("url").cloned().unwrap_or_default();
    if let Err(e) = ToolEngine::validate_url_against_ssrf(&target_url) {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            e.to_string(),
        )
            .into_response();
    }

    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("Too many redirects (max 5)");
        }
        if let Err(e) = ToolEngine::validate_url_against_ssrf(attempt.url().as_str()) {
            return attempt.error(e.to_string());
        }
        attempt.follow()
    });

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .redirect(redirect_policy)
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) UnjessRust/4.0")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    match client.get(&target_url).send().await {
        Ok(mut resp) => {
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html; charset=utf-8")
                .to_string();
            let mut bytes = Vec::new();
            const MAX_PROXY_BYTES: usize = 4 * 1024 * 1024;
            while let Ok(Some(chunk)) = resp.chunk().await {
                if bytes.len() + chunk.len() > MAX_PROXY_BYTES {
                    let take = MAX_PROXY_BYTES.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..take]);
                    break;
                }
                bytes.extend_from_slice(&chunk);
            }
            ([(header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Proxy fetch failed: {}", e),
        )
            .into_response(),
    }
}

async fn handle_browser_action(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("snapshot");
    let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let ws = hm.active_workspace_path();

    if !url.is_empty() && (url.starts_with("http://") || url.starts_with("https://")) {
        let url_owned = url.to_string();
        let text = tokio::task::spawn_blocking(move || {
            let ws_obj = Workspace::new(&ws).or_else(|_| Workspace::new("."))?;
            ToolEngine::new(ws_obj).read_url(&url_owned)
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
        return Json(serde_json::json!({
            "status": "ok",
            "action": action,
            "url": url,
            "snapshot": text,
            "result": {
                "url": url,
                "snapshot": text,
                "elements": [],
            },
        }));
    }

    let fallback_snap = "Workspace live preview active.";
    Json(serde_json::json!({
        "status": "ok",
        "action": action,
        "snapshot": fallback_snap,
        "result": {
            "url": url,
            "snapshot": fallback_snap,
            "elements": [],
        },
    }))
}

async fn handle_write_file(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let rel_path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let ws_path = body
        .get("workspace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    let res = Workspace::new(&ws_path)
        .and_then(|ws| ws.resolve(rel_path))
        .and_then(|abs| {
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(abs, content).map_err(Into::into)
        });

    match res {
        Ok(()) => Json(serde_json::json!({ "status": "ok", "path": rel_path })),
        Err(e) => Json(serde_json::json!({ "status": "error", "error": e.to_string() })),
    }
}

/// Supports deleting a workspace file via either query params (`?path=...`) or JSON body (`{ "path": "...", "workspace": "..." }`).
async fn handle_delete_file(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
    body_bytes: Bytes,
) -> Json<serde_json::Value> {
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let rel_path = params
        .get("path")
        .cloned()
        .or_else(|| {
            body_json
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    let ws_path = params
        .get("workspace")
        .cloned()
        .or_else(|| {
            body_json
                .get("workspace")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    if let Ok(ws) = Workspace::new(&ws_path) {
        if let Ok(resolved) = ws.resolve(&rel_path) {
            if resolved.is_file() {
                let _ = fs::remove_file(resolved);
            }
        }
    }
    let ws_files = collect_workspace_files_json(&ws_path);
    Json(serde_json::json!({
        "status": "ok",
        "deleted": rel_path,
        "workspace_files": ws_files,
    }))
}

async fn handle_models(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let mut settings = hm.settings();
    if let Some(u) = params
        .get("ollama_url")
        .or_else(|| params.get("ollama_base_url"))
        .or_else(|| params.get("endpoint"))
        .filter(|s| !s.trim().is_empty())
    {
        settings.ollama_base_url = u.trim().to_string();
    }
    if let Some(u) = params
        .get("llamacpp_url")
        .or_else(|| params.get("llamacpp_base_url"))
        .filter(|s| !s.trim().is_empty())
    {
        settings.llamacpp_base_url = u.trim().to_string();
    }
    if let Some(u) = params
        .get("lmstudio_url")
        .or_else(|| params.get("lmstudio_base_url"))
        .filter(|s| !s.trim().is_empty())
    {
        settings.lmstudio_base_url = u.trim().to_string();
    }
    if let Some(k) = params
        .get("ollama_api_key")
        .or_else(|| params.get("ollama_key"))
        .or_else(|| params.get("key"))
        .filter(|s| !s.trim().is_empty())
    {
        settings.api_keys.insert("ollama".to_string(), k.trim().to_string());
    }

    let endpoint = settings.ollama_base_url.clone();
    let discovered = tokio::task::spawn_blocking(move || settings.discover_models())
        .await
        .unwrap_or_default();

    let models: Vec<serde_json::Value> = discovered
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "name": m.name,
                "provider": m.provider,
                "is_cloud": m.is_cloud,
                "local": !m.is_cloud,
                "has_tools": m.has_tools,
                "has_thinking": m.has_thinking,
                "supports_effort": m.supports_effort,
                "desc": m.desc,
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "online",
        "ollama_online": true,
        "endpoint": endpoint,
        "default_model": hm.settings().model,
        "models": models,
    }))
}

async fn handle_get_settings(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let s = hm.settings();
    let mut obj = serde_json::to_value(&s).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(map) = obj.as_object_mut() {
        map.insert(
            "default_model".to_string(),
            serde_json::Value::String(s.model.clone()),
        );
        map.insert(
            "ollama_base_url".to_string(),
            serde_json::Value::String(s.ollama_base_url.clone()),
        );
        map.insert(
            "current_version".to_string(),
            serde_json::Value::String(CURRENT_VERSION.to_string()),
        );
        for (k, v) in &s.api_keys {
            map.insert(
                format!("{}_api_key", k.replace('-', "_")),
                serde_json::Value::String(v.clone()),
            );
        }
        let nested = serde_json::Value::Object(map.clone());
        map.insert("settings".to_string(), nested);
        map.insert(
            "status".to_string(),
            serde_json::Value::String("ok".to_string()),
        );
    }
    Json(obj)
}

async fn handle_post_settings(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut s = hm.settings();
    if let Some(obj) = body.as_object() {
        let given_model = obj
            .get("default_model")
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let given_provider = obj
            .get("provider")
            .and_then(|v| v.as_str())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        match (given_model, given_provider) {
            (Some(m), Some(p)) => {
                s.model = m;
                s.provider = p;
                s.sync_provider_with_model();
            }
            (Some(m), None) => {
                s.model = m;
                s.sync_provider_with_model();
            }
            (None, Some(p)) => {
                let norm_p = crate::llm::normalize_provider_id(&p);
                let current_resolved = crate::llm::resolve_provider_for_model(&s, &s.model);
                s.provider = p;
                if current_resolved != norm_p
                    && !(current_resolved == "ollama" && norm_p == "ollama-api")
                {
                    s.model = crate::llm::default_model_for_provider(&s, &norm_p);
                }
                s.sync_provider_with_model();
            }
            (None, None) => {}
        }
        if let Some(e) = obj
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
        {
            s.reasoning_effort = e.trim().to_string();
        }
        if let Some(u) = obj
            .get("ollama_base_url")
            .or_else(|| obj.get("ollama_url"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
        {
            s.ollama_base_url = u.trim().to_string();
        }
        if let Some(u) = obj.get("llamacpp_base_url").and_then(|v| v.as_str()) {
            if !u.trim().is_empty() {
                s.llamacpp_base_url = u.trim().to_string();
            }
        }
        if let Some(u) = obj.get("lmstudio_base_url").and_then(|v| v.as_str()) {
            if !u.trim().is_empty() {
                s.lmstudio_base_url = u.trim().to_string();
            }
        }
        if let Some(u) = obj.get("update_server_url").and_then(|v| v.as_str()) {
            if !u.trim().is_empty() {
                s.update_server_url = u.trim().to_string();
            }
        }
        if let Some(iters) = obj.get("max_iterations").and_then(|v| v.as_u64()) {
            s.max_iterations = iters as usize;
        }
        if let Some(max_cost) = obj.get("max_cost_per_session").and_then(|v| v.as_f64()) {
            s.max_cost_per_session = max_cost;
        }
        if let Some(fb) = obj.get("fallback_models").and_then(|v| v.as_array()) {
            s.fallback_models = fb
                .iter()
                .filter_map(|x| x.as_str().map(|st| st.trim().to_string()))
                .filter(|st| !st.is_empty())
                .collect();
        }
        let key_names = [
            ("openai_api_key", "openai"),
            ("anthropic_api_key", "anthropic"),
            ("google_api_key", "google"),
            ("groq_api_key", "groq"),
            ("openrouter_api_key", "openrouter"),
            ("mistral_api_key", "mistral"),
            ("xai_api_key", "xai"),
            ("cerebras_api_key", "cerebras"),
            ("kimi_api_key", "kimi"),
            ("qwen_api_key", "qwen"),
            ("ollama_api_key", "ollama"),
        ];
        for (field, provider_key) in key_names {
            if let Some(val) = obj.get(field).and_then(|v| v.as_str()) {
                if !val.trim().is_empty() {
                    s.api_keys
                        .insert(provider_key.to_string(), val.trim().to_string());
                }
            }
        }
    }
    hm.update_settings(s);
    Json(serde_json::json!({ "status": "ok" }))
}

async fn handle_get_workflows(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "workflows": hm.workflows(),
    }))
}

async fn handle_post_workflow(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = body
        .get("name")
        .or_else(|| body.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let desc = body
        .get("description")
        .or_else(|| body.get("desc"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let category = body
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general")
        .to_string();
    let prompt = body
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tools: Vec<String> = body
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let steps: Vec<String> = body
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    hm.save_workflow(WorkflowDef {
        id,
        name: name.clone(),
        title: name,
        description: desc,
        category,
        prompt,
        tools,
        steps,
    });
    Json(serde_json::json!({
        "status": "ok",
        "workflows": hm.workflows(),
    }))
}

async fn handle_delete_workflow(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    hm.delete_workflow(&id);
    Json(serde_json::json!({
        "status": "ok",
        "workflows": hm.workflows(),
    }))
}

async fn handle_get_agents(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "agents": hm.subagents(),
    }))
}

async fn handle_post_agent(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let id = body.get("id").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty());
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let role = body.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let prompt = body
        .get("system_prompt")
        .or_else(|| body.get("prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("inherit");
    let tools: Vec<String> = body
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let saved = hm.upsert_subagent(id, name, role, prompt, model, tools);
    Json(serde_json::json!({
        "status": "ok",
        "agent": saved,
        "agents": hm.subagents(),
    }))
}

async fn handle_delete_agent(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    hm.delete_subagent(&id);
    Json(serde_json::json!({
        "status": "ok",
        "agents": hm.subagents(),
    }))
}

async fn handle_run_agent(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let task = body
        .get("task")
        .or_else(|| body.get("prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("Inspect workspace and assist.")
        .to_string();
    let conv_id = body
        .get("conv_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    hm.spawn_subagent_task_with_conv(&id, &task, conv_id.as_deref());
    Json(serde_json::json!({
        "status": "ok",
        "agents": hm.subagents(),
    }))
}

async fn handle_get_subagents(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "subagents": hm.subagents(),
    }))
}

async fn handle_spawn_subagent(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let conv_id = body
        .get("conv_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(arr) = body
        .get("subagents")
        .or_else(|| body.get("specs"))
        .and_then(|v| v.as_array())
    {
        for item in arr {
            let sub_id = item
                .get("id")
                .or_else(|| item.get("subagent_id"))
                .or_else(|| item.get("role"))
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("sub-coder");
            let task = item
                .get("task")
                .or_else(|| item.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("Analyze workspace.");
            hm.spawn_subagent_task_with_conv(sub_id, task, conv_id.as_deref());
        }
    } else {
        let id = body
            .get("id")
            .or_else(|| body.get("subagent_id"))
            .or_else(|| body.get("role"))
            .or_else(|| body.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("sub-coder");
        let task = body
            .get("task")
            .or_else(|| body.get("prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("Analyze workspace.");
        hm.spawn_subagent_task_with_conv(id, task, conv_id.as_deref());
    }
    Json(serde_json::json!({
        "status": "ok",
        "subagents": hm.subagents(),
    }))
}

async fn handle_get_subagent_one(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let sub = hm.subagents().into_iter().find(|s| s.id == id);
    Json(serde_json::json!({
        "status": "ok",
        "subagent": sub,
    }))
}

async fn handle_message_subagent(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let msg = body
        .get("message")
        .or_else(|| body.get("prompt"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !msg.trim().is_empty() {
        hm.spawn_subagent_task(&id, msg.trim());
    }
    Json(serde_json::json!({
        "status": "ok",
        "subagents": hm.subagents(),
    }))
}

async fn handle_abort_subagent(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    hm.abort_subagent(&id);
    Json(serde_json::json!({
        "status": "ok",
        "subagents": hm.subagents(),
    }))
}

fn enrich_mcp_servers(servers: &[McpServerConfig]) -> (Vec<serde_json::Value>, usize) {
    let mut total_tools = 0usize;
    let enriched = servers
        .iter()
        .map(|s| {
            let count = s.cached_tools.len();
            if s.enabled {
                total_tools += count;
            }
            let mut val = serde_json::to_value(s).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("tools_count".to_string(), serde_json::json!(count));
                obj.insert("tools".to_string(), serde_json::json!(s.cached_tools));
            }
            val
        })
        .collect();
    (enriched, total_tools)
}

async fn handle_get_mcp() -> Json<serde_json::Value> {
    let servers = McpManager::load();
    let (enriched, total_tools) = enrich_mcp_servers(&servers);
    Json(serde_json::json!({
        "status": "ok",
        "servers": enriched,
        "tools_count": total_tools,
    }))
}

async fn handle_refresh_mcp() -> Json<serde_json::Value> {
    let probes = tokio::task::spawn_blocking(McpManager::probe_all_enabled)
        .await
        .unwrap_or_default();
    let servers = McpManager::load();
    let (enriched, total_tools) = enrich_mcp_servers(&servers);
    Json(serde_json::json!({
        "status": "ok",
        "servers": enriched,
        "tools_count": total_tools,
        "probes": probes,
    }))
}

async fn handle_post_mcp(
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut servers = McpManager::load();
    if let Some(arr) = body.as_array() {
        if let Ok(parsed) =
            serde_json::from_value::<Vec<McpServerConfig>>(serde_json::Value::Array(arr.clone()))
        {
            servers = parsed;
        }
    } else if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        let mut command = body
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("npx")
            .to_string();
        let mut url = body
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if url.trim().is_empty()
            && (command.trim().starts_with("http://") || command.trim().starts_with("https://"))
        {
            url = command.trim().to_string();
            command.clear();
        }
        let description = body
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("Configured MCP server")
            .to_string();
        let args: Vec<String> = body
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let env: HashMap<String, String> = body
            .get("env")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let headers: HashMap<String, String> = body
            .get("headers")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let cwd: Option<String> = body
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let enabled = body
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let existing_cached = servers
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .map(|s| {
                (
                    s.cached_tools.clone(),
                    s.last_probe_ok,
                    s.last_error.clone(),
                )
            })
            .unwrap_or_else(|| (Vec::new(), None, None));

        if let Some(pos) = servers
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
        {
            servers[pos] = McpServerConfig {
                name: name.to_string(),
                command,
                url,
                args,
                env,
                cwd,
                headers,
                enabled,
                description,
                cached_tools: existing_cached.0,
                last_probe_ok: existing_cached.1,
                last_error: existing_cached.2,
            };
        } else {
            servers.push(McpServerConfig {
                name: name.to_string(),
                command,
                url,
                args,
                env,
                cwd,
                headers,
                enabled,
                description,
                cached_tools: existing_cached.0,
                last_probe_ok: existing_cached.1,
                last_error: existing_cached.2,
            });
        }
    }
    let _ = McpManager::save(&servers);
    let (enriched, total_tools) = enrich_mcp_servers(&servers);
    Json(serde_json::json!({
        "status": "ok",
        "servers": enriched,
        "tools_count": total_tools,
    }))
}

async fn handle_delete_mcp(AxumPath(name): AxumPath<String>) -> Json<serde_json::Value> {
    let mut servers = McpManager::load();
    servers.retain(|s| !s.name.eq_ignore_ascii_case(&name));
    let _ = McpManager::save(&servers);
    let (enriched, total_tools) = enrich_mcp_servers(&servers);
    Json(serde_json::json!({
        "status": "ok",
        "servers": enriched,
        "tools_count": total_tools,
    }))
}

async fn handle_test_mcp(AxumPath(name): AxumPath<String>) -> Json<serde_json::Value> {
    let servers = McpManager::load();
    if let Some(srv) = servers
        .into_iter()
        .find(|s| s.name.eq_ignore_ascii_case(&name))
    {
        let probe = tokio::task::spawn_blocking(move || McpManager::probe_server(&srv))
            .await
            .ok();
        let updated_servers = McpManager::load();
        let (enriched, total_tools) = enrich_mcp_servers(&updated_servers);
        Json(serde_json::json!({
            "status": "ok",
            "probe": probe,
            "servers": enriched,
            "tools_count": total_tools,
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "error": "MCP server not found",
        }))
    }
}

async fn handle_get_skills(State(hm): State<Arc<HarnessManager>>) -> Json<serde_json::Value> {
    let ws = hm.active_workspace_path();
    Json(serde_json::json!({
        "status": "ok",
        "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
    }))
}

async fn handle_post_skill(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let existing_id = body
        .get("id")
        .or_else(|| body.get("original_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let desc = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = body
        .get("instructions")
        .or_else(|| body.get("content"))
        .or_else(|| body.get("body"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let scope = body
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("global");
    let repo_url = body
        .get("repo_url")
        .or_else(|| body.get("repo"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let folder = body
        .get("folder")
        .or_else(|| body.get("category"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mut triggers: Vec<String> = Vec::new();
    if let Some(arr) = body.get("triggers").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                let t = s.trim();
                if !t.is_empty() {
                    triggers.push(t.to_string());
                }
            }
        }
    } else if let Some(s) = body.get("triggers").and_then(|v| v.as_str()) {
        for part in s.split(',') {
            let t = part.trim();
            if !t.is_empty() {
                triggers.push(t.to_string());
            }
        }
    }

    let ws = body
        .get("workspace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| hm.active_workspace_path());

    match SkillsManager::save_skill_full_with_id(
        existing_id,
        name,
        desc,
        content,
        &triggers,
        scope,
        repo_url,
        folder,
        Some(&ws),
    ) {
        Ok(saved) => Json(serde_json::json!({
            "status": "ok",
            "skill": saved,
            "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
            "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
        })),
    }
}

async fn handle_install_github_skill(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut repo_raw = body
        .get("repo")
        .or_else(|| body.get("url"))
        .or_else(|| body.get("repo_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_start_matches("http://github.com/")
        .trim_end_matches(".git")
        .trim_matches('/')
        .to_string();

    let mut skill_path = body
        .get("skill_path")
        .or_else(|| body.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .trim_matches('/')
        .to_string();

    let mut branch_hint: Option<String> = None;

    // If repo_raw contains `/tree/<branch>/<subpath>` or `/blob/<branch>/<subpath>`, extract `owner/repo`, `branch`, and `subpath`
    for marker in ["/tree/", "/blob/"] {
        if let Some(pos) = repo_raw.find(marker) {
            let base = repo_raw[..pos].trim_matches('/').to_string();
            let rest = &repo_raw[pos + marker.len()..];
            if let Some(slash_idx) = rest.find('/') {
                branch_hint = Some(rest[..slash_idx].trim().to_string());
                if skill_path.is_empty() {
                    skill_path = rest[slash_idx + 1..].trim_matches('/').to_string();
                }
            } else if !rest.trim().is_empty() {
                branch_hint = Some(rest.trim().to_string());
            }
            repo_raw = base;
            break;
        }
    }

    if repo_raw.is_empty() {
        return Json(serde_json::json!({
            "status": "error",
            "error": "GitHub repository (owner/repo) is required",
        }));
    }

    let repo_segments: Vec<&str> = repo_raw.split('/').collect();
    let valid_repo = repo_segments.len() == 2
        && repo_segments.iter().all(|seg| {
            !seg.is_empty()
                && *seg != "."
                && *seg != ".."
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        });
    if !valid_repo {
        return Json(serde_json::json!({
            "status": "error",
            "error": "Invalid GitHub repository format: expected 'owner/repo' with alphanumeric, '-', '_', or '.' characters.",
        }));
    }

    if skill_path.contains("..")
        || skill_path
            .chars()
            .any(|c| matches!(c, '@' | '\\' | '?' | '#' | ':'))
    {
        return Json(serde_json::json!({
            "status": "error",
            "error": "Invalid skill_path: path traversal or URL authority/query characters are forbidden.",
        }));
    }

    let ws = hm.active_workspace_path();
    match SkillsManager::install_from_github(
        &repo_raw,
        branch_hint.as_deref(),
        &skill_path,
        Some(&ws),
    )
    .await
    {
        Ok((installed, saved)) => Json(serde_json::json!({
            "status": "ok",
            "installed": installed,
            "skill": saved,
            "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
        })),
    }
}

async fn handle_get_skill_file(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let skill_id = params
        .get("skill")
        .or_else(|| params.get("id"))
        .or_else(|| params.get("name"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let rel_path = params
        .get("path")
        .or_else(|| params.get("file"))
        .map(|s| s.as_str())
        .unwrap_or("SKILL.md");
    let ws = hm.active_workspace_path();
    match SkillsManager::read_skill_file(skill_id, rel_path, Some(&ws)) {
        Ok(content) => Json(serde_json::json!({
            "status": "ok",
            "skill": skill_id,
            "path": rel_path,
            "content": content,
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
        })),
    }
}

async fn handle_post_skill_file(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let skill_id = body
        .get("skill")
        .or_else(|| body.get("id"))
        .or_else(|| body.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let rel_path = body
        .get("path")
        .or_else(|| body.get("file"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let ws = hm.active_workspace_path();
    match SkillsManager::write_skill_file(skill_id, rel_path, content, Some(&ws)) {
        Ok(updated_skill) => Json(serde_json::json!({
            "status": "ok",
            "skill": updated_skill,
            "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
        })),
    }
}

async fn handle_delete_skill_file(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let skill_id = params
        .get("skill")
        .or_else(|| params.get("id"))
        .or_else(|| params.get("name"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let rel_path = params
        .get("path")
        .or_else(|| params.get("file"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let ws = hm.active_workspace_path();
    match SkillsManager::delete_skill_file(skill_id, rel_path, Some(&ws)) {
        Ok(updated_skill) => Json(serde_json::json!({
            "status": "ok",
            "skill": updated_skill,
            "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
        })),
    }
}

async fn handle_open_skill_folder(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let skill_id = body
        .get("skill")
        .or_else(|| body.get("id"))
        .or_else(|| body.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let ws = hm.active_workspace_path();
    let skills = SkillsManager::list_skills_for_workspace(Some(&ws));
    let target_slug = crate::skills::slugify_skill_id(skill_id);
    if let Some(sk) = skills.into_iter().find(|s| {
        s.id.eq_ignore_ascii_case(skill_id.trim())
            || s.name.eq_ignore_ascii_case(skill_id.trim())
            || crate::skills::slugify_skill_id(&s.name) == target_slug
    }) {
        let dir_to_open = if !sk.dir_path.is_empty() && PathBuf::from(&sk.dir_path).is_dir() {
            PathBuf::from(&sk.dir_path)
        } else {
            let rust_dir = SkillsManager::skills_dir().join(&sk.id);
            let _ = std::fs::create_dir_all(&rust_dir);
            let _ = std::fs::write(rust_dir.join("SKILL.md"), &sk.content);
            rust_dir
        };
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer.exe")
                .arg(&dir_to_open)
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&dir_to_open).spawn();
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&dir_to_open)
                .spawn();
        }
        Json(serde_json::json!({
            "status": "ok",
            "dir_path": dir_to_open.to_string_lossy().to_string(),
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "error": format!("Skill '{}' not found", skill_id),
        }))
    }
}

async fn handle_get_skill_one(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    let ws = hm.active_workspace_path();
    let skill = SkillsManager::list_skills_for_workspace(Some(&ws))
        .into_iter()
        .find(|s| s.name == name || s.id == name);
    Json(serde_json::json!({
        "status": "ok",
        "skill": skill,
    }))
}

async fn handle_delete_skill(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(name): AxumPath<String>,
) -> Json<serde_json::Value> {
    let ws = hm.active_workspace_path();
    let _ = SkillsManager::delete_skill_for_workspace(&name, Some(&ws));
    Json(serde_json::json!({
        "status": "ok",
        "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
    }))
}

async fn handle_delete_skill_query(
    State(hm): State<Arc<HarnessManager>>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let ws = hm.active_workspace_path();
    if let Some(name) = params.get("name").or_else(|| params.get("id")) {
        let _ = SkillsManager::delete_skill_for_workspace(name, Some(&ws));
    }
    Json(serde_json::json!({
        "status": "ok",
        "skills": SkillsManager::list_skills_for_workspace(Some(&ws)),
    }))
}

async fn handle_get_memories() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "memories": MemoryStore::load(),
    }))
}

async fn handle_post_memory(
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let key = body
        .get("key")
        .or_else(|| body.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("note");
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let category = body
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let _ = MemoryStore::save_entry(category, key, content);
    Json(serde_json::json!({
        "status": "ok",
        "memories": MemoryStore::load(),
    }))
}

async fn handle_delete_memory(
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    if let Some(id) = params.get("id").or_else(|| params.get("key")) {
        let _ = MemoryStore::delete_entry(id);
    }
    Json(serde_json::json!({
        "status": "ok",
        "memories": MemoryStore::load(),
    }))
}

async fn handle_get_schedules() -> Json<serde_json::Value> {
    let jobs = SchedulerManager::load();
    Json(serde_json::json!({
        "status": "ok",
        "schedules": jobs,
        "jobs": jobs,
    }))
}

async fn handle_post_schedule(
    State(hm): State<Arc<HarnessManager>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut jobs = SchedulerManager::load();
    let title = body
        .get("title")
        .or_else(|| body.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("Scheduled Task")
        .to_string();
    let cron_expr = body
        .get("cron_expression")
        .or_else(|| body.get("cron_or_interval"))
        .or_else(|| body.get("cron_expr"))
        .or_else(|| body.get("cron"))
        .and_then(|v| v.as_str())
        .unwrap_or("0 * * * *")
        .to_string();
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("prompt")
        .to_string();
    let notify_on = body
        .get("notify_on")
        .and_then(|v| v.as_str())
        .unwrap_or("always")
        .to_string();
    let prompt = body
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let workspace = body
        .get("workspace")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| hm.active_workspace_path().to_string_lossy().to_string());

    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mut task = ScheduledTask {
        id: format!("job-{}", uuid::Uuid::new_v4()),
        name: title.clone(),
        title,
        cron_or_interval: cron_expr.clone(),
        cron_expression: cron_expr.clone(),
        action,
        prompt,
        workspace,
        notify_on,
        enabled,
        last_run: None,
        last_run_at: None,
        next_run_at: Some(SchedulerManager::compute_next_run(&cron_expr)),
        last_status: None,
        last_output: None,
    };
    task.normalize();
    let created_task = task.clone();
    jobs.push(task);
    let _ = SchedulerManager::save(&jobs);
    Json(serde_json::json!({
        "status": "ok",
        "schedule": created_task,
        "schedules": jobs,
        "jobs": jobs,
    }))
}

async fn handle_put_schedule(
    AxumPath(id): AxumPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut jobs = SchedulerManager::load();
    let mut updated_job: Option<ScheduledTask> = None;
    if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
        if let Some(t) = body
            .get("title")
            .or_else(|| body.get("name"))
            .and_then(|v| v.as_str())
        {
            job.title = t.to_string();
            job.name = t.to_string();
        }
        if let Some(c) = body
            .get("cron_expression")
            .or_else(|| body.get("cron_or_interval"))
            .and_then(|v| v.as_str())
        {
            job.cron_expression = c.to_string();
            job.cron_or_interval = c.to_string();
            job.next_run_at = Some(SchedulerManager::compute_next_run(c));
        }
        if let Some(a) = body.get("action").and_then(|v| v.as_str()) {
            job.action = a.to_string();
        }
        if let Some(p) = body.get("prompt").and_then(|v| v.as_str()) {
            job.prompt = p.to_string();
        }
        if let Some(w) = body.get("workspace").and_then(|v| v.as_str()) {
            job.workspace = w.to_string();
        }
        if let Some(n) = body.get("notify_on").and_then(|v| v.as_str()) {
            job.notify_on = n.to_string();
        }
        if let Some(en) = body.get("enabled").and_then(|v| v.as_bool()) {
            job.enabled = en;
        }
        job.normalize();
        updated_job = Some(job.clone());
    }
    let _ = SchedulerManager::save(&jobs);
    Json(serde_json::json!({
        "status": "ok",
        "schedule": updated_job,
        "schedules": jobs,
        "jobs": jobs,
    }))
}

async fn handle_toggle_schedule(AxumPath(id): AxumPath<String>) -> Json<serde_json::Value> {
    let mut jobs = SchedulerManager::load();
    let mut toggled_job: Option<ScheduledTask> = None;
    if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
        job.enabled = !job.enabled;
        if job.enabled {
            job.next_run_at = Some(SchedulerManager::compute_next_run(&job.cron_expression));
        }
        toggled_job = Some(job.clone());
    }
    let _ = SchedulerManager::save(&jobs);
    Json(serde_json::json!({
        "status": "ok",
        "schedule": toggled_job,
        "schedules": jobs,
        "jobs": jobs,
    }))
}

async fn handle_run_schedule(
    State(hm): State<Arc<HarnessManager>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let hm_clone = Arc::clone(&hm);
    let id_clone = id.clone();
    let res = tokio::task::spawn_blocking(move || SchedulerManager::execute_task(&hm_clone, &id_clone))
        .await
        .ok();
    let jobs = SchedulerManager::load();
    match res {
        Some(Ok(task)) => Json(serde_json::json!({
            "status": "ok",
            "schedule": task,
            "schedules": jobs,
            "jobs": jobs,
        })),
        Some(Err(e)) => Json(serde_json::json!({
            "status": "error",
            "error": e,
            "schedules": jobs,
        })),
        None => Json(serde_json::json!({
            "status": "error",
            "error": "Schedule task runner failed",
        })),
    }
}

async fn handle_delete_schedule(AxumPath(id): AxumPath<String>) -> Json<serde_json::Value> {
    let mut jobs = SchedulerManager::load();
    jobs.retain(|j| j.id != id);
    let _ = SchedulerManager::save(&jobs);
    Json(serde_json::json!({
        "status": "ok",
        "schedules": jobs,
        "jobs": jobs,
    }))
}

async fn handle_security_duel(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let ws = hm.active_workspace_path();
    let report = scan_workspace(&ws);
    Json(serde_json::json!({
        "status": "ok",
        "score": report.score,
        "summary": report.summary,
        "findings": report.red_team_findings,
        "red_team_findings": report.red_team_findings,
        "white_team_patches": report.white_team_patches,
        "report": report,
    }))
}

async fn handle_watchdog(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let uptime_s = server_start_instant().elapsed().as_secs();
    let ws_path = hm.active_workspace_path();
    let ws_ok = ws_path.exists();
    let snap = hm.live_turn_snapshot();
    let checked_at = Utc::now().to_rfc3339();
    let mut errors: Vec<serde_json::Value> = Vec::new();
    if !ws_ok {
        errors.push(serde_json::json!({
            "code": "workspace_missing",
            "message": format!("Active workspace path does not exist: {}", ws_path.display()),
        }));
    }
    let summary = if ws_ok {
        format!("Engine nominal (uptime {}s, port 9758)", uptime_s)
    } else {
        "Engine degraded: active workspace path is inaccessible".to_string()
    };
    Json(serde_json::json!({
        "status": "ok",
        "healthy": ws_ok,
        "summary": summary,
        "checked_at": checked_at,
        "errors": errors,
        "watchdog": {
            "healthy": ws_ok,
            "status": if ws_ok { "nominal" } else { "degraded" },
            "summary": summary,
            "checked_at": checked_at,
            "errors": errors,
            "uptime_ok": true,
            "uptime_s": uptime_s,
            "workspace_accessible": ws_ok,
            "active_turn_running": snap.running,
            "engine": "unjess-rust-native",
            "port": 9758
        }
    }))
}

async fn handle_usage(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let settings = hm.settings();
    let conv = hm.active_conversation();
    let all_convs = hm.conversations();
    let live_snap = hm.live_turn_snapshot();

    let now_dt = Utc::now().date_naive();
    // Period buckets: (turns, in_tokens, out_tokens, total_tokens, cost_usd)
    let mut today_b = (0usize, 0usize, 0usize, 0usize, 0.0f64);
    let mut week_b = (0usize, 0usize, 0usize, 0usize, 0.0f64);
    let mut month_b = (0usize, 0usize, 0usize, 0usize, 0.0f64);
    let mut year_b = (0usize, 0usize, 0usize, 0usize, 0.0f64);

    // Aggregate per-model statistics across all conversations
    let mut model_stats: HashMap<String, (usize, usize, usize, usize, f64)> = HashMap::new();

    for c in &all_convs {
        let conv_date = c
            .created_at
            .get(..10)
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .unwrap_or(now_dt);
        let days_old = (now_dt - conv_date).num_days().max(0);

        let mut c_turns = 0usize;
        let mut c_in = 0usize;
        let mut c_out = 0usize;
        let mut c_tok = 0usize;
        let mut c_cost = 0.0f64;

        for m in &c.messages {
            if m.role == "assistant" || (m.role == "system" && m.tokens > 0 && m.model.is_some()) {
                if m.role == "assistant" {
                    c_turns += 1;
                }
                let in_t = if m.input_tokens > 0 {
                    m.input_tokens
                } else {
                    (m.tokens * 2) / 3
                };
                let out_t = if m.output_tokens > 0 {
                    m.output_tokens
                } else {
                    m.tokens.saturating_sub(in_t)
                };
                let tok = m.tokens.max(in_t + out_t);
                c_in += in_t;
                c_out += out_t;
                c_tok += tok;
                c_cost += m.cost_usd;

                let model_id = m
                    .model
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| settings.model.clone());
                let entry = model_stats.entry(model_id).or_insert((0, 0, 0, 0, 0.0));
                if m.role == "assistant" {
                    entry.0 += 1;
                }
                entry.1 += in_t;
                entry.2 += out_t;
                entry.3 += tok;
                entry.4 += m.cost_usd;
            }
        }

        // Reconcile against conversation totals in case messages were compacted
        if c.total_tokens > c_tok {
            let extra_tok = c.total_tokens - c_tok;
            let extra_in = c.total_input_tokens.saturating_sub(c_in);
            let extra_out = c.total_output_tokens.saturating_sub(c_out);
            let extra_cost = (c.total_cost_usd - c_cost).max(0.0);
            c_in += extra_in;
            c_out += extra_out;
            c_tok = c.total_tokens;
            c_cost = c.total_cost_usd.max(c_cost);
            let fallback_model = c
                .messages
                .iter()
                .rev()
                .find_map(|m| m.model.clone())
                .unwrap_or_else(|| settings.model.clone());
            let entry = model_stats
                .entry(fallback_model)
                .or_insert((0, 0, 0, 0, 0.0));
            entry.1 += extra_in;
            entry.2 += extra_out;
            entry.3 += extra_tok;
            entry.4 += extra_cost;
        }

        let add_bucket = |b: &mut (usize, usize, usize, usize, f64)| {
            b.0 += c_turns;
            b.1 += c_in;
            b.2 += c_out;
            b.3 += c_tok;
            b.4 += c_cost;
        };

        if days_old == 0 {
            add_bucket(&mut today_b);
        }
        if days_old <= 7 {
            add_bucket(&mut week_b);
        }
        if days_old <= 30 {
            add_bucket(&mut month_b);
        }
        if days_old <= 365 {
            add_bucket(&mut year_b);
        }
    }

    let session_turns = conv
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .count();
    let session_in = if conv.total_input_tokens > 0 {
        conv.total_input_tokens
    } else {
        (conv.total_tokens * 2) / 3
    };
    let session_out = if conv.total_output_tokens > 0 {
        conv.total_output_tokens
    } else {
        conv.total_tokens.saturating_sub(session_in)
    };
    let session_total_tokens = conv.total_tokens.max(session_in + session_out);

    let mut models_vec: Vec<(String, (usize, usize, usize, usize, f64))> =
        model_stats.into_iter().collect();
    models_vec.sort_by(|a, b| {
        b.1 .4
            .partial_cmp(&a.1 .4)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut models_arr: Vec<serde_json::Value> = models_vec
        .into_iter()
        .map(|(model_id, (turns, in_t, out_t, tok, cost))| {
            let prov = resolve_provider_for_model(&settings, &model_id);
            serde_json::json!({
                "model": model_id,
                "provider": prov,
                "turns_count": turns,
                "input_tokens": in_t,
                "output_tokens": out_t,
                "total_tokens": tok,
                "cost_usd": cost,
            })
        })
        .collect();

    if models_arr.is_empty() {
        models_arr.push(serde_json::json!({
            "model": settings.model,
            "provider": settings.provider,
            "turns_count": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
            "cost_usd": 0.0,
        }));
    }

    let period_obj = |turns: usize, in_t: usize, out_t: usize, tok: usize, cost: f64| {
        serde_json::json!({
            "turns_count": turns,
            "input_tokens": in_t,
            "output_tokens": out_t,
            "total_tokens": tok,
            "cost_usd": cost,
        })
    };

    let inflight_cost_usd = if live_snap.running && live_snap.conv_id == conv.id {
        live_snap.cost_usd
    } else {
        0.0
    };
    let combined_session_cost = conv.total_cost_usd + inflight_cost_usd;
    let remaining_usd = if settings.max_cost_per_session > 0.0 {
        Some((settings.max_cost_per_session - combined_session_cost).max(0.0))
    } else {
        None
    };
    let utilization_pct = if settings.max_cost_per_session > 0.0 {
        ((combined_session_cost / settings.max_cost_per_session) * 100.0).min(100.0)
    } else {
        0.0
    };
    let exceeded =
        settings.max_cost_per_session > 0.0 && combined_session_cost >= settings.max_cost_per_session;

    Json(serde_json::json!({
        "status": "ok",
        "budget": {
            "max_cost_per_session": settings.max_cost_per_session,
            "session_cost_usd": combined_session_cost,
            "inflight_cost_usd": inflight_cost_usd,
            "remaining_usd": remaining_usd,
            "utilization_pct": utilization_pct,
            "exceeded": exceeded,
        },
        "periods": {
            "session": period_obj(session_turns, session_in, session_out, session_total_tokens, combined_session_cost),
            "today": period_obj(today_b.0, today_b.1, today_b.2, today_b.3, today_b.4),
            "week": period_obj(week_b.0, week_b.1, week_b.2, week_b.3, week_b.4),
            "month": period_obj(month_b.0, month_b.1, month_b.2, month_b.3, month_b.4),
            "year": period_obj(year_b.0, year_b.1, year_b.2, year_b.3, year_b.4),
        },
        "models": models_arr,
    }))
}

async fn handle_context(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let ws_path = hm.active_workspace_path();
    let memories = MemoryStore::load();
    let memory_chars: usize = memories
        .iter()
        .map(|m| m.category.len() + m.key.len() + m.content.len() + 6)
        .sum();
    let memory_tokens = memory_chars / 4;

    let sys_prompt = hm.build_system_prompt(&ws_path);
    let raw_sys_tokens = (sys_prompt.len() / 4).max(1);
    let system_prompt_tokens = raw_sys_tokens.saturating_sub(memory_tokens).max(1);

    let tool_schemas_tokens = if hm.agent_mode().eq_ignore_ascii_case("Plan") {
        0usize
    } else {
        let schemas_str = ToolEngine::tool_schemas().to_string();
        (schemas_str.len() / 4).max(1)
    };

    let conv = hm.active_conversation();
    let mut user_prompt_tokens = 0usize;
    let mut agent_response_tokens = 0usize;
    let mut tool_output_tokens = 0usize;

    for m in &conv.messages {
        let content_toks = m.content.len() / 4;
        let thinking_toks = m.thinking.as_ref().map(|t| t.len() / 4).unwrap_or(0);
        match m.role.as_str() {
            "user" => user_prompt_tokens += content_toks,
            "tool" => tool_output_tokens += content_toks,
            _ => {
                agent_response_tokens += content_toks + thinking_toks;
                for tc in &m.tool_calls {
                    agent_response_tokens += tc.arguments.to_string().len() / 4;
                    tool_output_tokens += tc.result.len() / 4;
                }
            }
        }
    }

    let base_overhead = system_prompt_tokens + tool_schemas_tokens + memory_tokens;
    let total_context_tokens =
        base_overhead + user_prompt_tokens + agent_response_tokens + tool_output_tokens;

    let settings = hm.settings();
    let context_limit = if settings.model.contains("gemini") {
        1_000_000usize
    } else if settings.model.contains("claude") || settings.model.contains("gpt-5") {
        200_000usize
    } else {
        128_000usize
    };

    let workspaces = hm.workspaces();
    let sessions: Vec<serde_json::Value> = hm
        .conversations()
        .iter()
        .map(|c| {
            let ws_name = workspaces
                .iter()
                .find(|w| w.id == c.workspace_id)
                .map(|w| w.name.clone())
                .unwrap_or_else(|| "workspace".to_string());
            let msg_tokens: usize = c
                .messages
                .iter()
                .map(|m| {
                    let th = m.thinking.as_ref().map(|t| t.len() / 4).unwrap_or(0);
                    let tc_len: usize = m
                        .tool_calls
                        .iter()
                        .map(|tc| (tc.arguments.to_string().len() + tc.result.len()) / 4)
                        .sum();
                    (m.content.len() / 4) + th + tc_len
                })
                .sum();
            serde_json::json!({
                "id": c.id,
                "title": c.title,
                "workspace": ws_name,
                "model": settings.model,
                "turns_count": c.messages.iter().filter(|m| m.role == "user" || m.role == "assistant").count(),
                "context_tokens": base_overhead + msg_tokens,
                "context_limit": context_limit,
            })
        })
        .collect();

    Json(serde_json::json!({
        "status": "ok",
        "total_context_tokens": total_context_tokens,
        "context_limit": context_limit,
        "components": {
            "system_prompt": system_prompt_tokens,
            "tool_schemas": tool_schemas_tokens,
            "memory": memory_tokens,
            "user_prompts": user_prompt_tokens,
            "agent_responses": agent_response_tokens,
            "tool_outputs": tool_output_tokens,
        },
        "sessions": sessions,
    }))
}

async fn handle_network_info() -> Json<serde_json::Value> {
    let lan_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let companion_url = format!("http://{}:9758", lan_ip);
    Json(serde_json::json!({
        "status": "ok",
        "lan_ip": lan_ip,
        "local_ip": lan_ip,
        "companion_url": companion_url,
        "port": 9758,
    }))
}

async fn handle_update_check(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let url = hm.settings().update_server_url;
    let changelog = crate::version::builtin_changelog();
    match check_for_updates(&url).await {
        Ok(res) => {
            let mut val = serde_json::to_value(res).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "changelog".to_string(),
                    serde_json::to_value(&changelog).unwrap_or_default(),
                );
            }
            Json(val)
        }
        Err(e) => Json(serde_json::json!({
            "current_version": CURRENT_VERSION,
            "latest_version": CURRENT_VERSION,
            "update_available": false,
            "changelog": changelog,
            "error": e,
        })),
    }
}

async fn handle_update_check_post(
    State(hm): State<Arc<HarnessManager>>,
    body_bytes: Bytes,
) -> Json<serde_json::Value> {
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let url = body_json
        .get("server_url")
        .or_else(|| body_json.get("update_server_url"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| hm.settings().update_server_url);
    let changelog = crate::version::builtin_changelog();

    match check_for_updates(&url).await {
        Ok(res) => {
            let mut val = serde_json::to_value(res).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "changelog".to_string(),
                    serde_json::to_value(&changelog).unwrap_or_default(),
                );
            }
            Json(val)
        }
        Err(e) => Json(serde_json::json!({
            "current_version": CURRENT_VERSION,
            "latest_version": CURRENT_VERSION,
            "update_available": false,
            "changelog": changelog,
            "error": e,
        })),
    }
}

async fn handle_update_apply(
    State(hm): State<Arc<HarnessManager>>,
) -> Json<serde_json::Value> {
    let url = hm.settings().update_server_url;
    match check_for_updates(&url).await {
        Ok(info) => {
            if !info.update_available {
                return Json(serde_json::json!({
                    "status": "ok",
                    "applied": false,
                    "message": format!("Unjess {} is already up to date.", CURRENT_VERSION),
                }));
            }
            if info.download_url.is_empty() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Update available but no installer asset URL was provided.",
                }));
            }
            if !info.download_url.starts_with("https://") {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Refusing to apply update: download_url must use https://.",
                }));
            }
            if let Err(ssrf_err) = ToolEngine::validate_url_against_ssrf(&info.download_url) {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": format!("Refusing to download update from unsafe URL: {}", ssrf_err),
                }));
            }
            if info.sha256.trim().is_empty() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Refusing to apply update: remote manifest did not provide a SHA-256 digest.",
                }));
            }
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .user_agent("Unjess-Rust-Updater/4.0")
                .build()
                .unwrap_or_default();
            match client.get(&info.download_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = resp.bytes().await.unwrap_or_default();
                    if let Err(err_msg) = verify_sha256_bytes(&bytes, &info.sha256) {
                        return Json(serde_json::json!({
                            "status": "error",
                            "error": format!("Update package failed cryptographic SHA-256 verification: {}", err_msg),
                        }));
                    }
                    let cfg_dir = crate::config::Settings::config_dir();
                    let _ = fs::create_dir_all(&cfg_dir);
                    let tmp_path = cfg_dir.join("Unjess-Update-Setup.exe.tmp");
                    let setup_path = cfg_dir.join("Unjess-Update-Setup.exe");
                    if let Err(e) = fs::write(&tmp_path, &bytes) {
                        return Json(serde_json::json!({
                            "status": "error",
                            "error": format!("Failed writing temporary update installer: {}", e),
                        }));
                    }
                    if let Err(err_msg) = verify_file_sha256(&tmp_path, &info.sha256) {
                        let _ = fs::remove_file(&tmp_path);
                        return Json(serde_json::json!({
                            "status": "error",
                            "error": format!("Disk verification of update installer failed: {}", err_msg),
                        }));
                    }
                    if fs::rename(&tmp_path, &setup_path).is_err() {
                        if let Err(e) = fs::write(&setup_path, &bytes) {
                            let _ = fs::remove_file(&tmp_path);
                            return Json(serde_json::json!({
                                "status": "error",
                                "error": format!("Failed finalizing update installer: {}", e),
                            }));
                        }
                        let _ = fs::remove_file(&tmp_path);
                    }
                    match Command::new(&setup_path).spawn() {
                        Ok(_) => Json(serde_json::json!({
                            "status": "ok",
                            "applied": true,
                            "version": info.latest_version,
                            "installer": setup_path.to_string_lossy(),
                        })),
                        Err(e) => Json(serde_json::json!({
                            "status": "error",
                            "applied": false,
                            "error": format!("Failed to launch update installer: {}", e),
                        })),
                    }
                }
                Ok(resp) => Json(serde_json::json!({
                    "status": "error",
                    "error": format!("Download failed with HTTP {}", resp.status()),
                })),
                Err(e) => Json(serde_json::json!({
                    "status": "error",
                    "error": format!("Download error: {}", e),
                })),
            }
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e,
        })),
    }
}
