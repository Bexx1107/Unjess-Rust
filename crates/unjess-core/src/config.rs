//! Configuration, API key management, automatic migration from `~/.unjess`,
//! and live + static November 2026 AI model discovery.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// User-defined OpenAI-compatible custom AI provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomProvider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub context_size: usize,
}

/// Metadata describing an available local or cloud LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub desc: String,
    pub has_tools: bool,
    pub has_thinking: bool,
    pub supports_effort: bool,
    pub is_cloud: bool,
}

/// Complete runtime configuration for Unjess 4.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub api_keys: HashMap<String, String>,
    pub ollama_base_url: String,
    pub ollama_api_base_url: String,
    pub llamacpp_base_url: String,
    pub lmstudio_base_url: String,
    pub kimi_base_url: String,
    pub qwen_base_url: String,
    pub mashina_url: String,
    pub mashina_collab_token: String,
    pub workspace: String,
    pub max_iterations: usize,
    pub subagent_max_turns: usize,
    pub subagent_model: String,
    pub ask_before_subagent_spawn: bool,
    pub confirm_commands: bool,
    pub command_timeout: u64,
    pub llm_timeout: f64,
    pub reasoning_effort: String,
    pub planning_mode: String,
    pub enable_compaction: bool,
    pub accent_color: String,
    pub auto_update_enabled: bool,
    pub update_server_url: String,
    pub max_cost_per_session: f64,
    pub fallback_models: Vec<String>,
    pub custom_providers: Vec<CustomProvider>,
}

impl Default for Settings {
    fn default() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
            provider: "ollama".to_string(),
            model: "qwen2.5-coder:7b".to_string(),
            api_keys: HashMap::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_api_base_url: "https://ollama.com".to_string(),
            llamacpp_base_url: "http://localhost:8080".to_string(),
            lmstudio_base_url: "http://localhost:1234".to_string(),
            kimi_base_url: "https://api.moonshot.ai/v1".to_string(),
            qwen_base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1".to_string(),
            mashina_url: String::new(),
            mashina_collab_token: String::new(),
            workspace: cwd,
            max_iterations: 200,
            subagent_max_turns: 300,
            subagent_model: "inherit".to_string(),
            ask_before_subagent_spawn: false,
            confirm_commands: false,
            command_timeout: 1200,
            llm_timeout: 300.0,
            reasoning_effort: "high".to_string(),
            planning_mode: "auto".to_string(),
            enable_compaction: true,
            accent_color: "#10b981".to_string(),
            auto_update_enabled: true,
            update_server_url: "https://github.com/Bexx1107/Unjess-Rust".to_string(),
            custom_providers: Vec::new(),
            max_cost_per_session: 0.0,
            fallback_models: Vec::new(),
        }
    }
}

impl Settings {
    /// Returns the configuration directory `~/.unjess-rust` (or `UNJESS_RUST_CONFIG_DIR` if set).
    pub fn config_dir() -> PathBuf {
        if let Ok(env_dir) = std::env::var("UNJESS_RUST_CONFIG_DIR") {
            let trimmed = env_dir.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".unjess-rust")
    }

