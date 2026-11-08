//! Persistent Cron & Interval Background Task Scheduler (`~/.unjess-rust/schedules.json`).

use crate::agent::HarnessManager;
use crate::config::Settings;
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

static TICKER_STARTED: AtomicBool = AtomicBool::new(false);
static SCHEDULES_LOCK: Mutex<()> = Mutex::new(());
static LIVE_TASKS: Mutex<Vec<LiveBackgroundTask>> = Mutex::new(Vec::new());
static LIVE_NOTIFICATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Live in-memory background timer or file/command watcher (`AGY`-style reactive task).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBackgroundTask {
    pub id: String,
    pub kind: String, // "timer" | "watcher"
    pub label: String,
    pub payload: String,
    pub action: String, // "prompt" | "command"
    pub status: String, // "running" | "completed" | "cancelled" | "error"
    pub remaining_secs: u64,
    pub interval_secs: u64,
    pub trigger_count: usize,
    pub last_event: String,
    pub created_at: String,
}

/// A scheduled autonomous prompt or shell command execution task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cron_or_interval: String,
    #[serde(default)]
    pub cron_expression: String,
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default = "default_notify_on")]
    pub notify_on: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_output: Option<String>,
}

fn default_action() -> String {
    "prompt".to_string()
}

fn default_notify_on() -> String {
    "always".to_string()
}

fn default_enabled() -> bool {
    true
}

impl ScheduledTask {
    /// Synchronizes aliased fields (`name` <-> `title`, `cron_or_interval` <-> `cron_expression`, `last_run` <-> `last_run_at`).
    pub fn normalize(&mut self) {
        if self.title.is_empty() && !self.name.is_empty() {
            self.title = self.name.clone();
        } else if self.name.is_empty() && !self.title.is_empty() {
            self.name = self.title.clone();
        }
        if self.cron_expression.is_empty() && !self.cron_or_interval.is_empty() {
            self.cron_expression = self.cron_or_interval.clone();
        } else if self.cron_or_interval.is_empty() && !self.cron_expression.is_empty() {
            self.cron_or_interval = self.cron_expression.clone();
        }
        if self.last_run_at.is_none() && self.last_run.is_some() {
            self.last_run_at = self.last_run.clone();
        } else if self.last_run.is_none() && self.last_run_at.is_some() {
            self.last_run = self.last_run_at.clone();
        }
        if self.action.is_empty() {
            self.action = "prompt".to_string();
        }
        if self.notify_on.is_empty() {
            self.notify_on = "always".to_string();
        }
        if self.next_run_at.is_none() && self.enabled {
            self.next_run_at = Some(SchedulerManager::compute_next_run(&self.cron_expression));
        }
    }
}

pub struct SchedulerManager;

impl SchedulerManager {
    fn schedules_path() -> PathBuf {
        Settings::config_dir().join("schedules.json")
    }

    /// Computes an RFC3339 next run timestamp from an interval (`"every 30m"`, `"2h"`, `"1d"`)
    /// or 5-field cron expression (`"*/15 * * * *"`, `"0 9 * * *"`).
    pub fn compute_next_run(expr: &str) -> String {
        let raw = expr.trim().to_lowercase();
        let trimmed = raw.strip_prefix("every ").map(|s| s.trim()).unwrap_or(&raw);
        let now = chrono::Utc::now();

        if let Some(s) = trimmed.strip_suffix('s') {
            if let Ok(secs) = s.trim().parse::<i64>() {
                return (now + chrono::Duration::seconds(secs.max(10))).to_rfc3339();
            }
        }
        if let Some(m) = trimmed.strip_suffix('m') {
            if let Ok(mins) = m.trim().parse::<i64>() {
                return (now + chrono::Duration::minutes(mins.max(1))).to_rfc3339();
            }
        }
        if let Some(h) = trimmed.strip_suffix('h') {
            if let Ok(hours) = h.trim().parse::<i64>() {
                return (now + chrono::Duration::hours(hours.max(1))).to_rfc3339();
            }
        }
        if let Some(d) = trimmed.strip_suffix('d') {
            if let Ok(days) = d.trim().parse::<i64>() {
                return (now + chrono::Duration::days(days.max(1))).to_rfc3339();
            }
        }
        if trimmed.starts_with("*/") {
            let mins = trimmed
                .trim_start_matches("*/")
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(15);
            return (now + chrono::Duration::minutes(mins.max(1))).to_rfc3339();
        }

        // Standard 5-field cron: `<min> <hour> * * *`
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() >= 2 {
            let target_min = fields[0].parse::<u32>().ok();
            let target_hour = fields[1].parse::<u32>().ok();
            if let (Some(m), Some(h)) = (target_min, target_hour) {
                if m < 60 && h < 24 {
                    if let Some(today_candidate) = chrono::TimeZone::with_ymd_and_hms(
                        &chrono::Utc,
                        now.year(),
                        now.month(),
                        now.day(),
                        h,
                        m,
                        0,
                    )
                    .single()
                    {
                        let next_dt = if today_dt_in_future(today_candidate, now) {
                            today_candidate
                        } else {
                            today_candidate + chrono::Duration::days(1)
                        };
                        return next_dt.to_rfc3339();
                    }
                }
            } else if let Some(m) = target_min {
                if m < 60 && fields[1] == "*" {
                    let mut next_dt = now
                        .with_minute(m)
                        .and_then(|dt| dt.with_second(0))
                        .unwrap_or(now + chrono::Duration::hours(1));
                    if next_dt <= now {
                        next_dt += chrono::Duration::hours(1);
                    }
                    return next_dt.to_rfc3339();
                }
            }
        }

        (now + chrono::Duration::hours(1)).to_rfc3339()
    }

