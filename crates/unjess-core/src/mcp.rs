//! Model Context Protocol (MCP) Server Registry, JSON-RPC 2.0 Stdio/HTTP Prober,
//! and Tool Executor (`~/.unjess-rust/mcp.json`).
//! Supports MCP `2025-06-18` and `2024-11-05` specifications (Stdio & Streamable HTTP/SSE).

use crate::config::Settings;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// Configuration for an MCP server (stdio command or HTTP/SSE URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub cached_tools: Vec<McpToolInfo>,
    #[serde(default)]
    pub last_probe_ok: Option<bool>,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_enabled() -> bool {
    true
}

/// Metadata for a single tool exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_input_schema")]
    pub input_schema: serde_json::Value,
}

fn default_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

/// Result of probing an MCP server via JSON-RPC 2.0 `initialize` + `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpProbeResult {
    pub name: String,
    pub ok: bool,
    pub transport: String,
    pub latency_ms: u64,
    pub server_info: String,
    pub tools: Vec<McpToolInfo>,
    pub error: Option<String>,
}

fn kill_child_tree(child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let pid = child.id();
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn expand_workspace_vars(raw: &str) -> String {
    if !raw.contains("${workspaceRoot}") && !raw.contains("${workspace}") {
        return raw.to_string();
    }
    let ws = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string();
    raw.replace("${workspaceRoot}", &ws)
        .replace("${workspace}", &ws)
}

fn parse_jsonrpc_or_sse(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    let mut last_json = None;
    for line in trimmed.lines() {
        let l = line.trim();
        if let Some(data_str) = l.strip_prefix("data:") {
            let d = data_str.trim();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(d) {
                last_json = Some(v);
            }
        } else if l.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
                last_json = Some(v);
            }
        }
    }
    last_json
}

