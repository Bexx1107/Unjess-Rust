//! Multi-Provider Async LLM Client (`llm.rs`).
//!
//! Supports:
//! - Ollama (`/api/chat`)
//! - OpenAI-compatible (`/v1/chat/completions` for OpenAI, llama.cpp, LM Studio, Groq,
//!   Mistral, xAI, OpenRouter, Cerebras, Kimi, Qwen, Custom Providers, and automatic
//!   `.gguf` / Windows path routing to local llama.cpp or LM Studio)
//! - Anthropic Messages API (`/v1/messages`)
//! - Google Gemini API (`generateContent` with `x-goog-api-key` header)
//! Includes native function/tool-call extraction, resilient JSON argument recovery,
//! thinking/reasoning extraction, separate input/output token tracking, and per-model USD cost estimation.

use crate::config::Settings;
use crate::tools::{FileDiff, ToolCallRecord, ToolEngine};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// A single conversation message in the chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" | "assistant" | "system" | "tool"
    pub content: String,
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub diffs: Vec<FileDiff>,
    #[serde(default)]
    pub images: Vec<String>,
    pub timestamp: String,
    pub model: Option<String>,
    #[serde(default)]
    pub tokens: usize,
    #[serde(default)]
    pub input_tokens: usize,
    #[serde(default)]
    pub output_tokens: usize,
    #[serde(default)]
    pub cost_usd: f64,
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self {
            role: "user".to_string(),
            content: String::new(),
            thinking: None,
            tool_calls: Vec::new(),
            diffs: Vec::new(),
            images: Vec::new(),
            timestamp: String::new(),
            model: None,
            tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
        }
    }
}

/// Parsed response from a single LLM completion call.
#[derive(Debug, Clone)]
pub struct LlmTurnResponse {
    pub content: String,
    pub thinking: Option<String>,
    pub pending_tool_calls: Vec<(String, String, serde_json::Value)>, // (id, name, args)
    pub tokens: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub cost_usd: f64,
}

/// Determines the provider and endpoint for a given `model` and `Settings`.
pub fn resolve_provider_for_model(settings: &Settings, model: &str) -> String {
    let m = model.trim().to_lowercase();
    let configured_prov = normalize_provider_id(&settings.provider);

    // 1. Check explicit custom providers first (even for slash-prefixed model names)
    for cp in &settings.custom_providers {
        if cp.models.iter().any(|cm| cm.eq_ignore_ascii_case(model.trim())) {
            return cp.id.clone();
        }
    }

    if m.ends_with(".gguf") || m.contains(":\\") || m.contains("llama.cpp") {
        return "llamacpp".to_string();
    }
    if m.contains("lmstudio") {
        return "lmstudio".to_string();
    }

    // 2. Models ending in `:cloud` or containing an Ollama tag `:` (e.g. `deepseek-v4.1-flash:cloud`,
    // `kimi-k3.1-pro:cloud`, `glm-5.3:cloud`, `ornith-1.5:9b`, `qwen2.5-coder:7b`) route to local Ollama
    // (`/api/chat`) unless the user explicitly selected `ollama-api`.
    if m.ends_with(":cloud") {
        if configured_prov == "ollama-api" {
            return "ollama-api".to_string();
        }
        return "ollama".to_string();
    }
    if m.contains(':') && !m.starts_with("http:") && !m.starts_with("https:") {
        return "ollama".to_string();
    }

    // 3. Cloud model prefix heuristics
    if m.starts_with("gemini-")
        || m.starts_with("gemma-")
        || m.starts_with("antigravity-")
        || m.starts_with("deep-research")
        || m.starts_with("learnlm-")
    {
        return "google".to_string();
    }
    if m.starts_with("claude-") {
        return "anthropic".to_string();
    }
    if m.starts_with("gpt-") || m == "o3" || m.starts_with("o4-") || m.starts_with("o1-") {
        return "openai".to_string();
    }
    if m.starts_with("kimi-") || m.starts_with("moonshot-") {
        return "kimi".to_string();
    }
    if m.starts_with("qwen3.")
        || m.starts_with("qwen-")
        || m.starts_with("qwq")
        || m.starts_with("qvq")
        || m.starts_with("deepseek-v4")
        || m.starts_with("glm-5")
    {
        return "qwen".to_string();
    }
    if m.starts_with("grok-") {
        return "xai".to_string();
    }
    if m.starts_with("mistral-") || m.starts_with("codestral") || m.starts_with("pixtral-") {
        return "mistral".to_string();
    }
    if m.starts_with("llama-3.3-")
        || m.starts_with("llama-3.1-")
        || m.starts_with("deepseek-r1-distill-llama")
    {
        return "groq".to_string();
    }
    if m.starts_with("qwen-3-32b") || m.starts_with("llama3.1-8b") {
        return "cerebras".to_string();
    }
    if m.contains('/') && configured_prov != "llamacpp" && configured_prov != "lmstudio" {
        return "openrouter".to_string();
    }
    if configured_prov == "llamacpp" || configured_prov == "lmstudio" {
        return configured_prov;
    }
    if !configured_prov.is_empty() && m == settings.model.trim().to_lowercase() {
        return configured_prov;
    }
    "ollama".to_string()
}

pub fn normalize_provider_id(p: &str) -> String {
    match p.trim().to_lowercase().as_str() {
        "gemini" | "google gemini" => "google".to_string(),
        "claude" => "anthropic".to_string(),
        "moonshot" => "kimi".to_string(),
        "dashscope" | "alibaba" => "qwen".to_string(),
        "llama.cpp" => "llamacpp".to_string(),
        "lm-studio" | "lm studio" => "lmstudio".to_string(),
        "ollama cloud" => "ollama".to_string(),
        other => other.to_string(),
    }
}