    /// Loads all scheduled tasks from `~/.unjess-rust/schedules.json`.
    pub fn load() -> Vec<ScheduledTask> {
        let path = Self::schedules_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut list) = serde_json::from_str::<Vec<ScheduledTask>>(&raw) {
                for t in &mut list {
                    t.normalize();
                }
                return list;
            }
        }
        Vec::new()
    }

    /// Saves scheduled tasks to `~/.unjess-rust/schedules.json`.
    pub fn save(tasks: &[ScheduledTask]) -> Result<(), String> {
        let path = Self::schedules_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut normalized = tasks.to_vec();
        for t in &mut normalized {
            t.normalize();
        }
        let json = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
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

    /// Executes a single scheduled task immediately (either `"command"` or `"prompt"`) and updates its status on disk.
    pub fn execute_task(hm: &Arc<HarnessManager>, task_id: &str) -> Result<ScheduledTask, String> {
        let mut task = {
            let _guard = SCHEDULES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut tasks = Self::load();
            let idx = tasks
                .iter()
                .position(|t| t.id == task_id)
                .ok_or_else(|| format!("Schedule '{}' not found", task_id))?;
            let mut t = tasks[idx].clone();
            t.normalize();
            t.last_status = Some("running".to_string());
            tasks[idx] = t.clone();
            let _ = Self::save(&tasks);
            t
        };

        let now_str = chrono::Utc::now().to_rfc3339();
        let ws_path = if task.workspace.trim().is_empty() {
            hm.active_workspace_path().to_string_lossy().to_string()
        } else {
            task.workspace.clone()
        };

        let (status, output) = if task.action == "command" {
            if let Ok(ws) = crate::workspace::Workspace::new(&ws_path) {
                let te = crate::tools::ToolEngine::new(ws);
                match te.run_command(&task.prompt, 120) {
                    Ok(out) => ("success".to_string(), out),
                    Err(e) => ("error".to_string(), e.to_string()),
                }
            } else {
                (
                    "error".to_string(),
                    format!("Invalid workspace path: {}", ws_path),
                )
            }
        } else {
            if !ws_path.trim().is_empty() {
                let ws_id = hm.add_workspace(&ws_path);
                hm.set_active_workspace(&ws_id);
            }
            let conv_title = format!(
                "Scheduled: {}",
                if task.title.is_empty() {
                    "Task"
                } else {
                    &task.title
                }
            );
            let conv_id = hm.new_conversation(Some(&conv_title));
            hm.submit_prompt(task.prompt.clone());

            // Spawn background observer to record final assistant output into `schedules.json` when turn finishes
            let hm_obs = Arc::clone(hm);
            let tid_obs = task.id.clone();
            let cid_obs = conv_id.clone();
            std::thread::spawn(move || {
                for _ in 0..600 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let snap = hm_obs.live_turn_snapshot();
                    if !snap.running {
                        let conv = hm_obs.conversation_by_id(&cid_obs);
                        if let Some(last_msg) = conv.messages.iter().rev().find(|m| m.role == "assistant") {
                            let _guard = SCHEDULES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                            let mut fresh = Self::load();
                            if let Some(slot) = fresh.iter_mut().find(|x| x.id == tid_obs) {
                                slot.last_output = Some(
                                    last_msg.content.chars().take(500).collect::<String>(),
                                );
                                let _ = Self::save(&fresh);
                            }
                        }
                        break;
                    }
                }
            });

            (
                "success".to_string(),
                format!("Dispatched autonomous prompt in conversation {}", conv_id),
            )
        };

        task.last_run = Some(now_str.clone());
        task.last_run_at = Some(now_str);
        task.next_run_at = Some(Self::compute_next_run(&task.cron_expression));
        task.last_status = Some(status);
        task.last_output = Some(output);

        {
            let _guard = SCHEDULES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut fresh_tasks = Self::load();
            if let Some(slot) = fresh_tasks.iter_mut().find(|t| t.id == task.id) {
                *slot = task.clone();
            } else {
                fresh_tasks.push(task.clone());
            }
            let _ = Self::save(&fresh_tasks);
        }
        Ok(task)
    }

    /// Starts an idempotent background ticker thread with an attached Tokio runtime that checks
    /// every 30 seconds for due enabled schedules.
    pub fn start_background_ticker(hm: Arc<HarnessManager>) {
        if TICKER_STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build();
            let _guard = rt.as_ref().ok().map(|r| r.enter());
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                let tasks = Self::load();
                let now = chrono::Utc::now();
                for task in tasks {
                    if !task.enabled {
                        continue;
                    }
                    let is_due = if let Some(ref next_str) = task.next_run_at {
                        if let Ok(next_dt) = chrono::DateTime::parse_from_rfc3339(next_str) {
                            now >= next_dt.with_timezone(&chrono::Utc)
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if is_due {
                        let _ = Self::execute_task(&hm, &task.id);
                    }
                }
            }
        });
    }

    /// Returns a snapshot of all live in-memory background timers and watchers.
    pub fn list_live_tasks() -> Vec<LiveBackgroundTask> {
        LIVE_TASKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Cancels a running timer or watcher by ID (or `"all"` to cancel all active live tasks).
    pub fn cancel_live_task(task_id: &str) -> usize {
        let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
        let mut count = 0;
        let clean = task_id.trim();
        for t in guard.iter_mut() {
            if t.status == "running" && (clean.eq_ignore_ascii_case("all") || t.id == clean) {
                t.status = "cancelled".to_string();
                t.last_event = "Cancelled by user".to_string();
                count += 1;
            }
        }
        count
    }

    /// Drains pending reactive notifications emitted by background timers and watchers.
    pub fn drain_notifications() -> Vec<String> {
        let mut guard = LIVE_NOTIFICATIONS.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    fn push_notification(msg: String) {
        let mut guard = LIVE_NOTIFICATIONS.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(msg);
    }

    /// Starts an AGY-style one-shot background countdown timer (`seconds`).
    /// When the timer expires, it executes `payload` as either a shell command (`action == "command"` or starts with `!`)
    /// or an autonomous agent prompt (`action == "prompt"`).
    pub fn start_timer(
        seconds: u64,
        action: &str,
        payload: &str,
        workspace: &str,
        hm_opt: Option<Arc<HarnessManager>>,
    ) -> LiveBackgroundTask {
        let secs = seconds.max(1);
        let id = format!(
            "timer-{}",
            chrono::Utc::now().timestamp_millis() % 1_000_000
        );
        let clean_payload = payload.trim().to_string();
        let resolved_action = if action.eq_ignore_ascii_case("command")
            || clean_payload.starts_with('!')
        {
            "command".to_string()
        } else {
            "prompt".to_string()
        };
        let exec_payload = clean_payload
            .strip_prefix('!')
            .map(|s| s.trim().to_string())
            .unwrap_or(clean_payload.clone());

        let task = LiveBackgroundTask {
            id: id.clone(),
            kind: "timer".to_string(),
            label: format!("{}s Timer -> {}", secs, exec_payload),
            payload: exec_payload.clone(),
            action: resolved_action.clone(),
            status: "running".to_string(),
            remaining_secs: secs,
            interval_secs: secs,
            trigger_count: 0,
            last_event: format!("Counting down ({}s remaining)", secs),
            created_at: chrono::Local::now().format("%H:%M:%S").to_string(),
        };

        {
            let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(task.clone());
        }

        let ws_str = workspace.to_string();
        std::thread::spawn(move || {
            for rem in (1..=secs).rev() {
                {
                    let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(t) = guard.iter_mut().find(|x| x.id == id) {
                        if t.status != "running" {
                            return;
                        }
                        t.remaining_secs = rem;
                        t.last_event = format!("Counting down ({}s remaining)", rem);
                    } else {
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }

            // Check if cancelled before firing
            {
                let guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
                if !guard.iter().any(|x| x.id == id && x.status == "running") {
                    return;
                }
            }

            let event_summary = if resolved_action == "command" {
                let out = crate::workspace::Workspace::new(&ws_str)
                    .ok()
                    .and_then(|ws| crate::tools::ToolEngine::new(ws).run_command(&exec_payload, 60).ok())
                    .unwrap_or_else(|| "Command executed".to_string());
                let first_line = out.lines().next().unwrap_or("done").trim();
                format!("Ran `{}` -> {}", exec_payload, first_line)
            } else if let Some(ref hm) = hm_opt {
                hm.submit_prompt(format!("[Timer {} Fired] {}", id, exec_payload));
                format!("Triggered agent turn: {}", exec_payload)
            } else {
                format!("Timer expired: {}", exec_payload)
            };

            {
                let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(t) = guard.iter_mut().find(|x| x.id == id) {
                    t.remaining_secs = 0;
                    t.trigger_count = 1;
                    t.status = "completed".to_string();
                    t.last_event = event_summary.clone();
                }
            }
            Self::push_notification(format!("⏰ Timer {} finished: {}", id, event_summary));
        });

        task
    }

    /// Starts a live background file or command watcher that polls every `interval_secs` seconds.
    /// - If `target` starts with `cmd:` or `!`, it runs that command and triggers whenever its output changes.
    /// - Otherwise, it hashes file modification timestamps & sizes in the workspace matching `target` (or the whole workspace)
    ///   and triggers whenever files are modified, created, or deleted!
    pub fn start_watcher(
        target: &str,
        interval_secs: u64,
        on_change: &str,
        workspace: &str,
        hm_opt: Option<Arc<HarnessManager>>,
    ) -> LiveBackgroundTask {
        let interval = interval_secs.clamp(2, 3600);
        let id = format!(
            "watch-{}",
            chrono::Utc::now().timestamp_millis() % 1_000_000
        );
        let clean_target = if target.trim().is_empty() {
            ".".to_string()
        } else {
            target.trim().to_string()
        };
        let clean_on_change = on_change.trim().to_string();

        let task = LiveBackgroundTask {
            id: id.clone(),
            kind: "watcher".to_string(),
            label: format!("Watch `{}` (every {}s)", clean_target, interval),
            payload: if clean_on_change.is_empty() {
                clean_target.clone()
            } else {
                format!("{} -> {}", clean_target, clean_on_change)
            },
            action: if clean_on_change.starts_with('!') {
                "command".to_string()
            } else {
                "watch".to_string()
            },
            status: "running".to_string(),
            remaining_secs: interval,
            interval_secs: interval,
            trigger_count: 0,
            last_event: format!("Watching `{}` for changes...", clean_target),
            created_at: chrono::Local::now().format("%H:%M:%S").to_string(),
        };

        {
            let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
            guard.push(task.clone());
        }

        let ws_str = workspace.to_string();
        std::thread::spawn(move || {
            let compute_fingerprint = || -> (String, String) {
                if let Some(cmd) = clean_target
                    .strip_prefix("cmd:")
                    .or_else(|| clean_target.strip_prefix('!'))
                {
                    let out = crate::workspace::Workspace::new(&ws_str)
                        .ok()
                        .and_then(|ws| crate::tools::ToolEngine::new(ws).run_command(cmd.trim(), 30).ok())
                        .unwrap_or_default();
                    let first = out.lines().next().unwrap_or("").trim().to_string();
                    (out, first)
                } else {
                    let root = PathBuf::from(&ws_str);
                    let target_filter = clean_target.trim_start_matches("./").to_lowercase();
                    let files = crate::workspace::Workspace::new(&root)
                        .map(|ws| ws.list_all_files(400))
                        .unwrap_or_default();
                    let mut sig = String::new();
                    let mut newest_file = String::new();
                    let mut newest_mtime = std::time::SystemTime::UNIX_EPOCH;
                    for rel in files {
                        let rel_low = rel.to_lowercase();
                        if target_filter != "."
                            && target_filter != "*"
                            && !rel_low.contains(&target_filter)
                        {
                            continue;
                        }
                        let full = root.join(&rel);
                        if let Ok(meta) = fs::metadata(&full) {
                            let mtime = meta.modified().unwrap_or( std::time::SystemTime::UNIX_EPOCH);
                            let secs = mtime
                                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            sig.push_str(&format!("{}:{}:{};", rel, meta.len(), secs));
                            if mtime > newest_mtime {
                                newest_mtime = mtime;
                                newest_file = rel;
                            }
                        }
                    }
                    (sig, newest_file)
                }
            };

            let (mut prev_sig, _) = compute_fingerprint();

            loop {
                for rem in (1..=interval).rev() {
                    {
                        let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(t) = guard.iter_mut().find(|x| x.id == id) {
                            if t.status != "running" {
                                return;
                            }
                            t.remaining_secs = rem;
                        } else {
                            return;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }

                let (new_sig, changed_hint) = compute_fingerprint();
                if new_sig != prev_sig {
                    prev_sig = new_sig;
                    let detail = if changed_hint.is_empty() {
                        format!("Change detected on `{}`", clean_target)
                    } else {
                        format!("Change detected: `{}`", changed_hint)
                    };

                    if !clean_on_change.is_empty() {
                        if let Some(cmd) = clean_on_change.strip_prefix('!') {
                            if let Ok(ws) = crate::workspace::Workspace::new(&ws_str) {
                                let _ = crate::tools::ToolEngine::new(ws).run_command(cmd.trim(), 60);
                            }
                        } else if let Some(ref hm) = hm_opt {
                            let snap = hm.live_turn_snapshot();
                            if !snap.running {
                                hm.submit_prompt(format!(
                                    "[Watcher {} Triggered — {}] {}",
                                    id, detail, clean_on_change
                                ));
                            }
                        }
                    }

                    {
                        let mut guard = LIVE_TASKS.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(t) = guard.iter_mut().find(|x| x.id == id) {
                            t.trigger_count += 1;
                            t.last_event = format!(
                                "{} (at {})",
                                detail,
                                chrono::Local::now().format("%H:%M:%S")
                            );
                        }
                    }
                    Self::push_notification(format!("👁 Watcher {}: {}", id, detail));
                }
            }
        });

        task
    }
}

fn today_dt_in_future(
    candidate: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    candidate > now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_cron_intervals_and_execution() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("UNJESS_RUST_CONFIG_DIR", tmp.path());
        }

        let now = chrono::Utc::now();
        for expr in ["every 30m", "every 2h", "every 1d", "*/15 * * * *", "0 22 * * *"] {
            let next_str = SchedulerManager::compute_next_run(expr);
            let parsed = chrono::DateTime::parse_from_rfc3339(&next_str).unwrap();
            assert!(parsed.with_timezone(&chrono::Utc) > now);
        }

        let mut task = ScheduledTask {
            id: "job-test-1".to_string(),
            name: "Echo Check".to_string(),
            title: String::new(),
            cron_or_interval: "every 15m".to_string(),
            cron_expression: String::new(),
            action: "command".to_string(),
            prompt: "echo scheduler_ok".to_string(),
            workspace: tmp.path().to_string_lossy().to_string(),
            notify_on: String::new(),
            enabled: true,
            last_run: None,
            last_run_at: None,
            next_run_at: None,
            last_status: None,
            last_output: None,
        };
        task.normalize();
        assert_eq!(task.title, "Echo Check");
        assert_eq!(task.cron_expression, "every 15m");
        assert!(task.next_run_at.is_some());

        SchedulerManager::save(&[task]).unwrap();
        let hm = Arc::new(HarnessManager::new());
        let executed = SchedulerManager::execute_task(&hm, "job-test-1").unwrap();
        assert_eq!(executed.last_status.as_deref(), Some("success"));
        assert!(executed
            .last_output
            .unwrap_or_default()
            .contains("scheduler_ok"));
    }
}