fn resolve_mashina_token(env: &HashMap<String, String>) -> String {
    if let Some(t) = env
        .get("MASHINA_COLLAB_TOKEN")
        .or_else(|| env.get("MASHINA_TOKEN"))
    {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    if let Ok(t) = std::env::var("MASHINA_COLLAB_TOKEN") {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    let settings = Settings::load();
    if !settings.mashina_collab_token.trim().is_empty() {
        return settings.mashina_collab_token.trim().to_string();
    }
    if let Some(t) = settings.api_keys.get("mashina") {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    String::new()
}

pub struct McpManager;

impl McpManager {
    fn mcp_path() -> PathBuf {
        Settings::config_dir().join("mcp.json")
    }

    fn normalize_and_migrate(mut list: Vec<McpServerConfig>) -> (Vec<McpServerConfig>, bool) {
        let mut changed = false;
        for srv in &mut list {
            // Migrate legacy non-existent npm package `reverse-engineer-anything-mcp` -> `rea-agents@latest mcp`
            if srv
                .args
                .iter()
                .any(|a| a == "reverse-engineer-anything-mcp")
            {
                srv.args = vec![
                    "-y".to_string(),
                    "rea-agents@latest".to_string(),
                    "mcp".to_string(),
                ];
                changed = true;
            }
            if srv.name == "mashina-collab" && srv.url == "https://mashina.si/api/mcp" {
                srv.url = "https://mashina.si/api/collab/mcp".to_string();
                changed = true;
            }
            // If URL was placed into `command` field by mistake, move it to `url`
            if srv.url.trim().is_empty()
                && (srv.command.trim().starts_with("http://")
                    || srv.command.trim().starts_with("https://"))
            {
                srv.url = srv.command.trim().to_string();
                srv.command.clear();
                changed = true;
            }
        }
        (list, changed)
    }

    fn parse_mcp_json(raw: &str) -> Option<Vec<McpServerConfig>> {
        if let Ok(list) = serde_json::from_str::<Vec<McpServerConfig>>(raw) {
            return Some(list);
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(arr) = val.get("servers").and_then(|v| v.as_array()) {
                if let Ok(list) =
                    serde_json::from_value::<Vec<McpServerConfig>>(serde_json::Value::Array(
                        arr.clone(),
                    ))
                {
                    return Some(list);
                }
            }
            if let Some(obj) = val.get("mcpServers").and_then(|v| v.as_object()) {
                let mut list = Vec::new();
                for (name, cfg_val) in obj {
                    let command = cfg_val
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let url = cfg_val
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = cfg_val
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut env = HashMap::new();
                    if let Some(eobj) = cfg_val.get("env").and_then(|v| v.as_object()) {
                        for (ek, ev) in eobj {
                            if let Some(evs) = ev.as_str() {
                                env.insert(ek.clone(), evs.to_string());
                            }
                        }
                    }
                    let enabled = cfg_val
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    list.push(McpServerConfig {
                        name: name.clone(),
                        command,
                        url,
                        args,
                        env,
                        cwd: cfg_val
                            .get("cwd")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        headers: HashMap::new(),
                        enabled,
                        description: cfg_val
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        cached_tools: Vec::new(),
                        last_probe_ok: None,
                        last_error: None,
                    });
                }
                return Some(list);
            }
        }
        None
    }

    /// Loads configured MCP servers from `~/.unjess-rust/mcp.json`, falling back to `~/.unjess/mcp.json` or seeding default presets.
    pub fn load() -> Vec<McpServerConfig> {
        let path = Self::mcp_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Some(list) = Self::parse_mcp_json(&raw) {
                let (migrated, changed) = Self::normalize_and_migrate(list);
                if changed {
                    let _ = Self::save(&migrated);
                }
                return migrated;
            }
        }
        let legacy_path = Settings::legacy_go_config_dir().join("mcp.json");
        if let Ok(raw) = fs::read_to_string(&legacy_path) {
            if let Some(list) = Self::parse_mcp_json(&raw) {
                if !list.is_empty() {
                    let (migrated, _) = Self::normalize_and_migrate(list);
                    let _ = Self::save(&migrated);
                    return migrated;
                }
            }
        }
        let defaults = Self::presets("");
        let _ = Self::save(&defaults);
        defaults
    }

    /// Saves MCP server configurations to `~/.unjess-rust/mcp.json`.
    pub fn save(servers: &[McpServerConfig]) -> Result<(), String> {
        let path = Self::mcp_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(servers).map_err(|e| e.to_string())?;
        let tmp_path = path.with_extension("json.tmp");
        if fs::write(&tmp_path, &json).is_ok() {
            if fs::rename(&tmp_path, &path).is_err() {
                fs::write(&path, &json).map_err(|e| e.to_string())?;
                let _ = fs::remove_file(&tmp_path);
            }
            Ok(())
        } else {
            fs::write(&path, &json).map_err(|e| e.to_string())
        }
    }

    /// Builds a platform-safe `Command` for stdio MCP servers, wrapping `.cmd`/`.bat` shims (`npx`, `npm`, `pnpm`, `uvx`, `yarn`)
    /// via `cmd /C` on Windows with `CREATE_NO_WINDOW`, and expanding `${workspaceRoot}`.
    fn build_stdio_command(cfg: &McpServerConfig) -> Command {
        let raw_cmd = expand_workspace_vars(cfg.command.trim());
        let expanded_args: Vec<String> = cfg
            .args
            .iter()
            .map(|a| expand_workspace_vars(a))
            .collect();

        #[cfg(target_os = "windows")]
        let mut cmd = {
            let lower = raw_cmd.to_lowercase();
            let needs_cmd_shim = matches!(
                lower.as_str(),
                "npx" | "npm" | "pnpm" | "yarn" | "uvx" | "bunx"
            ) || lower.ends_with(".cmd")
                || lower.ends_with(".bat");
            let mut c = if needs_cmd_shim {
                let mut c = Command::new("cmd");
                c.arg("/C").arg(&raw_cmd).args(&expanded_args);
                c
            } else {
                let mut c = Command::new(&raw_cmd);
                c.args(&expanded_args);
                c
            };
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
            c
        };
        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new(&raw_cmd);
            c.args(&expanded_args);
            c
        };

        if let Some(ref cwd) = cfg.cwd {
            let expanded_cwd = expand_workspace_vars(cwd.trim());
            if !expanded_cwd.is_empty() && std::path::Path::new(&expanded_cwd).is_dir() {
                cmd.current_dir(&expanded_cwd);
            }
        }
        cmd.envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Probes an MCP server over stdio or HTTP using JSON-RPC 2.0 `initialize` + `tools/list`,
    /// and caches discovered tools in `mcp.json`.
    pub fn probe_server(cfg: &McpServerConfig) -> McpProbeResult {
        let started = Instant::now();
        let effective_url = if !cfg.url.trim().is_empty() {
            cfg.url.trim().to_string()
        } else if cfg.command.trim().starts_with("http://")
            || cfg.command.trim().starts_with("https://")
        {
            cfg.command.trim().to_string()
        } else {
            String::new()
        };

        let res = if !effective_url.is_empty() {
            let mut http_cfg = cfg.clone();
            http_cfg.url = effective_url;
            Self::probe_http_server(&http_cfg, started)
        } else if !cfg.command.trim().is_empty() {
            Self::probe_stdio_server(cfg, started)
        } else {
            McpProbeResult {
                name: cfg.name.clone(),
                ok: false,
                transport: "none".to_string(),
                latency_ms: 0,
                server_info: String::new(),
                tools: Vec::new(),
                error: Some("Neither command nor url is configured".to_string()),
            }
        };

        // Update cached tools & status in mcp.json if server exists
        let mut servers = Self::load();
        if let Some(existing) = servers
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(&cfg.name))
        {
            existing.last_probe_ok = Some(res.ok);
            existing.last_error = res.error.clone();
            if res.ok || !res.tools.is_empty() {
                existing.cached_tools = res.tools.clone();
            }
            let _ = Self::save(&servers);
        }

        res
    }

    /// Probes all enabled MCP servers in parallel threads and updates `mcp.json`.
    pub fn probe_all_enabled() -> Vec<McpProbeResult> {
        let servers = Self::load();
        let enabled_servers: Vec<McpServerConfig> =
            servers.into_iter().filter(|s| s.enabled).collect();
        let mut handles = Vec::new();
        for srv in enabled_servers {
            handles.push(std::thread::spawn(move || Self::probe_server(&srv)));
        }
        let mut results = Vec::new();
        for h in handles {
            if let Ok(r) = h.join() {
                results.push(r);
            }
        }
        results
    }

    fn probe_http_server(cfg: &McpServerConfig, started: Instant) -> McpProbeResult {
        let url = cfg.url.trim().to_string();
        let env = cfg.env.clone();
        let headers = cfg.headers.clone();
        let name = cfg.name.clone();

        let res = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?;

            let bearer_tok = resolve_mashina_token(&env);

            // Step 1: Send MCP `initialize` handshake (captures `mcp-session-id` if server is stateful)
            let init_payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "Unjess", "version": "4.0.0" }
                }
            });
            let mut init_req = client
                .post(&url)
                .header("Accept", "application/json, text/event-stream")
                .json(&init_payload);
            if !bearer_tok.is_empty() {
                init_req = init_req.bearer_auth(&bearer_tok);
            }
            for (hk, hv) in &headers {
                if !hk.trim().is_empty() {
                    init_req = init_req.header(hk.trim(), hv.trim());
                }
            }

            let mut session_id: Option<String> = None;
            let mut server_info_name = String::new();
            if let Ok(init_resp) = init_req.send() {
                if let Some(sid) = init_resp
                    .headers()
                    .get("mcp-session-id")
                    .or_else(|| init_resp.headers().get("Mcp-Session-Id"))
                    .and_then(|v| v.to_str().ok())
                {
                    session_id = Some(sid.to_string());
                }
                if let Ok(init_body) = init_resp.text() {
                    if let Some(json) = parse_jsonrpc_or_sse(&init_body) {
                        if let Some(sname) = json
                            .get("result")
                            .and_then(|r| r.get("serverInfo"))
                            .and_then(|s| s.get("name"))
                            .and_then(|v| v.as_str())
                        {
                            server_info_name = sname.to_string();
                        }
                    }
                }
            }

            // Step 2: Send `tools/list`
            let list_payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            });
            let mut req = client
                .post(&url)
                .header("Accept", "application/json, text/event-stream")
                .json(&list_payload);
            if !bearer_tok.is_empty() {
                req = req.bearer_auth(&bearer_tok);
            }
            if let Some(ref sid) = session_id {
                req = req.header("Mcp-Session-Id", sid);
            }
            for (hk, hv) in &headers {
                if !hk.trim().is_empty() {
                    req = req.header(hk.trim(), hv.trim());
                }
            }

            match req.send() {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().unwrap_or_default();
                    let mut tools = Vec::new();
                    let mut rpc_err: Option<String> = None;
                    if let Some(json) = parse_jsonrpc_or_sse(&body) {
                        if let Some(err_obj) = json.get("error") {
                            rpc_err = err_obj
                                .get("message")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                        if let Some(arr) = json
                            .get("result")
                            .and_then(|r| r.get("tools"))
                            .and_then(|t| t.as_array())
                        {
                            for t in arr {
                                tools.push(McpToolInfo {
                                    name: t
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("tool")
                                        .to_string(),
                                    description: t
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    input_schema: t
                                        .get("inputSchema")
                                        .cloned()
                                        .unwrap_or_else(default_input_schema),
                                });
                            }
                        }
                    }
                    if !status.is_success() {
                        return Err(format!(
                            "HTTP {} {}",
                            status,
                            rpc_err.unwrap_or_default()
                        )
                        .trim()
                        .to_string());
                    }
                    if let Some(e) = rpc_err {
                        if tools.is_empty() {
                            return Err(format!("MCP RPC error: {}", e));
                        }
                    }
                    let info = if !server_info_name.is_empty() {
                        format!("{} (HTTP {})", server_info_name, status)
                    } else {
                        format!("HTTP {}", status)
                    };
                    Ok((true, info, tools))
                }
                Err(e) => Err(e.to_string()),
            }
        })
        .join()
        .unwrap_or_else(|_| Err("HTTP probe thread panicked".to_string()));

        let latency_ms = started.elapsed().as_millis() as u64;
        match res {
            Ok((ok, info, tools)) => McpProbeResult {
                name,
                ok,
                transport: "http".to_string(),
                latency_ms,
                server_info: info,
                tools,
                error: None,
            },
            Err(e) => McpProbeResult {
                name,
                ok: false,
                transport: "http".to_string(),
                latency_ms,
                server_info: String::new(),
                tools: Vec::new(),
                error: Some(e),
            },
        }
    }

    fn probe_stdio_server(cfg: &McpServerConfig, started: Instant) -> McpProbeResult {
        let mut cmd = Self::build_stdio_command(cfg);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return McpProbeResult {
                    name: cfg.name.clone(),
                    ok: false,
                    transport: "stdio".to_string(),
                    latency_ms: started.elapsed().as_millis() as u64,
                    server_info: String::new(),
                    tools: Vec::new(),
                    error: Some(format!("Failed to spawn '{}': {}", cfg.command, e)),
                };
            }
        };

        let mut tools = Vec::new();
        let mut server_info = format!("stdio: {} {}", cfg.command, cfg.args.join(" "));
        let mut received_valid_rpc = false;
        let mut rpc_error: Option<String> = None;

        // Capture stderr in background thread for clear diagnostics if the process fails
        let stderr_rx = child.stderr.take().map(|stderr| {
            let (etx, erx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                let mut lines = Vec::new();
                for l in reader.lines().take(25).flatten() {
                    if !l.trim().is_empty() {
                        lines.push(l);
                    }
                }
                let _ = etx.send(lines.join(" | "));
            });
            erx
        });

        let mut stdin_opt = child.stdin.take();
        if let Some(ref mut stdin) = stdin_opt {
            let init_msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "Unjess", "version": "4.0.0" }
                }
            });
            let initialized_notif = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            });
            let list_msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            });
            let _ = writeln!(stdin, "{}", init_msg);
            let _ = writeln!(stdin, "{}", initialized_notif);
            let _ = writeln!(stdin, "{}", list_msg);
            let _ = stdin.flush();
        }

        if let Some(stdout) = child.stdout.take() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().take(200).flatten() {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });

            // Allow up to 12 seconds for `npx -y` package resolution + MCP startup
            let deadline = Instant::now() + Duration::from_secs(12);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(line) => {
                        let trimmed = line.trim();
                        if !trimmed.starts_with('{') {
                            continue;
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                            if json.get("jsonrpc").is_some() || json.get("result").is_some() {
                                received_valid_rpc = true;
                            }
                            if let Some(err_msg) = json
                                .get("error")
                                .and_then(|e| e.get("message"))
                                .and_then(|v| v.as_str())
                            {
                                rpc_error = Some(err_msg.to_string());
                            }
                            if let Some(sinfo) = json
                                .get("result")
                                .and_then(|r| r.get("serverInfo"))
                                .and_then(|s| s.get("name"))
                                .and_then(|v| v.as_str())
                            {
                                server_info = sinfo.to_string();
                            }
                            if let Some(arr) = json
                                .get("result")
                                .and_then(|r| r.get("tools"))
                                .and_then(|t| t.as_array())
                            {
                                for t in arr {
                                    tools.push(McpToolInfo {
                                        name: t
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("tool")
                                            .to_string(),
                                        description: t
                                            .get("description")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string(),
                                        input_schema: t
                                            .get("inputSchema")
                                            .cloned()
                                            .unwrap_or_else(default_input_schema),
                                    });
                                }
                                break;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Ok(Some(_status)) = child.try_wait() {
                            // Process already exited; drain any remaining queued stdout lines then stop
                            while let Ok(line) = rx.try_recv() {
                                if let Ok(json) =
                                    serde_json::from_str::<serde_json::Value>(line.trim())
                                {
                                    if json.get("result").is_some() {
                                        received_valid_rpc = true;
                                    }
                                }
                            }
                            break;
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }

        drop(stdin_opt);
        kill_child_tree(&mut child);

        let stderr_summary = stderr_rx
            .and_then(|erx| erx.recv_timeout(Duration::from_millis(200)).ok())
            .unwrap_or_default();

        McpProbeResult {
            name: cfg.name.clone(),
            ok: received_valid_rpc,
            transport: "stdio".to_string(),
            latency_ms: started.elapsed().as_millis() as u64,
            server_info,
            tools,
            error: if received_valid_rpc {
                None
            } else if let Some(re) = rpc_error {
                Some(re)
            } else if !stderr_summary.is_empty() {
                Some(format!("Stdio server failed: {}", stderr_summary))
            } else {
                Some("No JSON-RPC 2.0 response received from stdio server".to_string())
            },
        }
    }

    fn extract_mcp_result_payload(raw_json_str: &str) -> Result<String, String> {
        let Some(json) = parse_jsonrpc_or_sse(raw_json_str) else {
            return Ok(raw_json_str.to_string());
        };
        if let Some(err_obj) = json.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| err_obj.as_str().unwrap_or("MCP JSON-RPC error"));
            return Err(format!("MCP RPC error: {}", msg));
        }
        if let Some(res) = json.get("result") {
            let is_err = res
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mut extracted = None;
            if let Some(content_arr) = res.get("content").and_then(|c| c.as_array()) {
                let mut texts = Vec::new();
                for item in content_arr {
                    if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                        texts.push(t.to_string());
                    } else if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                        let mime = item
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/png");
                        texts.push(format!("[MCP Image Attachment ({})]", mime));
                    }
                }
                if !texts.is_empty() {
                    extracted = Some(texts.join("\n"));
                }
            }
            let payload_str = extracted.unwrap_or_else(|| {
                serde_json::to_string_pretty(res).unwrap_or_else(|_| res.to_string())
            });
            if is_err {
                return Err(format!("MCP tool error: {}", payload_str));
            }
            return Ok(payload_str);
        }
        Ok(raw_json_str.to_string())
    }

    /// Invokes a tool on a configured MCP server over JSON-RPC 2.0 (Stdio or Streamable HTTP).
    pub fn call_tool(
        server_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let servers = Self::load();
        let cfg = servers
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(server_name.trim()))
            .ok_or_else(|| format!("MCP server '{}' not found", server_name))?;

        let effective_url = if !cfg.url.trim().is_empty() {
            cfg.url.trim().to_string()
        } else if cfg.command.trim().starts_with("http://")
            || cfg.command.trim().starts_with("https://")
        {
            cfg.command.trim().to_string()
        } else {
            String::new()
        };

        if !effective_url.is_empty() {
            let url = effective_url;
            let env = cfg.env.clone();
            let headers = cfg.headers.clone();
            let tname = tool_name.to_string();
            let args = arguments.clone();
            let raw_resp = std::thread::spawn(move || {
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(|e| e.to_string())?;
                let bearer_tok = resolve_mashina_token(&env);

                // Initialize session if needed
                let init_payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": { "name": "Unjess", "version": "4.0.0" }
                    }
                });
                let mut init_req = client
                    .post(&url)
                    .header("Accept", "application/json, text/event-stream")
                    .json(&init_payload);
                if !bearer_tok.is_empty() {
                    init_req = init_req.bearer_auth(&bearer_tok);
                }
                for (hk, hv) in &headers {
                    if !hk.trim().is_empty() {
                        init_req = init_req.header(hk.trim(), hv.trim());
                    }
                }
                let mut session_id: Option<String> = None;
                if let Ok(init_resp) = init_req.send() {
                    if let Some(sid) = init_resp
                        .headers()
                        .get("mcp-session-id")
                        .or_else(|| init_resp.headers().get("Mcp-Session-Id"))
                        .and_then(|v| v.to_str().ok())
                    {
                        session_id = Some(sid.to_string());
                    }
                }

                let payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": { "name": tname, "arguments": args }
                });
                let mut req = client
                    .post(&url)
                    .header("Accept", "application/json, text/event-stream")
                    .json(&payload);
                if !bearer_tok.is_empty() {
                    req = req.bearer_auth(&bearer_tok);
                }
                if let Some(ref sid) = session_id {
                    req = req.header("Mcp-Session-Id", sid);
                }
                for (hk, hv) in &headers {
                    if !hk.trim().is_empty() {
                        req = req.header(hk.trim(), hv.trim());
                    }
                }
                let resp = req.send().map_err(|e| e.to_string())?;
                resp.text().map_err(|e| e.to_string())
            })
            .join()
            .map_err(|_| "MCP HTTP thread panicked".to_string())??;
            return Self::extract_mcp_result_payload(&raw_resp);
        }

        let mut cmd = Self::build_stdio_command(&cfg);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed spawning MCP server '{}': {}", cfg.name, e))?;

        let mut stdin_opt = child.stdin.take();
        if let Some(ref mut stdin) = stdin_opt {
            let init_msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "Unjess", "version": "4.0.0" }
                }
            });
            let initialized_notif = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            });
            let call_msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": tool_name,
                    "arguments": arguments
                }
            });
            let _ = writeln!(stdin, "{}", init_msg);
            let _ = writeln!(stdin, "{}", initialized_notif);
            let _ = writeln!(stdin, "{}", call_msg);
            let _ = stdin.flush();
        }

        let mut matched_id2_line = String::new();
        let mut fallback_line = String::new();
        if let Some(stdout) = child.stdout.take() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().take(300).flatten() {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_secs(25);
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(line) => {
                        let trimmed = line.trim();
                        if !trimmed.starts_with('{') {
                            continue;
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                            if json.get("id").and_then(|v| v.as_i64()) == Some(2) {
                                matched_id2_line = trimmed.to_string();
                                break;
                            }
                            fallback_line = trimmed.to_string();
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Ok(Some(_)) = child.try_wait() {
                            break;
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }

        drop(stdin_opt);
        kill_child_tree(&mut child);

        let response_line = if !matched_id2_line.is_empty() {
            matched_id2_line
        } else {
            fallback_line
        };

        if response_line.is_empty() {
            Err(format!(
                "MCP tool '{}' on '{}' returned no JSON-RPC response frame.",
                tool_name, cfg.name
            ))
        } else {
            Self::extract_mcp_result_payload(&response_line)
        }
    }

    /// Returns the built-in 1-click MCP server presets (`mashina-collab`, `rea`, `filesystem`, `github`, `fetch`).
    pub fn presets(mashina_token: &str) -> Vec<McpServerConfig> {
        let mut mashina_env = HashMap::new();
        if !mashina_token.trim().is_empty() {
            mashina_env.insert(
                "MASHINA_COLLAB_TOKEN".to_string(),
                mashina_token.trim().to_string(),
            );
        }

        vec![
            McpServerConfig {
                name: "mashina-collab".to_string(),
                command: String::new(),
                url: "https://mashina.si/api/collab/mcp".to_string(),
                args: Vec::new(),
                env: mashina_env,
                cwd: None,
                headers: HashMap::new(),
                enabled: !mashina_token.trim().is_empty(),
                description: "Mashina.si Multi-Agent Collaboration & Shared Dossier Bus".to_string(),
                cached_tools: Vec::new(),
                last_probe_ok: None,
                last_error: None,
            },
            McpServerConfig {
                name: "rea".to_string(),
                command: "npx".to_string(),
                url: String::new(),
                args: vec![
                    "-y".to_string(),
                    "rea-agents@latest".to_string(),
                    "mcp".to_string(),
                ],
                env: HashMap::new(),
                cwd: None,
                headers: HashMap::new(),
                enabled: true,
                description: "Reverse Engineer Anything (REA) Binary, APK, Firmware & Protocol Decompiler".to_string(),
                cached_tools: Vec::new(),
                last_probe_ok: None,
                last_error: None,
            },
            McpServerConfig {
                name: "filesystem".to_string(),
                command: "npx".to_string(),
                url: String::new(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-filesystem".to_string(),
                    "${workspaceRoot}".to_string(),
                ],
                env: HashMap::new(),
                cwd: None,
                headers: HashMap::new(),
                enabled: true,
                description: "Sandboxed local filesystem MCP provider".to_string(),
                cached_tools: Vec::new(),
                last_probe_ok: None,
                last_error: None,
            },
            McpServerConfig {
                name: "github".to_string(),
                command: "npx".to_string(),
                url: String::new(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                env: HashMap::new(),
                cwd: None,
                headers: HashMap::new(),
                enabled: false,
                description: "GitHub Issues, PRs, Actions & Repository search via MCP".to_string(),
                cached_tools: Vec::new(),
                last_probe_ok: None,
                last_error: None,
            },
            McpServerConfig {
                name: "fetch".to_string(),
                command: "npx".to_string(),
                url: String::new(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-fetch".to_string(),
                ],
                env: HashMap::new(),
                cwd: None,
                headers: HashMap::new(),
                enabled: true,
                description: "Headless web page & markdown documentation fetcher".to_string(),
                cached_tools: Vec::new(),
                last_probe_ok: None,
                last_error: None,
            },
        ]
    }

    /// Looks up a single 1-click preset by key.
    pub fn preset(key: &str, mashina_token: &str) -> Option<McpServerConfig> {
        Self::presets(mashina_token)
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(key))
    }
}