/// Returns the preferred default model for a given provider ID.
pub fn default_model_for_provider(settings: &Settings, provider: &str) -> String {
    let norm = normalize_provider_id(provider);
    let discovered = settings.discover_models();
    for m in &discovered {
        let mp = normalize_provider_id(&m.provider);
        if mp == norm || (norm == "ollama-api" && m.id.ends_with(":cloud")) {
            return m.id.clone();
        }
    }
    match norm.as_str() {
        "google" => "gemini-3.8-flash".to_string(),
        "anthropic" => "claude-sonnet-5-5".to_string(),
        "openai" => "gpt-5".to_string(),
        "groq" => "llama-3.3-70b-versatile".to_string(),
        "mistral" => "codestral-latest".to_string(),
        "xai" => "grok-4-latest".to_string(),
        "cerebras" => "qwen-3-32b".to_string(),
        "openrouter" => "openrouter/auto".to_string(),
        "kimi" => "kimi-k3".to_string(),
        "qwen" => "qwen3.8-max".to_string(),
        _ => "qwen2.5-coder:7b".to_string(),
    }
}

/// Resiliently parses tool call arguments from JSON values, stringified JSON,
/// doubly-encoded JSON, or markdown code fences without silently wiping valid objects.
pub fn parse_tool_arguments(raw: &serde_json::Value) -> serde_json::Value {
    if raw.is_object() {
        return raw.clone();
    }
    if let Some(s) = raw.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return serde_json::json!({});
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(inner_str) = val.as_str() {
                if let Ok(inner_val) = serde_json::from_str::<serde_json::Value>(inner_str.trim()) {
                    if inner_val.is_object() {
                        return inner_val;
                    }
                }
            }
            if val.is_object() {
                return val;
            }
        }
        // Strip optional ```json ... ``` fences
        let unfenced = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(unfenced) {
            if val.is_object() {
                return val;
            }
        }
        // Extract first `{ ... }` span if trailing text was emitted
        if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
            if start < end {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&unfenced[start..=end]) {
                    if val.is_object() {
                        return val;
                    }
                }
            }
        }
    }
    serde_json::json!({})
}

fn format_http_error(provider: &str, status: reqwest::StatusCode, body: &str) -> String {
    let snippet: String = body.chars().take(600).collect();
    format!("{} HTTP {}: {}", provider, status, snippet)
}

fn shared_http_client() -> &'static reqwest::Client {
    static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(8)
            .build()
            .unwrap_or_default()
    })
}

pub fn estimate_prompt_tokens(system_prompt: &str, history: &[ChatMessage]) -> usize {
    let hist_chars: usize = history.iter().map(|m| m.content.len() + 8).sum();
    ((system_prompt.len() + hist_chars) / 4).max(1)
}

fn estimate_output_tokens(
    clean_content: &str,
    thinking: &Option<String>,
    tool_calls: &[(String, String, serde_json::Value)],
) -> usize {
    let think_chars = thinking.as_ref().map(|s| s.len()).unwrap_or(0);
    let tc_chars: usize = tool_calls
        .iter()
        .map(|(_, name, args)| name.len() + args.to_string().len() + 12)
        .sum();
    ((clean_content.len() + think_chars + tc_chars) / 4).max(1)
}

pub fn is_transient_llm_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("http 429")
        || lower.contains("http 500")
        || lower.contains("http 502")
        || lower.contains("http 503")
        || lower.contains("http 504")
        || lower.contains("http 529")
        || lower.contains(" 503 ")
        || lower.contains("status 503")
        || lower.contains("high demand")
        || lower.contains("overloaded")
        || lower.contains("service unavailable")
        || lower.contains("\"unavailable\"")
        || lower.contains("resource_exhausted")
        || lower.contains("rate limit")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("connection error")
        || lower.contains("error sending request")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("broken pipe")
}

/// Returns the context window capacity (in tokens) for `model` to power dynamic context-budget compaction.
pub fn model_context_window_tokens(model: &str) -> usize {
    let m = model.trim().to_lowercase();
    if m.starts_with("gemini-")
        || m.starts_with("antigravity-")
        || m.starts_with("learnlm-")
        || m.contains("deepseek-v4")
    {
        1_000_000
    } else if m.starts_with("claude-")
        || m.starts_with("gpt-5")
        || m == "o3"
        || m.starts_with("o3-")
        || m.starts_with("o4-")
        || m.starts_with("kimi-")
        || m.starts_with("moonshot-")
        || m.starts_with("qwen3.")
        || m.starts_with("grok-")
    {
        200_000
    } else if m.starts_with("deepseek-")
        || m.starts_with("glm-")
        || m.starts_with("codestral")
        || m.starts_with("mistral-")
        || m.starts_with("llama-3.3")
        || m.ends_with(":cloud")
        || m.contains('/')
    {
        128_000
    } else {
        32_768
    }
}

/// Prunes older base64 vision images across conversation messages so that only the most recent
/// `max_active_images` images (default 2) retain their raw Base64 payload.
/// This prevents multi-step agent loops (e.g. 40+ turns) from accumulating tens of megabytes
/// of image data that trigger HTTP 400 "request body too large" errors from Ollama, OpenAI, or Claude.
pub fn filter_active_vision_images(history: &[ChatMessage], max_active_images: usize) -> Vec<ChatMessage> {
    let mut cloned = history.to_vec();
    let mut remaining_images = max_active_images;
    for m in cloned.iter_mut().rev() {
        if m.images.is_empty() {
            continue;
        }
        if remaining_images >= m.images.len() {
            remaining_images -= m.images.len();
        } else if remaining_images > 0 {
            let keep_count = remaining_images;
            let start = m.images.len().saturating_sub(keep_count);
            m.images = m.images[start..].to_vec();
            remaining_images = 0;
        } else {
            m.images.clear();
            if !m.content.contains("[Image payload pruned") {
                m.content.push_str(" [Image payload pruned from turn history to conserve context/request body budget]");
            }
        }
    }
    cloned
}