    /// Returns the legacy Go configuration directory `~/.unjess` for seamless key/config import.
    pub fn legacy_go_config_dir() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".unjess")
    }

    /// Loads `Settings` from `~/.unjess-rust/config.yaml` and `~/.unjess-rust/keys.yaml`.
    /// On first run (or if keys/config are missing in `~/.unjess-rust`), automatically imports
    /// settings, base URLs, custom providers, and API keys from `~/.unjess`.
    pub fn load() -> Self {
        let mut settings = Self::default();
        let cfg_dir = Self::config_dir();
        let _ = fs::create_dir_all(&cfg_dir);

        let config_path = cfg_dir.join("config.yaml");
        let keys_path = cfg_dir.join("keys.yaml");
        let custom_prov_path = cfg_dir.join("custom_providers.json");

        let legacy_dir = Self::legacy_go_config_dir();
        let legacy_config_path = legacy_dir.join("config.yaml");
        let legacy_keys_path = legacy_dir.join("keys.yaml");
        let legacy_custom_prov_path = legacy_dir.join("custom_providers.json");

        // 1. Load config.yaml (or import from ~/.unjess/config.yaml if ~/.unjess-rust/config.yaml doesn't exist)
        if config_path.exists() {
            if let Ok(raw) = fs::read_to_string(&config_path) {
                if let Ok(parsed) = serde_yaml::from_str::<Settings>(&raw) {
                    settings = parsed;
                }
            }
        } else if legacy_config_path.exists() {
            if let Ok(raw) = fs::read_to_string(&legacy_config_path) {
                if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&raw) {
                    Self::merge_yaml_value(&mut settings, &val);
                }
            }
        }

        // 2. Load keys.yaml from ~/.unjess-rust/keys.yaml AND merge any missing keys from ~/.unjess/keys.yaml
        if legacy_keys_path.exists() {
            if let Ok(raw) = fs::read_to_string(&legacy_keys_path) {
                Self::parse_keys_yaml(&raw, &mut settings.api_keys);
            }
        }
        if keys_path.exists() {
            if let Ok(raw) = fs::read_to_string(&keys_path) {
                Self::parse_keys_yaml(&raw, &mut settings.api_keys);
            }
        }

        // 3. Load custom_providers.json
        if custom_prov_path.exists() {
            if let Ok(raw) = fs::read_to_string(&custom_prov_path) {
                if let Ok(list) = serde_json::from_str::<Vec<CustomProvider>>(&raw) {
                    settings.custom_providers = list;
                }
            }
        } else if settings.custom_providers.is_empty() && legacy_custom_prov_path.exists() {
            if let Ok(raw) = fs::read_to_string(&legacy_custom_prov_path) {
                if let Ok(list) = serde_json::from_str::<Vec<CustomProvider>>(&raw) {
                    settings.custom_providers = list;
                }
            }
        }

        // 4. Overlay environment variable keys
        Self::load_env_keys(&mut settings);

        if settings.workspace.trim().is_empty() {
            settings.workspace = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
        }
        if settings.provider.trim().is_empty() {
            settings.provider = "ollama".to_string();
        }
        if settings.model.trim().is_empty() {
            settings.model = "qwen2.5-coder:7b".to_string();
        }
        if settings.update_server_url.trim().is_empty()
            || settings.update_server_url.contains("Bexx1107/unjess")
        {
            settings.update_server_url = "https://github.com/Bexx1107/Unjess-Rust".to_string();
        }
        let prev_provider = settings.provider.clone();
        settings.sync_provider_with_model();

        // Persist initial import or repair provider mismatch on disk
        if !config_path.exists() || settings.provider != prev_provider {
            let _ = settings.save();
        }

        settings
    }

    /// Synchronizes `self.provider` with `self.model` so that a model like `gemini-3.8-flash`
    /// never carries a mismatched provider like `ollama` (and vice versa).
    pub fn sync_provider_with_model(&mut self) {
        if self.model.trim().is_empty() {
            self.model = "qwen2.5-coder:7b".to_string();
        }
        let resolved = crate::llm::resolve_provider_for_model(self, &self.model);
        if !resolved.is_empty() {
            if !(resolved == "ollama" && self.provider.trim().eq_ignore_ascii_case("ollama-api")) {
                self.provider = resolved;
            }
        }
    }

    fn merge_yaml_value(settings: &mut Settings, val: &serde_yaml::Value) {
        let Some(map) = val.as_mapping() else {
            return;
        };
        let get_str = |k: &str| -> Option<String> {
            map.get(serde_yaml::Value::String(k.to_string()))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        if let Some(v) = get_str("provider") {
            settings.provider = v;
        }
        if let Some(v) = get_str("model") {
            settings.model = v;
        }
        if let Some(v) = get_str("ollama_base_url") {
            settings.ollama_base_url = v;
        }
        if let Some(v) = get_str("ollama_api_base_url") {
            settings.ollama_api_base_url = v;
        }
        if let Some(v) = get_str("llamacpp_base_url") {
            settings.llamacpp_base_url = v;
        }
        if let Some(v) = get_str("lmstudio_base_url") {
            settings.lmstudio_base_url = v;
        }
        if let Some(v) = get_str("kimi_base_url") {
            settings.kimi_base_url = v;
        }
        if let Some(v) = get_str("qwen_base_url") {
            settings.qwen_base_url = v;
        }
        if let Some(v) = get_str("mashina_url") {
            settings.mashina_url = v;
        }
        if let Some(v) = get_str("mashina_collab_token") {
            settings.mashina_collab_token = v;
        }
        if let Some(v) = get_str("reasoning_effort") {
            settings.reasoning_effort = v;
        }
        if let Some(v) = get_str("planning_mode") {
            settings.planning_mode = v;
        }
        if let Some(v) = get_str("subagent_model") {
            settings.subagent_model = v;
        }
        if let Some(v) = get_str("workspace") {
            settings.workspace = v;
        }
        if let Some(iters) = map
            .get(serde_yaml::Value::String("max_iterations".to_string()))
            .and_then(|v| v.as_u64())
        {
            settings.max_iterations = iters as usize;
        }
        if let Some(t) = map
            .get(serde_yaml::Value::String("command_timeout".to_string()))
            .and_then(|v| v.as_u64())
        {
            settings.command_timeout = t;
        }
        if let Some(t) = map
            .get(serde_yaml::Value::String("llm_timeout".to_string()))
            .and_then(|v| v.as_f64())
        {
            settings.llm_timeout = t;
        }
        if let Some(c) = map
            .get(serde_yaml::Value::String("enable_compaction".to_string()))
            .and_then(|v| v.as_bool())
        {
            settings.enable_compaction = c;
        }
        if let Some(cost) = map
            .get(serde_yaml::Value::String("max_cost_per_session".to_string()))
            .and_then(|v| v.as_f64())
        {
            settings.max_cost_per_session = cost;
        }
        if let Some(seq) = map
            .get(serde_yaml::Value::String("fallback_models".to_string()))
            .and_then(|v| v.as_sequence())
        {
            settings.fallback_models = seq
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    fn parse_keys_yaml(raw: &str, out: &mut HashMap<String, String>) {
        if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(raw) {
            if let Some(map) = val.as_mapping() {
                for (k, v) in map {
                    if let (Some(ks), Some(vs)) = (k.as_str(), v.as_str()) {
                        if ks != "key_pool" && !vs.trim().is_empty() {
                            out.insert(ks.trim().to_string(), vs.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    fn load_env_keys(s: &mut Settings) {
        let env_map = [
            ("OPENAI_API_KEY", "openai"),
            ("ANTHROPIC_API_KEY", "anthropic"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_API_KEY", "google"),
            ("GROQ_API_KEY", "groq"),
            ("MISTRAL_API_KEY", "mistral"),
            ("XAI_API_KEY", "xai"),
            ("OPENROUTER_API_KEY", "openrouter"),
            ("CEREBRAS_API_KEY", "cerebras"),
            ("MOONSHOT_API_KEY", "kimi"),
            ("KIMI_API_KEY", "kimi"),
            ("DASHSCOPE_API_KEY", "qwen"),
            ("QWEN_API_KEY", "qwen"),
            ("OLLAMA_API_KEY", "ollama-api"),
            ("MASHINA_COLLAB_TOKEN", "mashina"),
            ("MASHINA_TOKEN", "mashina"),
        ];
        for (env_var, provider) in env_map {
            if let Ok(val) = std::env::var(env_var) {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    s.api_keys
                        .entry(provider.to_string())
                        .or_insert_with(|| trimmed.to_string());
                }
            }
        }
        if s.mashina_collab_token.is_empty() {
            if let Some(tok) = s.api_keys.get("mashina") {
                s.mashina_collab_token = tok.clone();
            }
        }
    }

    fn write_atomic(path: &std::path::Path, contents: &str) -> Result<(), String> {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, contents).is_ok() {
            if fs::rename(&tmp, path).is_ok() {
                return Ok(());
            }
            let _ = fs::remove_file(&tmp);
        }
        fs::write(path, contents).map_err(|e| e.to_string())
    }

    /// Saves `Settings` to `~/.unjess-rust/config.yaml`, `~/.unjess-rust/keys.yaml`,
    /// and `~/.unjess-rust/custom_providers.json` using atomic temp-file-and-rename writes.
    pub fn save(&self) -> Result<(), String> {
        let cfg_dir = Self::config_dir();
        fs::create_dir_all(&cfg_dir).map_err(|e| e.to_string())?;

        let config_path = cfg_dir.join("config.yaml");
        let keys_path = cfg_dir.join("keys.yaml");
        let custom_prov_path = cfg_dir.join("custom_providers.json");

        let yaml_str = serde_yaml::to_string(self).map_err(|e| e.to_string())?;
        Self::write_atomic(&config_path, &yaml_str)?;

        let mut clean_keys = HashMap::new();
        for (k, v) in &self.api_keys {
            if !v.trim().is_empty() {
                clean_keys.insert(k.clone(), v.trim().to_string());
            }
        }
        if !self.mashina_collab_token.trim().is_empty() {
            clean_keys.insert(
                "mashina".to_string(),
                self.mashina_collab_token.trim().to_string(),
            );
        }
        let keys_yaml = serde_yaml::to_string(&clean_keys).map_err(|e| e.to_string())?;
        Self::write_atomic(&keys_path, &keys_yaml)?;

        if let Ok(cp_json) = serde_json::to_string_pretty(&self.custom_providers) {
            let _ = Self::write_atomic(&custom_prov_path, &cp_json);
        }

        Ok(())
    }

    /// Look up an API key for a provider (handling aliases like `gemini` -> `google`).
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        let p = provider.to_lowercase_trimmed();
        let candidates: &[&str] = match p.as_str() {
            "google" | "gemini" => &["google", "gemini"],
            "anthropic" | "claude" => &["anthropic", "claude"],
            "openai" => &["openai"],
            "groq" => &["groq"],
            "mistral" => &["mistral"],
            "xai" | "grok" => &["xai", "grok"],
            "openrouter" => &["openrouter"],
            "cerebras" => &["cerebras"],
            "kimi" | "moonshot" => &["kimi", "moonshot"],
            "qwen" | "dashscope" => &["qwen", "dashscope"],
            "ollama-api" | "ollama" | "ollama cloud" => &["ollama-api", "ollama"],
            other => {
                if let Some(v) = self.api_keys.get(other) {
                    if !v.trim().is_empty() {
                        return Some(v.trim().to_string());
                    }
                }
                for cp in &self.custom_providers {
                    if cp.id.eq_ignore_ascii_case(other) && !cp.api_key.trim().is_empty() {
                        return Some(cp.api_key.trim().to_string());
                    }
                }
                return None;
            }
        };
        for c in candidates {
            if let Some(v) = self.api_keys.get(*c) {
                if !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    /// Probes local endpoints (Ollama `/api/tags`, llama.cpp `/v1/models`, LM Studio `/v1/models`)
    /// with a short timeout and returns discovered local models plus the complete November 2026
    /// cloud model catalog and any user-configured custom providers.
    pub fn discover_models(&self) -> Vec<ModelInfo> {
        let mut models = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Only probe localhost HTTP endpoints if not running inside cargo test
        let skip_network = std::env::var("UNJESS_SKIP_LOCAL_PROBE").is_ok();
        let mut ollama_probed_any = false;
        if !skip_network {
            let ollama_base = self.ollama_base_url.trim_end_matches('/').to_string();
            let llama_base = self
                .llamacpp_base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_string();
            let lms_base = self
                .lmstudio_base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_string();

            // Run blocking HTTP probes in parallel inside a standalone OS thread so it is 100% safe
            // even when called from inside an active Tokio runtime (`rt.enter()`).
            if let Ok(probed) = std::thread::spawn(move || {
                std::thread::scope(|s| {
                    let h_ollama = s.spawn(|| {
                        let mut out = Vec::new();
                        if !ollama_base.is_empty() {
                            if let Ok(client) = reqwest::blocking::Client::builder()
                                .timeout(Duration::from_millis(2000))
                                .build()
                            {
                                // Prefer 127.0.0.1 over localhost on Windows to avoid 300ms IPv6 ::1 SYN timeout
                                let primary_base = ollama_base.replace("http://localhost:", "http://127.0.0.1:");
                                let primary_url = format!("{}/api/tags", primary_base);
                                let fallback_url = format!("{}/api/tags", ollama_base);
                                let resp_opt = client
                                    .get(&primary_url)
                                    .send()
                                    .ok()
                                    .filter(|r| r.status().is_success())
                                    .or_else(|| {
                                        if primary_url != fallback_url {
                                            client
                                                .get(&fallback_url)
                                                .send()
                                                .ok()
                                                .filter(|r| r.status().is_success())
                                        } else {
                                            None
                                        }
                                    });

                                if let Some(resp) = resp_opt {
                                    if let Ok(json) = resp.json::<serde_json::Value>() {
                                        if let Some(arr) =
                                            json.get("models").and_then(|v| v.as_array())
                                        {
                                            for item in arr {
                                                let id = item
                                                    .get("name")
                                                    .or_else(|| item.get("model"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                if id.is_empty() {
                                                    continue;
                                                }
                                                let lower = id.to_lowercase();
                                                let remote_host = item
                                                    .get("remote_host")
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("");
                                                let is_cloud =
                                                    lower.contains(":cloud") || !remote_host.is_empty();
                                                let cap_thinking = item
                                                    .get("capabilities")
                                                    .and_then(|v| v.as_array())
                                                    .map(|caps| {
                                                        caps.iter().any(|c| {
                                                            c.as_str() == Some("thinking")
                                                        })
                                                    })
                                                    .unwrap_or(false);
                                                let has_thinking = cap_thinking
                                                    || lower.contains("deepseek")
                                                    || lower.contains("r1")
                                                    || lower.contains("qwq")
                                                    || lower.contains("kimi")
                                                    || lower.contains("glm")
                                                    || lower.contains("qwen3")
                                                    || is_cloud;
                                                let param_size = item
                                                    .get("details")
                                                    .and_then(|d| d.get("parameter_size"))
                                                    .and_then(|v| v.as_str())
                                                    .unwrap_or("");
                                                let desc = if is_cloud {
                                                    if param_size.is_empty() {
                                                        "Ollama Cloud model".to_string()
                                                    } else {
                                                        format!("Ollama Cloud model ({})", param_size)
                                                    }
                                                } else if param_size.is_empty() {
                                                    "Local Ollama model".to_string()
                                                } else {
                                                    format!("Local Ollama model ({})", param_size)
                                                };
                                                out.push(ModelInfo {
                                                    name: id.clone(),
                                                    id,
                                                    provider: if is_cloud {
                                                        "Ollama Cloud".to_string()
                                                    } else {
                                                        "Ollama".to_string()
                                                    },
                                                    desc,
                                                    has_tools: true,
                                                    has_thinking,
                                                    supports_effort: has_thinking,
                                                    is_cloud,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        out
                    });

                    let h_llama = s.spawn(|| {
                        let mut out = Vec::new();
                        if !llama_base.is_empty() {
                            if let Ok(client) = reqwest::blocking::Client::builder()
                                .timeout(Duration::from_millis(800))
                                .build()
                            {
                                let primary = llama_base.replace("http://localhost:", "http://127.0.0.1:");
                                let url = format!("{}/v1/models", primary);
                                if let Ok(resp) = client.get(&url).send() {
                                    if resp.status().is_success() {
                                        if let Ok(json) = resp.json::<serde_json::Value>() {
                                            if let Some(arr) =
                                                json.get("data").and_then(|v| v.as_array())
                                            {
                                                for item in arr {
                                                    if let Some(id) =
                                                        item.get("id").and_then(|v| v.as_str())
                                                    {
                                                        if !id.is_empty() {
                                                            out.push(ModelInfo {
                                                                id: id.to_string(),
                                                                name: format!("{} (llama.cpp)", id),
                                                                provider: "llama.cpp".to_string(),
                                                                desc:
                                                                    "Local llama-server GGUF model"
                                                                        .to_string(),
                                                                has_tools: true,
                                                                has_thinking: true,
                                                                supports_effort: false,
                                                                is_cloud: false,
                                                            });
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        out
                    });

                    let h_lms = s.spawn(|| {
                        let mut out = Vec::new();
                        if !lms_base.is_empty() {
                            if let Ok(client) = reqwest::blocking::Client::builder()
                                .timeout(Duration::from_millis(800))
                                .build()
                            {
                                let primary = lms_base.replace("http://localhost:", "http://127.0.0.1:");
                                let url = format!("{}/v1/models", primary);
                                if let Ok(resp) = client.get(&url).send() {
                                    if resp.status().is_success() {
                                        if let Ok(json) = resp.json::<serde_json::Value>() {
                                            if let Some(arr) =
                                                json.get("data").and_then(|v| v.as_array())
                                            {
                                                for item in arr {
                                                    if let Some(id) =
                                                        item.get("id").and_then(|v| v.as_str())
                                                    {
                                                        if !id.is_empty() {
                                                            out.push(ModelInfo {
                                                                id: id.to_string(),
                                                                name: format!("{} (LM Studio)", id),
                                                                provider: "LM Studio".to_string(),
                                                                desc: "Local LM Studio model"
                                                                    .to_string(),
                                                                has_tools: true,
                                                                has_thinking: true,
                                                                supports_effort: false,
                                                                is_cloud: false,
                                                            });
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        out
                    });

                    let mut local_models = Vec::new();
                    if let Ok(v) = h_ollama.join() {
                        local_models.extend(v);
                    }
                    if let Ok(v) = h_llama.join() {
                        local_models.extend(v);
                    }
                    if let Ok(v) = h_lms.join() {
                        local_models.extend(v);
                    }
                    local_models
                })
            })
            .join()
            {
                for m in probed {
                    if m.provider.starts_with("Ollama") {
                        ollama_probed_any = true;
                    }
                    if seen.insert(m.id.clone()) {
                        models.push(m);
                    }
                }
            }
        }

        // Ensure default local models are listed if Ollama wasn't running or returned no models
        if !ollama_probed_any {
            for (id, name, provider, desc, thinking) in [
                (
                    "qwen2.5-coder:7b",
                    "Qwen 2.5 Coder 7B (Local)",
                    "Ollama",
                    "Default local coding model via Ollama",
                    false,
                ),
                (
                    "qwen3:14b",
                    "Qwen 3 14B (Local)",
                    "Ollama",
                    "Local hybrid reasoning & coding model via Ollama",
                    true,
                ),
                (
                    "deepseek-r1:14b",
                    "DeepSeek R1 14B (Local)",
                    "Ollama",
                    "Local reasoning model via Ollama",
                    true,
                ),
            ] {
                if seen.insert(id.to_string()) {
                    models.push(ModelInfo {
                        id: id.to_string(),
                        name: name.to_string(),
                        provider: provider.to_string(),
                        desc: desc.to_string(),
                        has_tools: true,
                        has_thinking: thinking,
                        supports_effort: thinking,
                        is_cloud: false,
                    });
                }
            }
        }

        // 4. Full November 2026 Frontier Cloud Catalog
        let cloud_catalog = [
            // Google DeepMind
            (
                "gemini-4-argon",
                "Gemini 4 Argon",
                "Google",
                "Google DeepMind next-gen flagship reasoning & architecture model",
                true,
                true,
            ),
            (
                "gemini-3.8-flash",
                "Gemini 3.8 Flash",
                "Google",
                "Google DeepMind high-speed hybrid reasoning model",
                true,
                true,
            ),
            (
                "gemini-3.7-flash",
                "Gemini 3.7 Flash",
                "Google",
                "Google DeepMind multimodal coding & reasoning model",
                true,
                true,
            ),
            (
                "gemini-3.6-flash",
                "Gemini 3.6 Flash",
                "Google",
                "Google DeepMind fast agentic coding model",
                true,
                true,
            ),
            (
                "gemini-3.5-flash",
                "Gemini 3.5 Flash",
                "Google",
                "Google DeepMind balanced Flash model",
                true,
                true,
            ),
            (
                "gemini-3.1-pro-preview",
                "Gemini 3.1 Pro Preview",
                "Google",
                "Google DeepMind Gemini 3.1 Pro frontier model",
                true,
                true,
            ),
            (
                "antigravity-preview-latest",
                "Antigravity Preview Latest",
                "Google",
                "Google DeepMind agentic coding preview",
                true,
                true,
            ),
            (
                "deep-research-max-preview-04-2026",
                "Deep Research Max (04-2026)",
                "Google",
                "Google DeepMind deep research & synthesis model",
                true,
                true,
            ),
            (
                "gemini-2.5-pro",
                "Gemini 2.5 Pro",
                "Google",
                "Google DeepMind Pro reasoning model",
                true,
                true,
            ),
            (
                "gemini-2.5-flash",
                "Gemini 2.5 Flash",
                "Google",
                "Google DeepMind hybrid reasoning model",
                true,
                true,
            ),
            // Anthropic
            (
                "claude-opus-5-5",
                "Claude Opus 5.5",
                "Anthropic",
                "Anthropic frontier reasoning & systems architecture flagship",
                true,
                true,
            ),
            (
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                "Anthropic",
                "Anthropic autonomous coding & agentic flagship",
                true,
                true,
            ),
            (
                "claude-haiku-5-5",
                "Claude Haiku 5.5",
                "Anthropic",
                "Anthropic ultra-fast adaptive reasoning model",
                true,
                true,
            ),
            // OpenAI
            (
                "gpt-5",
                "GPT-5",
                "OpenAI",
                "OpenAI flagship unified reasoning & coding model",
                true,
                true,
            ),
            (
                "o3",
                "o3",
                "OpenAI",
                "OpenAI deep multi-step reasoning flagship",
                true,
                true,
            ),
            (
                "o4-mini",
                "o4-mini",
                "OpenAI",
                "OpenAI fast agentic STEM & code reasoning",
                true,
                true,
            ),
            // Kimi (Moonshot AI)
            (
                "kimi-k3",
                "Kimi K3",
                "Kimi",
                "Moonshot AI flagship reasoning & coding model",
                true,
                true,
            ),
            (
                "kimi-k2.7-code",
                "Kimi K2.7 Code",
                "Kimi",
                "Moonshot AI specialized agentic coding model",
                true,
                true,
            ),
            // Alibaba Qwen / DashScope
            (
                "qwen3.8-max",
                "Qwen 3.8 Max",
                "Qwen",
                "Alibaba QwenCloud flagship reasoning model",
                true,
                true,
            ),
            (
                "deepseek-v4.1-flash",
                "DeepSeek V4.1 Flash",
                "Qwen",
                "DeepSeek V4.1 Flash high-speed reasoning",
                true,
                true,
            ),
            (
                "deepseek-v4-pro",
                "DeepSeek V4 Pro",
                "Qwen",
                "DeepSeek V4 Pro frontier reasoning",
                true,
                true,
            ),
            (
                "glm-5.3-prime",
                "GLM 5.3 Prime",
                "Qwen",
                "Zhipu GLM 5.3 Prime agentic synthesis",
                true,
                true,
            ),
            // xAI, Groq, Mistral, OpenRouter, Cerebras
            (
                "grok-4-latest",
                "Grok 4",
                "xAI",
                "xAI frontier reasoning & coding flagship",
                true,
                true,
            ),
            (
                "llama-3.3-70b-versatile",
                "Llama 3.3 70B",
                "Groq",
                "Ultra-fast LPU inference on Groq",
                false,
                false,
            ),
            (
                "codestral-latest",
                "Codestral",
                "Mistral",
                "Mistral code generation specialist",
                false,
                false,
            ),
            (
                "openrouter/auto",
                "OpenRouter Auto",
                "OpenRouter",
                "Auto-routed optimal frontier model",
                true,
                true,
            ),
            (
                "qwen-3-32b",
                "Qwen 3 32B (Cerebras)",
                "Cerebras",
                "Wafer-scale ultra-fast coding & reasoning",
                true,
                true,
            ),
        ];

        for (id, name, provider, desc, thinking, effort) in cloud_catalog {
            if seen.insert(id.to_string()) {
                models.push(ModelInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    provider: provider.to_string(),
                    desc: desc.to_string(),
                    has_tools: true,
                    has_thinking: thinking,
                    supports_effort: effort,
                    is_cloud: true,
                });
            }
        }

        // 5. Custom providers
        for cp in &self.custom_providers {
            for m in &cp.models {
                if seen.insert(m.clone()) {
                    models.push(ModelInfo {
                        id: m.clone(),
                        name: format!("{} ({})", m, cp.name),
                        provider: cp.name.clone(),
                        desc: format!("Custom provider ({})", cp.base_url),
                        has_tools: true,
                        has_thinking: true,
                        supports_effort: true,
                        is_cloud: true,
                    });
                }
            }
        }

        models
    }

    /// Probes a specific local/LAN LLM endpoint (`ollama`, `lmstudio`, or `llamacpp`)
    /// and returns the list of discovered model IDs or a descriptive connection error.
    pub fn probe_provider_endpoint(
        provider_kind: &str,
        base_url: &str,
    ) -> Result<Vec<String>, String> {
        let clean_base = base_url.trim().trim_end_matches('/').to_string();
        if clean_base.is_empty() {
            return Err("Server URL is empty".to_string());
        }
        let kind = provider_kind.trim().to_lowercase();
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|e| e.to_string())?;

            if kind == "ollama" {
                let url = format!("{}/api/tags", clean_base);
                let resp = client.get(&url).send().map_err(|e| {
                    format!(
                        "Could not connect to Ollama at {} ({})",
                        clean_base,
                        e.without_url()
                    )
                })?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "Ollama at {} returned HTTP {}",
                        clean_base,
                        resp.status()
                    ));
                }
                let json: serde_json::Value = resp
                    .json()
                    .map_err(|e| format!("Invalid JSON from {}: {}", url, e))?;
                let mut names = Vec::new();
                if let Some(arr) = json.get("models").and_then(|m| m.as_array()) {
                    for item in arr {
                        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                            names.push(name.to_string());
                        }
                    }
                }
                Ok(names)
            } else {
                let url = if clean_base.ends_with("/v1") {
                    format!("{}/models", clean_base)
                } else {
                    format!("{}/v1/models", clean_base)
                };
                let resp = client.get(&url).send().map_err(|e| {
                    format!(
                        "Could not connect to {} at {} ({})",
                        kind,
                        clean_base,
                        e.without_url()
                    )
                })?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "{} at {} returned HTTP {}",
                        kind,
                        clean_base,
                        resp.status()
                    ));
                }
                let json: serde_json::Value = resp
                    .json()
                    .map_err(|e| format!("Invalid JSON from {}: {}", url, e))?;
                let mut ids = Vec::new();
                if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                    for item in arr {
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            ids.push(id.to_string());
                        }
                    }
                }
                Ok(ids)
            }
        })
        .join()
        .unwrap_or_else(|_| Err("Endpoint probe thread panicked".to_string()))
    }
}

trait StrExt {
    fn to_lowercase_trimmed(&self) -> String;
}

impl StrExt for str {
    fn to_lowercase_trimmed(&self) -> String {
        self.trim().to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_load_save_and_models() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("UNJESS_RUST_CONFIG_DIR", temp.path());
            std::env::set_var("UNJESS_SKIP_LOCAL_PROBE", "1");
        }
        let mut s = Settings::load();
        s.model = "gemini-4-argon".to_string();
        s.provider = "google".to_string();
        s.api_keys
            .insert("google".to_string(), "test-key-123".to_string());
        s.save().unwrap();

        let reloaded = Settings::load();
        assert_eq!(reloaded.model, "gemini-4-argon");
        assert_eq!(
            reloaded.get_api_key("google").as_deref(),
            Some("test-key-123")
        );

        let catalog = reloaded.discover_models();
        assert!(catalog.iter().any(|m| m.id == "gemini-4-argon"));
        assert!(catalog.iter().any(|m| m.id == "claude-opus-5-5"));
        assert!(catalog.iter().any(|m| m.id == "gpt-5"));
    }
}