/// Serializes conversation history for OpenAI-compatible endpoints (`/v1/chat/completions`),
/// preserving structured `tool_calls`, `reasoning_content`, and paired `role: "tool"` (`tool_call_id`)
/// messages across multi-step reasoning turns.
pub fn serialize_openai_messages(
    system_prompt: &str,
    history: &[ChatMessage],
) -> Vec<serde_json::Value> {
    let filtered_history = filter_active_vision_images(history, 2);
    let history = &filtered_history;
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    for idx in 0..history.len() {
        let m = &history[idx];
        let paired_with_next_tool = m.role == "assistant"
            && !m.tool_calls.is_empty()
            && idx + 1 < history.len()
            && history[idx + 1].role == "tool"
            && !history[idx + 1].tool_calls.is_empty();
        let paired_with_prev_assistant = m.role == "tool"
            && !m.tool_calls.is_empty()
            && idx > 0
            && history[idx - 1].role == "assistant"
            && !history[idx - 1].tool_calls.is_empty();

        if paired_with_next_tool {
            let tc_json: Vec<serde_json::Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments.to_string()
                        }
                    })
                })
                .collect();
            let mut msg_obj = serde_json::json!({
                "role": "assistant",
                "content": if m.content.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(m.content.clone())
                },
                "tool_calls": tc_json
            });
            let is_latest_assistant = idx >= history.len().saturating_sub(2);
            if is_latest_assistant {
                if let Some(ref th) = m.thinking {
                    if !th.trim().is_empty() {
                        msg_obj["reasoning_content"] = serde_json::Value::String(th.clone());
                    }
                }
            }
            messages.push(msg_obj);
        } else if m.role == "assistant" && !m.tool_calls.is_empty() {
            // Self-contained multi-turn assistant message with historical tool calls & results
            let tc_json: Vec<serde_json::Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments.to_string()
                        }
                    })
                })
                .collect();
            let mut msg_obj = serde_json::json!({
                "role": "assistant",
                "content": if m.content.trim().is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(m.content.clone())
                },
                "tool_calls": tc_json
            });
            let is_latest_assistant = idx >= history.len().saturating_sub(2);
            if is_latest_assistant {
                if let Some(ref th) = m.thinking {
                    if !th.trim().is_empty() {
                        msg_obj["reasoning_content"] = serde_json::Value::String(th.clone());
                    }
                }
            }
            messages.push(msg_obj);
            for tc in &m.tool_calls {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "name": tc.name,
                    "content": tc.result
                }));
            }
        } else if paired_with_prev_assistant {
            for tc in &m.tool_calls {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "name": tc.name,
                    "content": tc.result
                }));
            }
        } else {
            if m.images.is_empty() {
                messages.push(serde_json::json!({
                    "role": if m.role == "tool" { "user" } else { &m.role },
                    "content": m.content
                }));
            } else {
                let mut parts = vec![serde_json::json!({
                    "type": "text",
                    "text": m.content
                })];
                for img in &m.images {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{}", img)
                        }
                    }));
                }
                messages.push(serde_json::json!({
                    "role": if m.role == "tool" { "user" } else { &m.role },
                    "content": parts
                }));
            }
        }
    }
    messages
}

/// Serializes conversation history for Ollama (`/api/chat`), preserving native `tool_calls`,
/// `thinking`, and `role: "tool"` messages across multi-step reasoning turns.
pub fn serialize_ollama_messages(
    system_prompt: &str,
    history: &[ChatMessage],
) -> Vec<serde_json::Value> {
    let filtered_history = filter_active_vision_images(history, 2);
    let history = &filtered_history;
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    for idx in 0..history.len() {
        let m = &history[idx];
        let paired_with_next_tool = m.role == "assistant"
            && !m.tool_calls.is_empty()
            && idx + 1 < history.len()
            && history[idx + 1].role == "tool"
            && !history[idx + 1].tool_calls.is_empty();
        let paired_with_prev_assistant = m.role == "tool"
            && !m.tool_calls.is_empty()
            && idx > 0
            && history[idx - 1].role == "assistant"
            && !history[idx - 1].tool_calls.is_empty();

        if paired_with_next_tool {
            let tc_json: Vec<serde_json::Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    })
                })
                .collect();
            let mut msg_obj = serde_json::json!({
                "role": "assistant",
                "content": m.content,
                "tool_calls": tc_json
            });
            let is_latest_assistant = idx >= history.len().saturating_sub(2);
            if is_latest_assistant {
                if let Some(ref th) = m.thinking {
                    if !th.trim().is_empty() {
                        msg_obj["thinking"] = serde_json::Value::String(th.clone());
                    }
                }
            }
            messages.push(msg_obj);
        } else if m.role == "assistant" && !m.tool_calls.is_empty() {
            // Self-contained multi-turn assistant message with historical tool calls & results
            let tc_json: Vec<serde_json::Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments
                        }
                    })
                })
                .collect();
            let mut msg_obj = serde_json::json!({
                "role": "assistant",
                "content": m.content,
                "tool_calls": tc_json
            });
            let is_latest_assistant = idx >= history.len().saturating_sub(2);
            if is_latest_assistant {
                if let Some(ref th) = m.thinking {
                    if !th.trim().is_empty() {
                        msg_obj["thinking"] = serde_json::Value::String(th.clone());
                    }
                }
            }
            messages.push(msg_obj);
            for tc in &m.tool_calls {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "name": tc.name,
                    "content": tc.result
                }));
            }
        } else if paired_with_prev_assistant {
            for tc in &m.tool_calls {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "name": tc.name,
                    "content": tc.result
                }));
            }
        } else {
            let mut obj = serde_json::json!({
                "role": if m.role == "tool" { "user" } else { &m.role },
                "content": m.content
            });
            if !m.images.is_empty() {
                obj["images"] = serde_json::json!(m.images);
            }
            messages.push(obj);
        }
    }
    messages
}

/// Executes a single async completion turn against the resolved provider with connection pooling
/// and up to 3 automatic retries on transient HTTP 503 (High Demand / Service Unavailable) and 429/5xx errors.
pub async fn complete_llm_turn(
    settings: &Settings,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
) -> Result<LlmTurnResponse, String> {
    complete_llm_turn_filtered(settings, system_prompt, history, enable_tools, None).await
}

/// Executes a single async completion turn with optional subagent tool-schema filtering (`allowed_tools`).
pub async fn complete_llm_turn_filtered(
    settings: &Settings,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
    allowed_tools: Option<&[String]>,
) -> Result<LlmTurnResponse, String> {
    let model = if settings.model.trim().is_empty() {
        "qwen2.5-coder:7b".to_string()
    } else {
        settings.model.trim().to_string()
    };
    let provider = resolve_provider_for_model(settings, &model);
    let client = shared_http_client();
    // Initial attempt + up to 3 retries = 4 total attempts
    let max_retries: usize = if std::env::var("UNJESS_SKIP_LOCAL_PROBE").is_ok() {
        0
    } else {
        3
    };
    let max_attempts = 1 + max_retries;

    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let res = match provider.as_str() {
            "google" | "gemini" => {
                call_gemini(
                    client,
                    settings,
                    &model,
                    system_prompt,
                    history,
                    enable_tools,
                    allowed_tools,
                )
                .await
            }
            "anthropic" | "claude" => {
                call_anthropic(
                    client,
                    settings,
                    &model,
                    system_prompt,
                    history,
                    enable_tools,
                    allowed_tools,
                )
                .await
            }
            "ollama" => {
                call_ollama(
                    client,
                    settings,
                    &model,
                    system_prompt,
                    history,
                    enable_tools,
                    allowed_tools,
                )
                .await
            }
            other => {
                call_openai_compatible(
                    client,
                    settings,
                    other,
                    &model,
                    system_prompt,
                    history,
                    enable_tools,
                    allowed_tools,
                )
                .await
            }
        };

        match res {
            Ok(ok_resp) => return Ok(ok_resp),
            Err(e) => {
                last_err = e;
                if attempt < max_attempts && is_transient_llm_error(&last_err) {
                    // Exponential backoff: 1.5s, 3.0s, 6.0s
                    let backoff_ms = 1500u64 * (1u64 << (attempt - 1).min(3));
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    continue;
                }
                break;
            }
        }
    }

    Err(last_err)
}

async fn call_ollama(
    client: &reqwest::Client,
    settings: &Settings,
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
    allowed_tools: Option<&[String]>,
) -> Result<LlmTurnResponse, String> {
    let raw_base = settings.ollama_base_url.trim_end_matches('/');
    let primary_base = raw_base.replace("http://localhost:", "http://127.0.0.1:");
    let url = format!("{}/api/chat", primary_base);

    let messages = serialize_ollama_messages(system_prompt, history);

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false
    });
    if !model.to_lowercase().ends_with(":cloud") {
        let est_prompt = estimate_prompt_tokens(system_prompt, history);
        let num_ctx = (est_prompt + 4096).next_power_of_two().clamp(16384, 65536);
        body["options"] = serde_json::json!({ "num_ctx": num_ctx });
    }
    if enable_tools {
        body["tools"] = ToolEngine::tool_schemas_filtered(allowed_tools);
    }

    let timeout_secs = settings.llm_timeout.max(15.0) as u64;
    let api_key = settings.get_api_key("ollama").unwrap_or_default();
    let mut req = client
        .post(&url)
        .timeout(Duration::from_secs(timeout_secs))
        .json(&body);
    if !api_key.is_empty() && api_key != "ollama" {
        req = req.bearer_auth(&api_key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Ollama connection error ({}): {}", raw_base, e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_txt = resp.text().await.unwrap_or_default();
        return Err(format_http_error("Ollama", status, &err_txt));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let msg = data.get("message").cloned().unwrap_or_default();
    let raw_content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let native_thinking = msg
        .get("thinking")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (tag_thinking, clean_content) = extract_think_tags(&raw_content);
    let thinking = match (native_thinking, tag_thinking) {
        (Some(a), Some(b)) => Some(format!("{}\n\n{}", a, b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let mut pending_tool_calls = Vec::new();
    if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            if let Some(func) = tc.get("function") {
                let name = func
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = func
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let parsed_args = parse_tool_arguments(&args);
                if !name.is_empty() {
                    pending_tool_calls.push((Uuid::new_v4().to_string(), name, parsed_args));
                }
            }
        }
    }

    let output_tokens = data
        .get("eval_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_output_tokens(&clean_content, &thinking, &pending_tool_calls));
    let input_tokens = data
        .get("prompt_eval_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_prompt_tokens(system_prompt, history));
    let tokens = input_tokens + output_tokens;

    Ok(LlmTurnResponse {
        content: clean_content,
        thinking,
        pending_tool_calls,
        tokens,
        input_tokens,
        output_tokens,
        cost_usd: 0.0,
    })
}

async fn call_openai_compatible(
    client: &reqwest::Client,
    settings: &Settings,
    provider: &str,
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
    allowed_tools: Option<&[String]>,
) -> Result<LlmTurnResponse, String> {
    let base_url = match provider {
        "openai" => "https://api.openai.com/v1".to_string(),
        "groq" => "https://api.groq.com/openai/v1".to_string(),
        "mistral" => "https://api.mistral.ai/v1".to_string(),
        "xai" => "https://api.x.ai/v1".to_string(),
        "openrouter" => "https://openrouter.ai/api/v1".to_string(),
        "cerebras" => "https://api.cerebras.ai/v1".to_string(),
        "kimi" => settings.kimi_base_url.trim_end_matches('/').to_string(),
        "qwen" => settings.qwen_base_url.trim_end_matches('/').to_string(),
        "llamacpp" => {
            let b = settings.llamacpp_base_url.trim_end_matches('/');
            if b.ends_with("/v1") {
                b.to_string()
            } else {
                format!("{}/v1", b)
            }
        }
        "lmstudio" => {
            let b = settings.lmstudio_base_url.trim_end_matches('/');
            if b.ends_with("/v1") {
                b.to_string()
            } else {
                format!("{}/v1", b)
            }
        }
        "ollama-api" => {
            let b = settings.ollama_api_base_url.trim_end_matches('/');
            if b.ends_with("/v1") {
                b.to_string()
            } else {
                format!("{}/v1", b)
            }
        }
        other => {
            if let Some(cp) = settings
                .custom_providers
                .iter()
                .find(|c| c.id.eq_ignore_ascii_case(other))
            {
                cp.base_url.trim_end_matches('/').to_string()
            } else {
                "https://api.openai.com/v1".to_string()
            }
        }
    };

    let url = format!("{}/chat/completions", base_url);
    let api_key = settings.get_api_key(provider).unwrap_or_default();

    let messages = serialize_openai_messages(system_prompt, history);

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
    });
    if enable_tools {
        body["tools"] = ToolEngine::tool_schemas_filtered(allowed_tools);
    }
    let m_lower = model.trim().to_lowercase();
    if (m_lower.starts_with("o1") || m_lower.starts_with("o3") || m_lower.starts_with("o4-"))
        && matches!(settings.reasoning_effort.as_str(), "low" | "medium" | "high")
    {
        body["reasoning_effort"] = serde_json::Value::String(settings.reasoning_effort.clone());
    }

    let timeout_secs = settings.llm_timeout.max(15.0) as u64;
    let mut req = client
        .post(&url)
        .timeout(Duration::from_secs(timeout_secs))
        .json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(&api_key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("{} connection error ({}): {}", provider, url, e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_txt = resp.text().await.unwrap_or_default();
        return Err(format_http_error(provider, status, &err_txt));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let choice = data
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_default();
    let msg = choice.get("message").cloned().unwrap_or_default();

    let raw_content = msg
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let reasoning_field = msg
        .get("reasoning_content")
        .or_else(|| msg.get("reasoning"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (tag_thinking, clean_content) = extract_think_tags(&raw_content);
    let thinking = match (reasoning_field, tag_thinking) {
        (Some(a), Some(b)) => Some(format!("{}\n\n{}", a, b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let mut pending_tool_calls = Vec::new();
    if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tcs {
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if let Some(func) = tc.get("function") {
                let name = func
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args_val = func
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let parsed_args = parse_tool_arguments(&args_val);
                if !name.is_empty() {
                    pending_tool_calls.push((id, name, parsed_args));
                }
            }
        }
    }

    let usage = data.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_prompt_tokens(system_prompt, history));
    let cached_prompt_tokens = usage
        .and_then(|u| u.get("prompt_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0)
        .min(input_tokens);
    let output_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_output_tokens(&clean_content, &thinking, &pending_tool_calls));
    let total_tokens = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(input_tokens + output_tokens);
    let tokens = total_tokens.max(input_tokens + output_tokens);
    let effective_input_tokens =
        input_tokens.saturating_sub(cached_prompt_tokens) + (cached_prompt_tokens / 2);
    let cost_usd = estimate_model_cost(provider, model, effective_input_tokens, output_tokens);

    Ok(LlmTurnResponse {
        content: clean_content,
        thinking,
        pending_tool_calls,
        tokens,
        input_tokens,
        output_tokens,
        cost_usd,
    })
}

async fn call_anthropic(
    client: &reqwest::Client,
    settings: &Settings,
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
    allowed_tools: Option<&[String]>,
) -> Result<LlmTurnResponse, String> {
    let api_key = settings
        .get_api_key("anthropic")
        .ok_or_else(|| "Missing Anthropic API key in Settings -> Cloud Providers".to_string())?;

    let filtered_history = filter_active_vision_images(history, 2);
    let history = &filtered_history;

    // Coalesce consecutive messages of the same role so Anthropic's strict alternating role validator never rejects
    let mut messages: Vec<serde_json::Value> = Vec::new();
    for m in history {
        let mapped_role = if m.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        if m.images.is_empty() {
            if let Some(last) = messages.last_mut() {
                if last.get("role").and_then(|r| r.as_str()) == Some(mapped_role) {
                    if let Some(prev) = last.get("content").and_then(|c| c.as_str()) {
                        last["content"] = serde_json::Value::String(format!("{}\n\n{}", prev, m.content));
                        continue;
                    }
                }
            }
            messages.push(serde_json::json!({
                "role": mapped_role,
                "content": m.content
            }));
        } else {
            let mut parts = Vec::new();
            if !m.content.is_empty() {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": m.content
                }));
            }
            for img in &m.images {
                parts.push(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": img
                    }
                }));
            }
            if parts.is_empty() {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": "..."
                }));
            }
            messages.push(serde_json::json!({
                "role": mapped_role,
                "content": parts
            }));
        }
    }

    let mut tools_list = Vec::new();
    if enable_tools {
        if let Some(arr) = ToolEngine::tool_schemas_filtered(allowed_tools).as_array() {
            for item in arr {
                if let Some(f) = item.get("function") {
                    tools_list.push(serde_json::json!({
                        "name": f.get("name"),
                        "description": f.get("description"),
                        "input_schema": f.get("parameters")
                    }));
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": 16384,
        "system": system_prompt,
        "messages": messages,
    });
    if !tools_list.is_empty() {
        body["tools"] = serde_json::Value::Array(tools_list);
    }

    let timeout_secs = settings.llm_timeout.max(15.0) as u64;
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .timeout(Duration::from_secs(timeout_secs))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Anthropic connection error: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_txt = resp.text().await.unwrap_or_default();
        return Err(format_http_error("Anthropic", status, &err_txt));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut content = String::new();
    let mut thinking = None;
    let mut pending_tool_calls = Vec::new();

    if let Some(blocks) = data.get("content").and_then(|v| v.as_array()) {
        for b in blocks {
            match b.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(t);
                    }
                }
                "thinking" => {
                    if let Some(t) = b.get("thinking").and_then(|v| v.as_str()) {
                        thinking = Some(t.to_string());
                    }
                }
                "tool_use" => {
                    let id = b
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = b
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = b
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    let parsed_input = parse_tool_arguments(&input);
                    if !name.is_empty() {
                        pending_tool_calls.push((id, name, parsed_input));
                    }
                }
                _ => {}
            }
        }
    }

    let usage = data.get("usage");
    let raw_in = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);
    let cache_read = usage
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);
    let cache_create = usage
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);
    let total_in_reported = raw_in + cache_read + cache_create;
    let input_tokens = if total_in_reported > 0 {
        total_in_reported
    } else {
        estimate_prompt_tokens(system_prompt, history)
    };
    let output_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_output_tokens(&content, &thinking, &pending_tool_calls));
    let tokens = input_tokens + output_tokens;
    let effective_input_tokens = if total_in_reported > 0 {
        raw_in + (cache_read / 10) + ((cache_create * 5) / 4)
    } else {
        input_tokens
    };

    Ok(LlmTurnResponse {
        content,
        thinking,
        pending_tool_calls,
        tokens,
        input_tokens,
        output_tokens,
        cost_usd: estimate_model_cost("anthropic", model, effective_input_tokens, output_tokens),
    })
}

fn sanitize_gemini_schema(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut cleaned = serde_json::Map::new();
            for (k, v) in map {
                if matches!(
                    k.as_str(),
                    "$schema" | "additionalProperties" | "$ref" | "definitions" | "$defs"
                ) {
                    continue;
                }
                cleaned.insert(k.clone(), sanitize_gemini_schema(v));
            }
            serde_json::Value::Object(cleaned)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_gemini_schema).collect())
        }
        other => other.clone(),
    }
}

async fn call_gemini(
    client: &reqwest::Client,
    settings: &Settings,
    model: &str,
    system_prompt: &str,
    history: &[ChatMessage],
    enable_tools: bool,
    allowed_tools: Option<&[String]>,
) -> Result<LlmTurnResponse, String> {
    let api_key = settings
        .get_api_key("google")
        .ok_or_else(|| "Missing Google Gemini API key in Settings -> Cloud Providers".to_string())?;

    let api_model = model.trim().trim_start_matches("models/");

    let filtered_history = filter_active_vision_images(history, 2);
    let history = &filtered_history;

    let mut contents: Vec<serde_json::Value> = Vec::new();
    for m in history {
        let role = if m.role == "assistant" {
            "model"
        } else {
            "user"
        };
        let mut parts = Vec::new();
        if !m.content.is_empty() {
            parts.push(serde_json::json!({ "text": m.content }));
        }
        for img in &m.images {
            parts.push(serde_json::json!({
                "inline_data": {
                    "mime_type": "image/png",
                    "data": img
                }
            }));
        }
        if parts.is_empty() {
            parts.push(serde_json::json!({ "text": "..." }));
        }
        if let Some(last) = contents.last_mut() {
            if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                if let Some(p) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                    p.extend(parts);
                    continue;
                }
            }
        }
        contents.push(serde_json::json!({
            "role": role,
            "parts": parts
        }));
    }
    if contents.first().and_then(|c| c.get("role")).and_then(|r| r.as_str()) == Some("model") {
        contents.insert(
            0,
            serde_json::json!({
                "role": "user",
                "parts": [{ "text": "[Context from prior session]" }]
            }),
        );
    }

    let mut func_decls = Vec::new();
    if enable_tools {
        if let Some(arr) = ToolEngine::tool_schemas_filtered(allowed_tools).as_array() {
            for item in arr {
                if let Some(f) = item.get("function") {
                    func_decls.push(sanitize_gemini_schema(f));
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "systemInstruction": {
            "parts": [{ "text": system_prompt }]
        },
        "contents": contents,
    });
    if !func_decls.is_empty() {
        body["tools"] = serde_json::json!([{ "functionDeclarations": func_decls }]);
    }

    let timeout_secs = settings.llm_timeout.max(15.0) as u64;
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        api_model
    );
    let mut resp = client
        .post(&url)
        .timeout(Duration::from_secs(timeout_secs))
        .header("x-goog-api-key", &api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Google Gemini connection error: {}", e))?;

    // If a preview/alias model returns 404 Not Found on a standard key, fallback to gemini-2.5-flash
    if resp.status() == reqwest::StatusCode::NOT_FOUND && api_model != "gemini-2.5-flash" {
        let fallback_url =
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent";
        resp = client
            .post(fallback_url)
            .timeout(Duration::from_secs(timeout_secs))
            .header("x-goog-api-key", &api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Google Gemini connection error: {}", e))?;
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let err_txt = resp.text().await.unwrap_or_default();
        return Err(format_http_error("Google Gemini", status, &err_txt));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let mut content = String::new();
    let mut thought_parts = Vec::new();
    let mut pending_tool_calls = Vec::new();

    if let Some(parts) = data
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for p in parts {
            if p.get("thought").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                    if !t.trim().is_empty() {
                        thought_parts.push(t.trim().to_string());
                    }
                }
            } else if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(t);
            } else if let Some(fc) = p.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = fc
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let parsed_args = parse_tool_arguments(&args);
                if !name.is_empty() {
                    pending_tool_calls.push((Uuid::new_v4().to_string(), name, parsed_args));
                }
            }
        }
    }
    let (tag_thinking, clean_content) = extract_think_tags(&content);
    content = clean_content;
    if let Some(tt) = tag_thinking {
        thought_parts.push(tt);
    }
    let thinking = if thought_parts.is_empty() {
        None
    } else {
        Some(thought_parts.join("\n\n"))
    };

    let usage = data.get("usageMetadata");
    let input_tokens = usage
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_prompt_tokens(system_prompt, history));
    let candidates_tokens = usage
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| estimate_output_tokens(&content, &thinking, &pending_tool_calls));
    let thoughts_tokens = usage
        .and_then(|u| u.get("thoughtsTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);
    let output_tokens = candidates_tokens + thoughts_tokens;
    let total_tokens = usage
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(input_tokens + output_tokens);
    let tokens = total_tokens.max(input_tokens + output_tokens);

    Ok(LlmTurnResponse {
        content,
        thinking,
        pending_tool_calls,
        tokens,
        input_tokens,
        output_tokens,
        cost_usd: estimate_model_cost("google", model, input_tokens, output_tokens),
    })
}

pub fn extract_think_tags(text: &str) -> (Option<String>, String) {
    let mut remaining = text.to_string();
    let mut thoughts: Vec<String> = Vec::new();

    while let Some(start) = remaining.find("<think>") {
        let after_start = start + 7;
        if let Some(rel_end) = remaining[after_start..].find("</think>") {
            let end = after_start + rel_end;
            let block = remaining[after_start..end].trim();
            if !block.is_empty() {
                thoughts.push(block.to_string());
            }
            remaining = format!("{}{}", &remaining[..start], &remaining[end + 8..]);
        } else {
            // Unclosed trailing <think> block
            let block = remaining[after_start..].trim();
            if !block.is_empty() {
                thoughts.push(block.to_string());
            }
            remaining = remaining[..start].to_string();
            break;
        }
    }

    let clean = remaining.trim().to_string();
    let thinking_opt = if thoughts.is_empty() {
        None
    } else {
        Some(thoughts.join("\n\n"))
    };
    (thinking_opt, clean)
}

/// Estimates USD cost using separate input and output token rates per 1M tokens for the given provider and model.
pub fn estimate_model_cost(
    provider: &str,
    model: &str,
    input_tokens: usize,
    output_tokens: usize,
) -> f64 {
    let p = provider.to_lowercase();
    if matches!(p.as_str(), "ollama" | "llamacpp" | "lmstudio") {
        return 0.0;
    }
    let m = model.to_lowercase();
    // (input_usd_per_1m, output_usd_per_1m)
    let (in_rate, out_rate) = if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet") {
        (3.0, 15.0)
    } else if m.contains("haiku") {
        (0.80, 4.0)
    } else if m.contains("gemini-4-argon") {
        (2.0, 8.0)
    } else if m.starts_with("o4-mini") {
        (1.10, 4.40)
    } else if m.starts_with("gpt-4o-mini") || m.contains("flash-lite") {
        (0.15, 0.60)
    } else if m.contains("flash") {
        (0.30, 1.20)
    } else if m.contains("gemini") && m.contains("pro") {
        (1.25, 5.0)
    } else if m.starts_with("o3") || m.starts_with("o1") || m.starts_with("gpt-5") || m.starts_with("gpt-4") {
        (2.50, 10.0)
    } else if m.contains("grok-4") || p == "xai" {
        (3.0, 15.0)
    } else if m.contains("deepseek") {
        (0.55, 2.19)
    } else if m.contains("glm-5") {
        (0.50, 1.50)
    } else if m.contains("codestral") || p == "mistral" {
        (0.30, 0.90)
    } else if p == "groq" || p == "cerebras" {
        (0.50, 0.80)
    } else if p == "kimi" || p == "qwen" {
        (0.80, 2.40)
    } else {
        (1.0, 3.0)
    };

    ((input_tokens as f64) * in_rate + (output_tokens as f64) * out_rate) / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_arguments_resilience() {
        let raw_str = serde_json::Value::String("```json\n{\"path\": \"src/main.rs\"}\n```".to_string());
        let parsed = parse_tool_arguments(&raw_str);
        assert_eq!(parsed.get("path").and_then(|v| v.as_str()), Some("src/main.rs"));

        let double_encoded = serde_json::Value::String("\"{\\\"query\\\":\\\"fn main\\\"}\"".to_string());
        let parsed2 = parse_tool_arguments(&double_encoded);
        assert_eq!(parsed2.get("query").and_then(|v| v.as_str()), Some("fn main"));
    }

    #[test]
    fn test_estimate_model_cost_differentiates_in_out_and_tiers() {
        let local = estimate_model_cost("ollama", "qwen2.5-coder:7b", 100_000, 50_000);
        assert_eq!(local, 0.0);

        let haiku = estimate_model_cost("anthropic", "claude-3-5-haiku", 1_000_000, 1_000_000);
        let opus = estimate_model_cost("anthropic", "claude-4-opus", 1_000_000, 1_000_000);
        assert!(opus > haiku * 10.0);
    }

    #[test]
    fn test_extract_multiple_and_unclosed_think_tags() {
        let multi = "<think>First thought</think>Hello <think>Second thought</think>world!";
        let (thinking, clean) = extract_think_tags(multi);
        assert_eq!(thinking.as_deref(), Some("First thought\n\nSecond thought"));
        assert_eq!(clean, "Hello world!");

        let unclosed = "Answer prefix <think>Trailing truncated thought";
        let (thinking2, clean2) = extract_think_tags(unclosed);
        assert_eq!(thinking2.as_deref(), Some("Trailing truncated thought"));
        assert_eq!(clean2, "Answer prefix");
    }

    #[test]
    fn test_native_tool_call_and_reasoning_replay_serialization() {
        assert_eq!(model_context_window_tokens("gemini-3.8-flash"), 1_000_000);
        assert_eq!(model_context_window_tokens("kimi-k3"), 200_000);
        assert_eq!(model_context_window_tokens("deepseek-v4.1-flash:cloud"), 1_000_000);
        assert_eq!(model_context_window_tokens("deepseek-v3:cloud"), 128_000);
        assert_eq!(model_context_window_tokens("qwen2.5-coder:7b"), 32_768);

        let tc = ToolCallRecord {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({ "path": "src/main.rs" }),
            result: "1: fn main() {}".to_string(),
            status: "ok".to_string(),
            duration_ms: 5,
        };
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "Inspect main.rs".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:00".to_string(),
                model: None,
                tokens: 10,
                input_tokens: 10,
                output_tokens: 0,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: String::new(),
                thinking: Some("I should read src/main.rs first.".to_string()),
                tool_calls: vec![tc.clone()],
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:01".to_string(),
                model: Some("kimi-k3".to_string()),
                tokens: 20,
                input_tokens: 10,
                output_tokens: 10,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: "[Tool `read_file` result]:\n1: fn main() {}".to_string(),
                thinking: None,
                tool_calls: vec![tc.clone()],
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:02".to_string(),
                model: None,
                tokens: 0,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            },
        ];

        let openai_msgs = serialize_openai_messages("sys", &history);
        assert_eq!(openai_msgs.len(), 4);
        assert_eq!(openai_msgs[2]["role"], "assistant");
        assert_eq!(
            openai_msgs[2]["reasoning_content"],
            "I should read src/main.rs first."
        );
        assert_eq!(openai_msgs[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(openai_msgs[3]["role"], "tool");
        assert_eq!(openai_msgs[3]["tool_call_id"], "call_1");

        let ollama_msgs = serialize_ollama_messages("sys", &history);
        assert_eq!(ollama_msgs.len(), 4);
        assert_eq!(ollama_msgs[2]["role"], "assistant");
        assert_eq!(
            ollama_msgs[2]["thinking"],
            "I should read src/main.rs first."
        );
        assert_eq!(ollama_msgs[3]["role"], "tool");
        assert_eq!(ollama_msgs[3]["name"], "read_file");

        // Verify multi-turn self-contained assistant messages preserve historical tool calls and results
        let multi_turn_history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "First turn prompt".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:00".to_string(),
                model: None,
                tokens: 10,
                input_tokens: 10,
                output_tokens: 0,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "Hey, I inspected the code.".to_string(),
                thinking: None,
                tool_calls: vec![tc.clone()],
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:01".to_string(),
                model: Some("deepseek-v4.1-flash:cloud".to_string()),
                tokens: 20,
                input_tokens: 10,
                output_tokens: 10,
                cost_usd: 0.0,
            },
            ChatMessage {
                role: "user".to_string(),
                content: "generate shot 4".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                images: Vec::new(),
                timestamp: "12:00:02".to_string(),
                model: None,
                tokens: 10,
                input_tokens: 10,
                output_tokens: 0,
                cost_usd: 0.0,
            },
        ];

        let ollama_multi = serialize_ollama_messages("sys", &multi_turn_history);
        assert_eq!(ollama_multi.len(), 5);
        assert_eq!(ollama_multi[2]["role"], "assistant");
        assert_eq!(
            ollama_multi[2]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(ollama_multi[3]["role"], "tool");
        assert_eq!(ollama_multi[3]["content"], "1: fn main() {}");
        assert_eq!(ollama_multi[4]["role"], "user");
        assert_eq!(ollama_multi[4]["content"], "generate shot 4");

        let openai_multi = serialize_openai_messages("sys", &multi_turn_history);
        assert_eq!(openai_multi.len(), 5);
        assert_eq!(openai_multi[2]["role"], "assistant");
        assert_eq!(openai_multi[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(openai_multi[3]["role"], "tool");
        assert_eq!(openai_multi[3]["tool_call_id"], "call_1");
        assert_eq!(openai_multi[3]["content"], "1: fn main() {}");
        assert_eq!(openai_multi[4]["role"], "user");
        assert_eq!(openai_multi[4]["content"], "generate shot 4");
    }
    #[test]
    fn test_filter_active_vision_images_prunes_older_payloads() {
        let mut history = Vec::new();
        // Create 5 user messages each with an image payload
        for i in 1..=5 {
            history.push(ChatMessage {
                role: "user".to_string(),
                content: format!("Step {} inspection", i),
                thinking: None,
                tool_calls: Vec::new(),
                diffs: Vec::new(),
                images: vec![format!("base64_data_for_image_{}", i)],
                timestamp: format!("12:00:0{}", i),
                model: None,
                tokens: 10,
                input_tokens: 10,
                output_tokens: 0,
                cost_usd: 0.0,
            });
        }

        let filtered = filter_active_vision_images(&history, 2);
        assert_eq!(filtered.len(), 5);

        // Steps 1, 2, 3 should have empty images and pruned content note
        assert!(filtered[0].images.is_empty());
        assert!(filtered[0].content.contains("pruned"));
        assert!(filtered[1].images.is_empty());
        assert!(filtered[2].images.is_empty());

        // Steps 4 and 5 (the 2 most recent) should retain their image payloads
        assert_eq!(filtered[3].images, vec!["base64_data_for_image_4".to_string()]);
        assert_eq!(filtered[4].images, vec!["base64_data_for_image_5".to_string()]);

        // When serialized for Ollama, only steps 4 and 5 should have "images" in JSON
        let ollama_json = serialize_ollama_messages("sys", &history);
        // ollama_json[0] is sys, [1..=3] are steps 1..=3, [4..=5] are steps 4..=5
        assert!(ollama_json[1].get("images").is_none());
        assert!(ollama_json[2].get("images").is_none());
        assert!(ollama_json[3].get("images").is_none());
        assert!(ollama_json[4].get("images").is_some());
        assert!(ollama_json[5].get("images").is_some());
    }
}
