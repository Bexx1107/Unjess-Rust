#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icons;
mod markdown;
mod theme;
mod views;

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextEdit, Ui,
};
use icons::{
    dark_action_btn, header_pill_btn, icon_btn, mode_segment_pill, modifier_pill_btn, paint_icon,
    pill_switch, sidebar_nav_item, Icon,
};
use markdown::render_markdown;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use unjess_core::{
    start_api_server, AstSymbol, ChatMessage, FileDiff, GitInfo, HarnessManager, McpManager,
    McpServerConfig, ModelInfo, ScheduledTask, SchedulerManager, Settings, SkillInfo,
    SkillsManager, Workspace,
};

/// An attached screenshot or image in the desktop composer.
#[derive(Debug, Clone)]
pub struct AttachedImage {
    pub path: PathBuf,
    pub filename: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainTab {
    Chat,
    Agents,
    Workflows,
    Schedules,
    Files,
    Browser,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSubTab {
    ModelsProviders,
    ApiKeys,
    SkillsLibrary,
    McpServers,
    BehaviorContext,
    DisplayBudgetRag,
    UpdatesChangelog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightInspectorTab {
    Diffs,
    Controls,
}

#[derive(Debug, Clone)]
pub enum PendingDeleteAction {
    Chat { id: String, title: String },
    Workspace { id: String, name: String },
    Skill { id: String, name: String },
    Subagent { id: String, name: String },
    Workflow { id: String, name: String },
    Schedule { id: String, name: String },
    McpServer { name: String },
}

pub struct UnjessDesktopApp {
    pub hm: Arc<HarnessManager>,
    pub active_tab: MainTab,
    pub settings_subtab: SettingsSubTab,
    pub right_inspector_tab: RightInspectorTab,

    pub show_left_sidebar: bool,
    pub show_right_sidebar: bool,

    // Chat & Composer state
    pub prompt_input: String,
    pub attached_images: Vec<AttachedImage>,
    pub renaming_conv_id: Option<String>,
    pub rename_conv_buf: String,
    pub renaming_ws_id: Option<String>,
    pub rename_ws_buf: String,
    pub pending_delete: Option<PendingDeleteAction>,
    pub expanded_activity_msgs: HashSet<usize>,
    pub show_live_activity_details: bool,
    pub collapsed_workspaces: HashSet<String>,

    // Cached Workspace & Git metadata
    pub git_info: GitInfo,
    pub workspace_files: Vec<String>,
    pub last_cache_refresh: Instant,

    // Files tab state
    pub file_filter_buf: String,
    pub selected_file_path: String,
    pub selected_file_content: String,
    pub ast_symbols: Vec<AstSymbol>,

    // Browser & Security tab state
    pub browser_query_buf: String,
    pub browser_result_buf: String,

    // Settings & Control Center state
    pub settings_draft: Settings,
    pub discovered_models: Vec<ModelInfo>,
    pub custom_model_override_buf: String,
    pub show_api_keys_plain: bool,
    pub skills_cache: Vec<SkillInfo>,
    pub github_skill_repo_buf: String,
    pub skill_search_buf: String,
    pub skill_scope_filter: String,
    pub mcp_cache: Vec<McpServerConfig>,
    pub schedules_cache: Vec<ScheduledTask>,
    pub mem_key_buf: String,
    pub mem_val_buf: String,

    // Modals & Drawers
    pub show_subagent_modal: bool,
    pub subagent_edit_id: String,
    pub subagent_name_buf: String,
    pub subagent_role_buf: String,
    pub subagent_prompt_buf: String,
    pub subagent_model_buf: String,
    pub selected_subagent_id: String,
    pub subagent_task_buf: String,

    pub show_workflow_modal: bool,
    pub wf_edit_id: String,
    pub wf_name_buf: String,
    pub wf_desc_buf: String,
    pub wf_steps_buf: String,

    pub show_schedule_modal: bool,
    pub sched_name_buf: String,
    pub sched_cron_buf: String,
    pub sched_prompt_buf: String,
    pub sched_action_buf: String,

    pub show_skill_modal: bool,
    pub skill_edit_id: String,
    pub skill_name_buf: String,
    pub skill_desc_buf: String,
    pub skill_folder_buf: String,
    pub skill_triggers_buf: String,
    pub skill_body_buf: String,

    pub show_terminal_drawer: bool,
    pub terminal_cmd_buf: String,
    pub terminal_output_buf: String,

    // Active tools switches in Controls inspector
    pub tool_enabled_read: bool,
    pub tool_enabled_write: bool,
    pub tool_enabled_edit: bool,
    pub tool_enabled_cmd: bool,
    pub tool_enabled_grep: bool,
    pub tool_enabled_web: bool,

    pub status_toast: Option<(String, Instant)>,
    pub ollama_probe_status: Option<(bool, String)>,
    pub lmstudio_probe_status: Option<(bool, String)>,
    pub llamacpp_probe_status: Option<(bool, String)>,
    pub mcp_probing_target: Arc<std::sync::Mutex<Option<String>>>,
    pub mcp_last_summary: Arc<std::sync::Mutex<Option<(bool, String)>>>,
    pub update_checking: Arc<std::sync::atomic::AtomicBool>,
    pub update_check_result:
        Arc<std::sync::Mutex<Option<Result<unjess_core::updater::UpdateCheckResult, String>>>>,
    pub update_downloading: Arc<std::sync::atomic::AtomicBool>,
    pub update_download_progress: Arc<std::sync::Mutex<Option<String>>>,
    pub slash_selected_idx: usize,
}

impl UnjessDesktopApp {
    pub fn new(cc: &eframe::CreationContext<'_>, hm: Arc<HarnessManager>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply_obsidian_theme(&cc.egui_ctx);

        let settings_draft = hm.settings();
        let ws_path = hm.active_workspace_path();
        let discovered_models = settings_draft.discover_models();
        let skills_cache = SkillsManager::list_skills_for_workspace(Some(&ws_path));
        let mcp_cache = McpManager::load();
        let schedules_cache = SchedulerManager::load();

        let (git_info, workspace_files) = if let Ok(ws) = Workspace::new(&ws_path) {
            (ws.git_info(), ws.list_all_files(500))
        } else {
            (
                GitInfo {
                    branch: "main".to_string(),
                    dirty: false,
                    changed_files: 0,
                },
                Vec::new(),
            )
        };

        Self {
            hm,
            active_tab: MainTab::Chat,
            settings_subtab: SettingsSubTab::ModelsProviders,
            right_inspector_tab: RightInspectorTab::Diffs,
            show_left_sidebar: true,
            show_right_sidebar: true,
            prompt_input: String::new(),
            attached_images: Vec::new(),
            renaming_conv_id: None,
            rename_conv_buf: String::new(),
            renaming_ws_id: None,
            rename_ws_buf: String::new(),
            pending_delete: None,
            expanded_activity_msgs: HashSet::new(),
            show_live_activity_details: false,
            collapsed_workspaces: HashSet::new(),
            git_info,
            workspace_files,
            last_cache_refresh: Instant::now(),
            file_filter_buf: String::new(),
            selected_file_path: String::new(),
            selected_file_content: String::new(),
            ast_symbols: Vec::new(),
            browser_query_buf: String::new(),
            browser_result_buf: String::new(),
            settings_draft,
            discovered_models,
            custom_model_override_buf: String::new(),
            show_api_keys_plain: false,
            skills_cache,
            github_skill_repo_buf: String::new(),
            skill_search_buf: String::new(),
            skill_scope_filter: "all".to_string(),
            mcp_cache,
            schedules_cache,
            mem_key_buf: String::new(),
            mem_val_buf: String::new(),
            show_subagent_modal: false,
            subagent_edit_id: String::new(),
            subagent_name_buf: String::new(),
            subagent_role_buf: String::new(),
            subagent_prompt_buf: String::new(),
            subagent_model_buf: "inherit".to_string(),
            selected_subagent_id: "sub-coder".to_string(),
            subagent_task_buf: String::new(),
            show_workflow_modal: false,
            wf_edit_id: String::new(),
            wf_name_buf: String::new(),
            wf_desc_buf: String::new(),
            wf_steps_buf: String::new(),
            show_schedule_modal: false,
            sched_name_buf: String::new(),
            sched_cron_buf: "every 1h".to_string(),
            sched_prompt_buf: String::new(),
            sched_action_buf: "prompt".to_string(),
            show_skill_modal: false,
            skill_edit_id: String::new(),
            skill_name_buf: String::new(),
            skill_desc_buf: String::new(),
            skill_folder_buf: "General".to_string(),
            skill_triggers_buf: String::new(),
            skill_body_buf: String::new(),
            show_terminal_drawer: false,
            terminal_cmd_buf: String::new(),
            terminal_output_buf: String::new(),
            tool_enabled_read: true,
            tool_enabled_write: true,
            tool_enabled_edit: true,
            tool_enabled_cmd: true,
            tool_enabled_grep: true,
            tool_enabled_web: true,
            status_toast: None,
            ollama_probe_status: None,
            lmstudio_probe_status: None,
            llamacpp_probe_status: None,
            mcp_probing_target: Arc::new(std::sync::Mutex::new(None)),
            mcp_last_summary: Arc::new(std::sync::Mutex::new(None)),
            update_checking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_check_result: Arc::new(std::sync::Mutex::new(None)),
            update_downloading: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_download_progress: Arc::new(std::sync::Mutex::new(None)),
            slash_selected_idx: 0,
        }
    }

    pub fn refresh_workspace_cache(&mut self) {
        let ws_path = self.hm.active_workspace_path();
        if let Ok(ws) = Workspace::new(&ws_path) {
            self.git_info = ws.git_info();
            self.workspace_files = ws.list_all_files(500);
        }
        self.skills_cache = SkillsManager::list_skills_for_workspace(Some(&ws_path));
        self.last_cache_refresh = Instant::now();
    }

    pub fn gui_slash_commands() -> Vec<(&'static str, &'static str, &'static str, bool)> {
        vec![
            ("/help", "/help", "Show all slash commands, timers, watchers & shortcuts", false),
            ("/context", "/context", "Inspect active workspace, model, token usage & cost", false),
            ("/compact", "/compact", "Compress conversation context to free up token window", false),
            ("/timer", "/timer <sec> <cmd|prompt>", "Schedule a background countdown timer (e.g. /timer 10 !cargo check)", true),
            ("/watch", "/watch <path|!cmd>", "Start a live background file or command watcher (e.g. /watch src)", true),
            ("/tasks", "/tasks", "List active background timers, file watchers & cron schedules", false),
            ("/stop-tasks", "/stop-tasks", "Cancel all running background timers and watchers", false),
            ("/run", "/run <command>", "Execute a shell command in the integrated terminal (or !<cmd>)", true),
            ("/clear", "/clear", "Clear active conversation messages", false),
            ("/new", "/new", "Create and switch to a fresh conversation", false),
            ("/plan", "/plan", "Switch to Plan Mode (architectural blueprint before coding)", false),
            ("/auto", "/auto", "Switch to Autonomous Mode (full multi-step execution)", false),
            ("/supervised", "/supervised", "Switch to Supervised Mode (confirm destructive commands)", false),
            ("/duel", "/duel", "Switch to Red Team vs. White Team Security Duel Mode", false),
            ("/caveman", "/caveman", "Toggle Caveman Mode (ultra-concise token compression)", false),
            ("/ponytail", "/ponytail", "Toggle Ponytail Mode (expressive pair-programming persona)", false),
            ("/skills", "/skills", "Open the Skills Library (47 skills)", false),
            ("/mcp", "/mcp", "Open & probe Model Context Protocol (MCP) servers", false),
            ("/subagents", "/subagents", "Open the Autonomous Subagent Fleet", false),
            ("/schedules", "/schedules", "Open the Background Schedules & Cron Manager", false),
            ("/undo", "/undo", "Undo last turn and revert file modifications", false),
        ]
    }

    /// Extracts an image from the system clipboard (e.g. from Windows Snipping Tool `Win+Shift+S`,
    /// Discord, or browser image copies), writes it as a PNG file in `<workspace>/.unjess/attachments/`
    /// (or `~/.unjess/attachments/`), and returns an `AttachedImage` descriptor.
    ///
    /// If the clipboard does not contain valid bitmap data, returns `None` so standard text paste
    /// into `TextEdit` proceeds completely unaffected.
    pub fn try_paste_clipboard_image(target_dir: &Path) -> Option<AttachedImage> {
        let mut clipboard = arboard::Clipboard::new().ok()?;

        let img_data = clipboard.get_image().ok()?;
        if img_data.width == 0 || img_data.height == 0 {
            return None;
        }

        let attach_dir = if target_dir.exists() && target_dir.is_dir() {
            target_dir.join(".unjess").join("attachments")
        } else {
            unjess_core::Settings::config_dir().join("attachments")
        };
        let _ = std::fs::create_dir_all(&attach_dir);

        let now = chrono::Utc::now();
        let filename = format!(
            "screenshot_{}_{:03}.png",
            now.format("%Y%m%d_%H%M%S"),
            now.timestamp_subsec_millis()
        );
        let out_path = attach_dir.join(&filename);

        let width = img_data.width as u32;
        let height = img_data.height as u32;
        let rgba_bytes = img_data.bytes.into_owned();

        let img = image::RgbaImage::from_raw(width, height, rgba_bytes)?;
        img.save(&out_path).ok()?;

        Some(AttachedImage {
            path: out_path,
            filename,
            width,
            height,
        })
    }

    /// Submits the prompt currently in the composer together with any attached screenshots.
    pub fn submit_current_composer_prompt(&mut self) {
        let clean = self.prompt_input.trim().to_string();
        if clean.is_empty() && self.attached_images.is_empty() {
            return;
        }

        // If the user typed a partial `/cmd` without spaces, resolve it to the highlighted slash command
        if clean.starts_with('/') && !clean.contains(' ') {
            let cmd_token = clean.to_lowercase();
            let matching: Vec<_> = Self::gui_slash_commands()
                .into_iter()
                .filter(|(c, _, _, _)| {
                    cmd_token == "/"
                        || c.starts_with(&cmd_token)
                        || c.contains(cmd_token.trim_start_matches('/'))
                })
                .collect();
            if let Some(&(sel_cmd, _, _, takes_args)) =
                matching.get(self.slash_selected_idx).or_else(|| matching.first())
            {
                if takes_args && clean != sel_cmd {
                    self.prompt_input = format!("{} ", sel_cmd);
                    return;
                } else {
                    self.prompt_input.clear();
                    self.dispatch_composer_input(sel_cmd.to_string());
                    return;
                }
            }
        }

        let mut final_prompt = if clean.is_empty() {
            "Please inspect the attached screenshot/image.".to_string()
        } else {
            clean
        };

        if !self.attached_images.is_empty() {
            for att in &self.attached_images {
                final_prompt.push_str(&format!(
                    "\n\n[Attached Screenshot: `{}` ({}×{})]",
                    att.path.display(),
                    att.width,
                    att.height
                ));
            }
            self.attached_images.clear();
        }

        self.prompt_input.clear();
        self.dispatch_composer_input(final_prompt);
    }

    pub fn dispatch_composer_input(&mut self, raw: String) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }

        // Direct shell execution via `!<cmd>`
        if let Some(shell_cmd) = trimmed.strip_prefix('!') {
            let cmd_clean = shell_cmd.trim().to_string();
            if !cmd_clean.is_empty() {
                self.show_terminal_drawer = true;
                self.terminal_cmd_buf = cmd_clean.clone();
                let ws_path = self.hm.active_workspace_path();
                if let Ok(ws) = Workspace::new(&ws_path) {
                    let te = unjess_core::ToolEngine::new(ws);
                    let out = te
                        .run_command(&cmd_clean, 60)
                        .unwrap_or_else(|e| format!("Error: {}", e));
                    self.terminal_output_buf
                        .push_str(&format!("$ {}\n{}\n\n", cmd_clean, out.trim()));
                }
                self.status_toast = Some((
                    format!("Executed `{}` in Terminal Drawer", cmd_clean),
                    Instant::now(),
                ));
            }
            return;
        }

        if !trimmed.starts_with('/') {
            self.hm.submit_prompt(raw);
            return;
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.next().unwrap_or("").trim();

        match cmd.as_str() {
            "/" | "/help" => {
                self.status_toast = Some((
                    "Slash Palette active — type /timer, /watch, /run, /compact, /context, /plan, /duel, etc.".to_string(),
                    Instant::now(),
                ));
            }
            "/context" => {
                let conv = self.hm.active_conversation();
                let ws = self.hm.active_workspace_path();
                self.status_toast = Some((
                    format!(
                        "Workspace: {} | Model: {} ({}) | Msgs: {} | Tokens: {} (${:.4})",
                        ws.display(),
                        self.settings_draft.model,
                        self.settings_draft.provider,
                        conv.messages.len(),
                        conv.total_tokens,
                        conv.total_cost_usd
                    ),
                    Instant::now(),
                ));
            }
            "/compact" => {
                self.hm.compact_active_conversation();
                self.status_toast = Some((
                    "Compacted active conversation context".to_string(),
                    Instant::now(),
                ));
            }
            "/clear" => {
                self.hm.clear_active_conversation();
                self.status_toast = Some((
                    "Cleared conversation messages".to_string(),
                    Instant::now(),
                ));
            }
            "/new" => {
                let title = if arg.is_empty() { None } else { Some(arg) };
                self.hm.new_conversation(title);
                self.status_toast = Some((
                    "Started new conversation".to_string(),
                    Instant::now(),
                ));
            }
            "/plan" => {
                self.hm.set_agent_mode("Plan");
                self.status_toast = Some(("Switched to Plan Mode".to_string(), Instant::now()));
                if !arg.is_empty() {
                    self.hm.submit_prompt(arg.to_string());
                }
            }
            "/auto" => {
                self.hm.set_agent_mode("Autonomous");
                self.status_toast =
                    Some(("Switched to Autonomous Mode".to_string(), Instant::now()));
                if !arg.is_empty() {
                    self.hm.submit_prompt(arg.to_string());
                }
            }
            "/supervised" => {
                self.hm.set_agent_mode("Supervised");
                self.status_toast =
                    Some(("Switched to Supervised Mode".to_string(), Instant::now()));
                if !arg.is_empty() {
                    self.hm.submit_prompt(arg.to_string());
                }
            }
            "/duel" => {
                self.hm.set_agent_mode("Duel");
                self.status_toast = Some((
                    "Switched to Red Team vs. White Team Security Duel Mode".to_string(),
                    Instant::now(),
                ));
                if !arg.is_empty() {
                    self.hm.submit_prompt(arg.to_string());
                }
            }
            "/caveman" => {
                let next = !self.hm.caveman_mode();
                self.hm.set_caveman_mode(next);
                self.status_toast = Some((
                    format!("Caveman Mode {}", if next { "ON" } else { "OFF" }),
                    Instant::now(),
                ));
            }
            "/ponytail" => {
                let next = !self.hm.ponytail_mode();
                self.hm.set_ponytail_mode(next);
                self.status_toast = Some((
                    format!("Ponytail Mode {}", if next { "ON" } else { "OFF" }),
                    Instant::now(),
                ));
            }
            "/run" => {
                if arg.is_empty() {
                    self.show_terminal_drawer = !self.show_terminal_drawer;
                } else {
                    self.dispatch_composer_input(format!("!{}", arg));
                }
            }
            "/timer" => {
                let mut tparts = arg.splitn(2, char::is_whitespace);
                let sec_str = tparts.next().unwrap_or("10").trim_end_matches('s');
                let secs = sec_str.parse::<u64>().unwrap_or(10).max(1);
                let payload = tparts.next().unwrap_or("Check workspace status").trim();
                let ws_str = self.hm.active_workspace_path().to_string_lossy().to_string();
                let action = if payload.starts_with('!') {
                    "command"
                } else {
                    "prompt"
                };
                let task = SchedulerManager::start_timer(
                    secs,
                    action,
                    payload,
                    &ws_str,
                    Some(Arc::clone(&self.hm)),
                );
                self.status_toast = Some((
                    format!("⏰ Started {}s timer ({}) -> {}", secs, task.id, payload),
                    Instant::now(),
                ));
            }
            "/watch" => {
                let mut wparts = arg.splitn(2, "->");
                let target = wparts.next().unwrap_or(".").trim();
                let on_change = wparts.next().unwrap_or("").trim();
                let ws_str = self.hm.active_workspace_path().to_string_lossy().to_string();
                let task = SchedulerManager::start_watcher(
                    if target.is_empty() { "." } else { target },
                    4,
                    on_change,
                    &ws_str,
                    Some(Arc::clone(&self.hm)),
                );
                self.status_toast = Some((
                    format!("👁 Started live watcher ({}) on `{}`", task.id, task.label),
                    Instant::now(),
                ));
            }
            "/tasks" => {
                let live = SchedulerManager::list_live_tasks();
                let running = live.iter().filter(|t| t.status == "running").count();
                self.status_toast = Some((
                    format!(
                        "{} running live timer/watcher(s) ({} total in session)",
                        running,
                        live.len()
                    ),
                    Instant::now(),
                ));
            }
            "/stop-tasks" => {
                let target = if arg.is_empty() { "all" } else { arg };
                let n = SchedulerManager::cancel_live_task(target);
                self.status_toast = Some((
                    format!("Cancelled {} background task(s)", n),
                    Instant::now(),
                ));
            }
            "/skills" => {
                self.active_tab = MainTab::Settings;
                self.settings_subtab = SettingsSubTab::SkillsLibrary;
            }
            "/mcp" => {
                self.active_tab = MainTab::Settings;
                self.settings_subtab = SettingsSubTab::McpServers;
            }
            "/subagents" | "/agents" => {
                self.active_tab = MainTab::Agents;
            }
            "/schedules" | "/schedule" => {
                self.active_tab = MainTab::Schedules;
            }
            "/undo" => {
                let cid = self.hm.active_conversation().id;
                self.hm.undo_last_turn(&cid);
                self.status_toast = Some(("Reverted last turn".to_string(), Instant::now()));
            }
            _ => {
                self.hm.submit_prompt(raw);
            }
        }
    }

    /// Renders the Top Global Header Bar (`40px` height) matching `media_1794094798213_77679872.jpg` & `media_1794094817258_ea42a648.png`.
    fn render_top_header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_global_header")
            .exact_height(40.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_HEADER)
                    .inner_margin(Margin::symmetric(10, 0))
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE)),
            )
            .show(ctx, |ui| {
                let bar_rect = ui.max_rect();

                // Left: Sidebar Toggle Button `[||]`
                ui.horizontal_centered(|ui| {
                    if icon_btn(
                        ui,
                        Icon::PanelLeft,
                        self.show_left_sidebar,
                        "Toggle Left Sidebar",
                    )
                    .clicked()
                    {
                        self.show_left_sidebar = !self.show_left_sidebar;
                    }

                    // Right-aligned cluster first so we can also place the centered cluster accurately
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        if icon_btn(
                            ui,
                            Icon::PanelRight,
                            self.show_right_sidebar,
                            "Toggle Right Inspector",
                        )
                        .clicked()
                        {
                            self.show_right_sidebar = !self.show_right_sidebar;
                        }

                        if icon_btn(
                            ui,
                            Icon::Sliders,
                            self.active_tab == MainTab::Settings,
                            "Settings & Control Center",
                        )
                        .clicked()
                        {
                            self.active_tab = MainTab::Settings;
                        }

                        if header_pill_btn(
                            ui,
                            Icon::Terminal,
                            "Terminal",
                            self.show_terminal_drawer,
                        )
                        .clicked()
                        {
                            self.show_terminal_drawer = !self.show_terminal_drawer;
                        }

                        if header_pill_btn(
                            ui,
                            Icon::Search,
                            "Files",
                            self.active_tab == MainTab::Files,
                        )
                        .clicked()
                        {
                            self.active_tab = MainTab::Files;
                        }

                        if header_pill_btn(
                            ui,
                            Icon::Cpu,
                            "Subagents",
                            self.active_tab == MainTab::Agents,
                        )
                        .clicked()
                        {
                            self.active_tab = MainTab::Agents;
                        }
                    });
                });

                // Center overlay: `[Workspace: unjess v]` + `[git-branch main ● +4]`
                let workspaces = self.hm.workspaces();
                let active_ws_id = self.hm.active_workspace_id();
                let active_ws_name = workspaces
                    .iter()
                    .find(|w| w.id == active_ws_id)
                    .or_else(|| workspaces.first())
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "unjess".to_string());

                let center_width = 290.0;
                let center_rect = egui::Rect::from_center_size(
                    bar_rect.center(),
                    egui::vec2(center_width, 28.0),
                );
                let mut child_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(center_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child_ui.spacing_mut().item_spacing.x = 6.0;

                // Workspace selector pill
                let ws_btn_w = (48.0 + active_ws_name.len() as f32 * 6.8 + 42.0).clamp(135.0, 210.0);
                let (ws_rect, ws_resp) =
                    child_ui.allocate_exact_size(egui::vec2(ws_btn_w, 26.0), Sense::click());
                if ws_resp.hovered() {
                    child_ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                child_ui.painter().rect(
                    ws_rect,
                    CornerRadius::same(6),
                    if ws_resp.hovered() {
                        theme::BG_CARD_HOVER
                    } else {
                        Color32::from_rgb(18, 18, 21)
                    },
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                    StrokeKind::Inside,
                );
                let ic_folder = egui::Rect::from_center_size(
                    egui::pos2(ws_rect.left() + 13.0, ws_rect.center().y),
                    egui::vec2(12.0, 12.0),
                );
                paint_icon(
                    child_ui.painter(),
                    ic_folder,
                    Icon::Folder,
                    theme::ACCENT_AMBER,
                );
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    "Workspace: ",
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::new(11.5, FontFamily::Proportional),
                        color: theme::TEXT_SECONDARY,
                        ..Default::default()
                    },
                );
                job.append(
                    &active_ws_name,
                    0.0,
                    egui::TextFormat {
                        font_id: FontId::new(11.5, FontFamily::Proportional),
                        color: theme::TEXT_WHITE,
                        ..Default::default()
                    },
                );
                let galley = child_ui.painter().layout_job(job);
                child_ui.painter().galley(
                    egui::pos2(
                        ws_rect.left() + 24.0,
                        ws_rect.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    theme::TEXT_WHITE,
                );
                let ic_chev = egui::Rect::from_center_size(
                    egui::pos2(ws_rect.right() - 11.0, ws_rect.center().y),
                    egui::vec2(10.0, 10.0),
                );
                paint_icon(
                    child_ui.painter(),
                    ic_chev,
                    Icon::ChevronDown,
                    theme::TEXT_MUTED,
                );

                let ws_popup_id = child_ui.make_persistent_id("header_ws_popup");
                if ws_resp.clicked() {
                    child_ui.memory_mut(|mem| mem.toggle_popup(ws_popup_id));
                }
                egui::popup::popup_below_widget(
                    &child_ui,
                    ws_popup_id,
                    &ws_resp,
                    egui::popup::PopupCloseBehavior::CloseOnClickOutside,
                    |ui| {
                        ui.set_min_width(210.0);
                        for ws in &workspaces {
                            let is_cur = ws.id == active_ws_id;
                            if ui
                                .selectable_label(is_cur, format!("{} ({})", ws.name, ws.path))
                                .clicked()
                            {
                                self.hm.set_active_workspace(&ws.id);
                                self.refresh_workspace_cache();
                            }
                        }
                        ui.separator();
                        if ui.button("+ Add Workspace Folder...").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                let id = self.hm.add_workspace(&folder.to_string_lossy());
                                self.hm.set_active_workspace(&id);
                                self.refresh_workspace_cache();
                            }
                        }
                    },
                );

                // Git branch & dirty count badge `[git-branch main ● +4]`
                let branch_name = if self.git_info.branch.is_empty() {
                    "main"
                } else {
                    &self.git_info.branch
                };
                let dirty_str = if self.git_info.changed_files > 0 {
                    format!("+{}", self.git_info.changed_files)
                } else {
                    "+0".to_string()
                };
                let git_w = (46.0 + (branch_name.len() + dirty_str.len()) as f32 * 6.8)
                    .clamp(86.0, 140.0);
                let (git_rect, _) =
                    child_ui.allocate_exact_size(egui::vec2(git_w, 26.0), Sense::hover());
                child_ui.painter().rect(
                    git_rect,
                    CornerRadius::same(6),
                    Color32::from_rgb(18, 18, 21),
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                    StrokeKind::Inside,
                );
                let ic_git = egui::Rect::from_center_size(
                    egui::pos2(git_rect.left() + 12.0, git_rect.center().y),
                    egui::vec2(12.0, 12.0),
                );
                paint_icon(
                    child_ui.painter(),
                    ic_git,
                    Icon::GitBranch,
                    theme::ACCENT_EMERALD,
                );
                child_ui.painter().text(
                    egui::pos2(git_rect.left() + 22.0, git_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    branch_name,
                    FontId::new(11.5, FontFamily::Monospace),
                    theme::TEXT_WHITE,
                );
                let dot_x = git_rect.right() - 28.0;
                child_ui.painter().circle_filled(
                    egui::pos2(dot_x, git_rect.center().y),
                    3.0,
                    theme::TEXT_SECONDARY,
                );
                child_ui.painter().text(
                    egui::pos2(git_rect.right() - 8.0, git_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    dirty_str,
                    FontId::new(10.5, FontFamily::Monospace),
                    theme::TEXT_MUTED,
                );
            });
    }

    /// Renders the Left Navigation & Projects Sidebar (`235px` width) matching `media_1794094798213_77679872.jpg`.
    fn render_left_sidebar(&mut self, ctx: &egui::Context) {
        if !self.show_left_sidebar {
            return;
        }

        egui::SidePanel::left("left_navigation_sidebar")
            .default_width(235.0)
            .width_range(160.0..=500.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(Margin::symmetric(10, 10))
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 3.0;

                let sub_count = self.hm.subagents().len();
                let wf_count = self.hm.workflows().len();
                let sched_count = self.schedules_cache.len();
                let file_count = self.workspace_files.len();

                // 7 Primary Navigation Stack Items
                if sidebar_nav_item(
                    ui,
                    Icon::MessageSquare,
                    "Chat",
                    None,
                    self.active_tab == MainTab::Chat,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Chat;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::Cpu,
                    "Agents",
                    Some(&sub_count.to_string()),
                    self.active_tab == MainTab::Agents,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Agents;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::GitMerge,
                    "Workflows",
                    Some(&wf_count.to_string()),
                    self.active_tab == MainTab::Workflows,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Workflows;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::Clock,
                    "Schedules",
                    Some(&sched_count.to_string()),
                    self.active_tab == MainTab::Schedules,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Schedules;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::Folder,
                    "Files",
                    Some(&file_count.to_string()),
                    self.active_tab == MainTab::Files,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Files;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::Globe,
                    "Browser",
                    None,
                    self.active_tab == MainTab::Browser,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Browser;
                }
                if sidebar_nav_item(
                    ui,
                    Icon::Settings,
                    "Settings",
                    None,
                    self.active_tab == MainTab::Settings,
                )
                .clicked()
                {
                    self.active_tab = MainTab::Settings;
                }

                ui.add_space(8.0);
                let (sep1, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().line_segment(
                    [sep1.left_center(), sep1.right_center()],
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                );
                ui.add_space(6.0);

                // Projects Section Header
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Projects")
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if icon_btn(ui, Icon::FolderPlus, false, "Open Workspace Folder").clicked()
                        {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                let id = self.hm.add_workspace(&folder.to_string_lossy());
                                self.hm.set_active_workspace(&id);
                                self.refresh_workspace_cache();
                            }
                        }
                        if icon_btn(ui, Icon::ChevronsUpDown, false, "Collapse / Expand All")
                            .clicked()
                        {
                            if self.collapsed_workspaces.is_empty() {
                                for w in self.hm.workspaces() {
                                    self.collapsed_workspaces.insert(w.id);
                                }
                            } else {
                                self.collapsed_workspaces.clear();
                            }
                        }
                    });
                });

                ui.add_space(4.0);

                // Workspace Folders & Conversations Tree
                let workspaces = self.hm.workspaces();
                let all_convs = self.hm.conversations();
                let active_conv_id = self.hm.active_conversation_id();

                ScrollArea::vertical()
                    .id_salt("left_projects_scroll")
                    .max_height((ui.available_height() - 125.0).max(120.0))
                    .show(ui, |ui| {
                        for ws in &workspaces {
                            let ws_convs: Vec<_> = all_convs
                                .iter()
                                .filter(|c| {
                                    c.mode != "quick"
                                        && (c.workspace_id == ws.id || workspaces.len() == 1)
                                })
                                .collect();
                            let is_collapsed = self.collapsed_workspaces.contains(&ws.id);

                            // Workspace Folder Row (`v [folder] unjess 2   + [edit] [del]`)
                            if self.renaming_ws_id.as_deref() == Some(&ws.id) {
                                ui.horizontal(|ui| {
                                    let resp = ui.add_sized(
                                        egui::vec2(ui.available_width() - 48.0, 24.0),
                                        TextEdit::singleline(&mut self.rename_ws_buf),
                                    );
                                    if (resp.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                        || icon_btn(ui, Icon::Check, false, "Save Folder Name")
                                            .clicked()
                                    {
                                        self.hm.rename_workspace(&ws.id, &self.rename_ws_buf);
                                        self.renaming_ws_id = None;
                                    }
                                    if icon_btn(ui, Icon::X, false, "Cancel").clicked() {
                                        self.renaming_ws_id = None;
                                    }
                                });
                            } else {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    let row_w = (ui.available_width() - 62.0).max(95.0);
                                    let (f_rect, f_resp) = ui.allocate_exact_size(
                                        egui::vec2(row_w, 26.0),
                                        Sense::click(),
                                    );
                                    if f_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                        ui.painter().rect_filled(
                                            f_rect,
                                            CornerRadius::same(5),
                                            Color32::from_rgb(18, 18, 22),
                                        );
                                    }
                                    let chev_r = egui::Rect::from_center_size(
                                        egui::pos2(f_rect.left() + 8.0, f_rect.center().y),
                                        egui::vec2(10.0, 10.0),
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        chev_r,
                                        if is_collapsed {
                                            Icon::ChevronRight
                                        } else {
                                            Icon::ChevronDown
                                        },
                                        theme::TEXT_MUTED,
                                    );
                                    let fold_r = egui::Rect::from_center_size(
                                        egui::pos2(f_rect.left() + 23.0, f_rect.center().y),
                                        egui::vec2(13.0, 13.0),
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        fold_r,
                                        Icon::FolderOpen,
                                        theme::ACCENT_EMERALD,
                                    );
                                    let ws_display = if ws.name.chars().count() > 14 {
                                        let s: String = ws.name.chars().take(12).collect();
                                        format!("{s}..")
                                    } else {
                                        ws.name.clone()
                                    };
                                    ui.painter().text(
                                        egui::pos2(f_rect.left() + 35.0, f_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        &ws_display,
                                        FontId::new(12.5, FontFamily::Proportional),
                                        theme::TEXT_WHITE,
                                    );
                                    let name_w = ws_display.len() as f32 * 6.8;
                                    ui.painter().text(
                                        egui::pos2(
                                            (f_rect.left() + 42.0 + name_w)
                                                .min(f_rect.right() - 8.0),
                                            f_rect.center().y,
                                        ),
                                        egui::Align2::LEFT_CENTER,
                                        ws_convs.len().to_string(),
                                        FontId::new(10.5, FontFamily::Monospace),
                                        theme::TEXT_DIM,
                                    );
                                    if f_resp.clicked() {
                                        self.hm.set_active_workspace(&ws.id);
                                        self.refresh_workspace_cache();
                                        if is_collapsed {
                                            self.collapsed_workspaces.remove(&ws.id);
                                        } else {
                                            self.collapsed_workspaces.insert(ws.id.clone());
                                        }
                                    }

                                    // Right-click Context Menu on Workspace Folder
                                    f_resp.context_menu(|ui| {
                                        if ui.button("+ New Chat in Folder").clicked() {
                                            self.hm.set_active_workspace(&ws.id);
                                            self.hm.new_conversation(Some("New Chat"));
                                            self.active_tab = MainTab::Chat;
                                            ui.close_menu();
                                        }
                                        if ui.button("Rename Folder").clicked() {
                                            self.renaming_ws_id = Some(ws.id.clone());
                                            self.rename_ws_buf = ws.name.clone();
                                            ui.close_menu();
                                        }
                                        if workspaces.len() > 1
                                            && ui.button("Delete / Remove Folder").clicked()
                                        {
                                            self.pending_delete =
                                                Some(PendingDeleteAction::Workspace {
                                                    id: ws.id.clone(),
                                                    name: ws.name.clone(),
                                                });
                                            ui.close_menu();
                                        }
                                    });

                                    // Action buttons on right of Workspace Folder row (`+`, `Edit`, `Trash`)
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = 1.0;
                                            if workspaces.len() > 1
                                                && icon_btn(
                                                    ui,
                                                    Icon::Trash2,
                                                    false,
                                                    "Remove Workspace Folder",
                                                )
                                                .clicked()
                                            {
                                                self.pending_delete =
                                                    Some(PendingDeleteAction::Workspace {
                                                        id: ws.id.clone(),
                                                        name: ws.name.clone(),
                                                    });
                                            }
                                            if icon_btn(
                                                ui,
                                                Icon::Edit3,
                                                false,
                                                "Rename Workspace Folder",
                                            )
                                            .clicked()
                                            {
                                                self.renaming_ws_id = Some(ws.id.clone());
                                                self.rename_ws_buf = ws.name.clone();
                                            }
                                            if icon_btn(
                                                ui,
                                                Icon::Plus,
                                                false,
                                                "New Chat in Workspace",
                                            )
                                            .clicked()
                                            {
                                                self.hm.set_active_workspace(&ws.id);
                                                self.hm.new_conversation(Some("New Chat"));
                                                self.active_tab = MainTab::Chat;
                                            }
                                        },
                                    );
                                });
                            }

                            if !is_collapsed {
                                for conv in ws_convs {
                                    if self.renaming_conv_id.as_deref() == Some(&conv.id) {
                                        ui.horizontal(|ui| {
                                            ui.add_space(16.0);
                                            let resp = ui.add_sized(
                                                egui::vec2(ui.available_width() - 48.0, 24.0),
                                                TextEdit::singleline(&mut self.rename_conv_buf),
                                            );
                                            if (resp.lost_focus()
                                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                                || icon_btn(
                                                    ui,
                                                    Icon::Check,
                                                    false,
                                                    "Save Chat Title",
                                                )
                                                .clicked()
                                            {
                                                self.hm.rename_conversation(
                                                    &conv.id,
                                                    &self.rename_conv_buf,
                                                );
                                                self.renaming_conv_id = None;
                                            }
                                            if icon_btn(ui, Icon::X, false, "Cancel").clicked() {
                                                self.renaming_conv_id = None;
                                            }
                                        });
                                        continue;
                                    }

                                    let is_active = conv.id == active_conv_id
                                        && self.active_tab == MainTab::Chat;
                                    let full_row_rect = ui.available_rect_before_wrap();
                                    let row_rect = egui::Rect::from_min_size(
                                        full_row_rect.min,
                                        egui::vec2(ui.available_width(), 28.0),
                                    );
                                    let row_hovered = ui.rect_contains_pointer(row_rect);

                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 1.0;
                                        let main_w = if row_hovered {
                                            (ui.available_width() - 44.0).max(90.0)
                                        } else {
                                            ui.available_width()
                                        };
                                        let (c_rect, c_resp) = ui.allocate_exact_size(
                                            egui::vec2(main_w, 28.0),
                                            Sense::click(),
                                        );
                                        if c_resp.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                        // Vertical tree guide line on left
                                        ui.painter().line_segment(
                                            [
                                                egui::pos2(c_rect.left() + 10.0, c_rect.top()),
                                                egui::pos2(c_rect.left() + 10.0, c_rect.bottom()),
                                            ],
                                            Stroke::new(1.0, Color32::from_rgb(28, 28, 33)),
                                        );

                                        let item_rect =
                                            row_rect.with_min_x(c_rect.left() + 16.0);
                                        if is_active {
                                            ui.painter().rect_filled(
                                                item_rect,
                                                CornerRadius::same(6),
                                                Color32::from_rgb(24, 24, 27),
                                            );
                                        } else if row_hovered {
                                            ui.painter().rect_filled(
                                                item_rect,
                                                CornerRadius::same(6),
                                                Color32::from_rgb(16, 16, 19),
                                            );
                                        }

                                        let max_chars = if row_hovered { 15 } else { 20 };
                                        let title_short = if conv.title.chars().count() > max_chars
                                        {
                                            let s: String =
                                                conv.title.chars().take(max_chars - 2).collect();
                                            format!("{s}..")
                                        } else {
                                            conv.title.clone()
                                        };
                                        ui.painter().text(
                                            egui::pos2(
                                                item_rect.left() + 10.0,
                                                item_rect.center().y,
                                            ),
                                            egui::Align2::LEFT_CENTER,
                                            title_short,
                                            FontId::new(12.5, FontFamily::Proportional),
                                            if is_active {
                                                theme::TEXT_WHITE
                                            } else {
                                                theme::TEXT_SECONDARY
                                            },
                                        );

                                        if !row_hovered {
                                            ui.painter().text(
                                                egui::pos2(
                                                    item_rect.right() - 8.0,
                                                    item_rect.center().y,
                                                ),
                                                egui::Align2::RIGHT_CENTER,
                                                "now",
                                                FontId::new(10.5, FontFamily::Proportional),
                                                theme::TEXT_DIM,
                                            );
                                        }

                                        if c_resp.clicked() {
                                            self.hm.set_active_conversation(&conv.id);
                                            self.active_tab = MainTab::Chat;
                                        }

                                        // Right-click Context Menu on Chat Row
                                        c_resp.context_menu(|ui| {
                                            if ui.button("Rename Chat").clicked() {
                                                self.renaming_conv_id = Some(conv.id.clone());
                                                self.rename_conv_buf = conv.title.clone();
                                                ui.close_menu();
                                            }
                                            if ui.button("Delete Chat").clicked() {
                                                self.pending_delete =
                                                    Some(PendingDeleteAction::Chat {
                                                        id: conv.id.clone(),
                                                        title: conv.title.clone(),
                                                    });
                                                ui.close_menu();
                                            }
                                        });

                                        // Hover inline Rename & Delete buttons
                                        if row_hovered {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.x = 1.0;
                                                    if icon_btn(
                                                        ui,
                                                        Icon::Trash2,
                                                        false,
                                                        "Delete Chat",
                                                    )
                                                    .clicked()
                                                    {
                                                        self.pending_delete =
                                                            Some(PendingDeleteAction::Chat {
                                                                id: conv.id.clone(),
                                                                title: conv.title.clone(),
                                                            });
                                                    }
                                                    if icon_btn(
                                                        ui,
                                                        Icon::Edit3,
                                                        false,
                                                        "Rename Chat",
                                                    )
                                                    .clicked()
                                                    {
                                                        self.renaming_conv_id =
                                                            Some(conv.id.clone());
                                                        self.rename_conv_buf = conv.title.clone();
                                                    }
                                                },
                                            );
                                        }
                                    });
                                }
                            }
                        }
                    });

                ui.add_space(6.0);
                let (sep2, _) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().line_segment(
                    [sep2.left_center(), sep2.right_center()],
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                );
                ui.add_space(6.0);

                // Quick Chats Section
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Quick Chats")
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_btn(ui, Icon::Plus, false, "New Quick Chat").clicked() {
                            self.hm
                                .new_conversation_with_mode(Some("Quick Chat"), Some("quick"));
                            self.active_tab = MainTab::Chat;
                        }
                    });
                });

                let quick_convs: Vec<_> =
                    all_convs.iter().filter(|c| c.mode == "quick").collect();
                if quick_convs.is_empty() {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("No quick chats yet.")
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                } else {
                    ScrollArea::vertical()
                        .id_salt("left_quick_chats_scroll")
                        .max_height(90.0)
                        .show(ui, |ui| {
                            for qc in quick_convs {
                                if self.renaming_conv_id.as_deref() == Some(&qc.id) {
                                    ui.horizontal(|ui| {
                                        let resp = ui.add_sized(
                                            egui::vec2(ui.available_width() - 48.0, 24.0),
                                            TextEdit::singleline(&mut self.rename_conv_buf),
                                        );
                                        if (resp.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                            || icon_btn(ui, Icon::Check, false, "Save").clicked()
                                        {
                                            self.hm
                                                .rename_conversation(&qc.id, &self.rename_conv_buf);
                                            self.renaming_conv_id = None;
                                        }
                                        if icon_btn(ui, Icon::X, false, "Cancel").clicked() {
                                            self.renaming_conv_id = None;
                                        }
                                    });
                                    continue;
                                }

                                let is_active =
                                    qc.id == active_conv_id && self.active_tab == MainTab::Chat;
                                ui.horizontal(|ui| {
                                    let resp = ui.selectable_label(
                                        is_active,
                                        RichText::new(&qc.title).size(12.0).color(if is_active {
                                            theme::TEXT_WHITE
                                        } else {
                                            theme::TEXT_SECONDARY
                                        }),
                                    );
                                    if resp.clicked() {
                                        self.hm.set_active_conversation(&qc.id);
                                        self.active_tab = MainTab::Chat;
                                    }
                                    resp.context_menu(|ui| {
                                        if ui.button("Rename Quick Chat").clicked() {
                                            self.renaming_conv_id = Some(qc.id.clone());
                                            self.rename_conv_buf = qc.title.clone();
                                            ui.close_menu();
                                        }
                                        if ui.button("Delete Quick Chat").clicked() {
                                            self.pending_delete = Some(PendingDeleteAction::Chat {
                                                id: qc.id.clone(),
                                                title: qc.title.clone(),
                                            });
                                            ui.close_menu();
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = 1.0;
                                            if icon_btn(
                                                ui,
                                                Icon::Trash2,
                                                false,
                                                "Delete Quick Chat",
                                            )
                                            .clicked()
                                            {
                                                self.pending_delete =
                                                    Some(PendingDeleteAction::Chat {
                                                        id: qc.id.clone(),
                                                        title: qc.title.clone(),
                                                    });
                                            }
                                            if icon_btn(
                                                ui,
                                                Icon::Edit3,
                                                false,
                                                "Rename Quick Chat",
                                            )
                                            .clicked()
                                            {
                                                self.renaming_conv_id = Some(qc.id.clone());
                                                self.rename_conv_buf = qc.title.clone();
                                            }
                                        },
                                    );
                                });
                            }
                        });
                }
            });
    }

    /// Renders the Right Inspector Sidebar (`290px` width) matching `media_1794094798213_77679872.jpg`.
    fn render_right_inspector(&mut self, ctx: &egui::Context) {
        if !self.show_right_sidebar || self.active_tab != MainTab::Chat {
            return;
        }

        egui::SidePanel::right("right_changes_inspector")
            .default_width(290.0)
            .width_range(200.0..=700.0)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(Margin::same(0))
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE)),
            )
            .show(ctx, |ui| {
                // Inspector Header (`44px` height)
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ic_r, _) =
                                ui.allocate_exact_size(egui::vec2(15.0, 15.0), Sense::hover());
                            paint_icon(
                                ui.painter(),
                                ic_r,
                                if self.right_inspector_tab == RightInspectorTab::Diffs {
                                    Icon::FileDiff
                                } else {
                                    Icon::Sliders
                                },
                                theme::TEXT_SECONDARY,
                            );
                            ui.label(
                                RichText::new(
                                    if self.right_inspector_tab == RightInspectorTab::Diffs {
                                        "Changes & Diffs"
                                    } else {
                                        "Agent Controls"
                                    },
                                )
                                .size(13.0)
                                .strong()
                                .color(theme::TEXT_WHITE),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let toggle_label =
                                        if self.right_inspector_tab == RightInspectorTab::Diffs {
                                            "Controls"
                                        } else {
                                            "Diffs"
                                        };
                                    if dark_action_btn(
                                        ui,
                                        Some(Icon::Sliders),
                                        toggle_label,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        self.right_inspector_tab = match self.right_inspector_tab {
                                            RightInspectorTab::Diffs => RightInspectorTab::Controls,
                                            RightInspectorTab::Controls => RightInspectorTab::Diffs,
                                        };
                                    }
                                },
                            );
                        });
                    });

                let (sep_r, _) = ui
                    .allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().line_segment(
                    [sep_r.left_center(), sep_r.right_center()],
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                );

                match self.right_inspector_tab {
                    RightInspectorTab::Diffs => self.render_inspector_diffs(ui),
                    RightInspectorTab::Controls => self.render_inspector_controls(ui),
                }
            });
    }

    fn render_inspector_diffs(&mut self, ui: &mut Ui) {
        let conv = self.hm.active_conversation();
        let live = self.hm.live_turn_snapshot();
        let mut all_diffs: Vec<FileDiff> = Vec::new();
        for m in &conv.messages {
            for d in &m.diffs {
                all_diffs.push(d.clone());
            }
        }
        if live.running && live.conv_id == conv.id {
            for d in &live.diffs {
                all_diffs.push(d.clone());
            }
        }

        if all_diffs.is_empty() {
            // Centered Empty State matching `media_1794094798213_77679872.jpg`
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                let (box_r, _) =
                    ui.allocate_exact_size(egui::vec2(42.0, 42.0), Sense::hover());
                ui.painter().rect(
                    box_r,
                    CornerRadius::same(10),
                    theme::BG_CARD,
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                    StrokeKind::Inside,
                );
                let ic_r =
                    egui::Rect::from_center_size(box_r.center(), egui::vec2(18.0, 18.0));
                paint_icon(ui.painter(), ic_r, Icon::FilePlus, theme::TEXT_DIM);

                ui.add_space(12.0);
                ui.label(
                    RichText::new("No code diffs yet")
                        .size(12.5)
                        .strong()
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Code modifications made by the\nagent will appear here in real-time.",
                    )
                    .size(11.5)
                    .color(theme::TEXT_DIM),
                );
            });
        } else {
            ScrollArea::vertical()
                .id_salt("inspector_diffs_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(10.0);
                    for diff in all_diffs.iter().rev() {
                        egui::Frame::new()
                            .fill(theme::BG_CARD)
                            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(10))
                            .outer_margin(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&diff.path)
                                            .size(11.5)
                                            .strong()
                                            .family(FontFamily::Monospace)
                                            .color(theme::TEXT_WHITE),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if dark_action_btn(
                                                ui,
                                                Some(Icon::RotateCcw),
                                                "Revert",
                                                false,
                                            )
                                            .clicked()
                                            {
                                                if self.hm.revert_diff(diff).is_ok() {
                                                    self.hm.remove_diff_from_conversation(
                                                        &conv.id, &diff.path,
                                                    );
                                                }
                                            }
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                                for line in diff.diff.lines().take(18) {
                                    let col = if line.starts_with('+') {
                                        theme::ACCENT_EMERALD
                                    } else if line.starts_with('-') {
                                        theme::ACCENT_RED
                                    } else {
                                        theme::TEXT_MUTED
                                    };
                                    ui.label(
                                        RichText::new(line)
                                            .size(10.5)
                                            .family(FontFamily::Monospace)
                                            .color(col),
                                    );
                                }
                            });
                    }
                });
        }
    }

    fn render_inspector_controls(&mut self, ui: &mut Ui) {
        ScrollArea::vertical()
            .id_salt("inspector_controls_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin::same(14))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("AGENT EXECUTION MODE")
                                .size(10.5)
                                .strong()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(8.0);
                        let cur_mode = self.hm.agent_mode();
                        ui.horizontal_wrapped(|ui| {
                            for (m_id, m_label, ic) in [
                                ("Autonomous", "Auto", Icon::Zap),
                                ("Plan", "Plan", Icon::FileText),
                                ("Supervised", "Supervised", Icon::Shield),
                                ("Duel", "Duel", Icon::Swords),
                            ] {
                                if mode_segment_pill(
                                    ui,
                                    ic,
                                    m_label,
                                    cur_mode.eq_ignore_ascii_case(m_id),
                                )
                                .clicked()
                                {
                                    self.hm.set_agent_mode(m_id);
                                }
                            }
                        });

                        ui.add_space(18.0);
                        ui.label(
                            RichText::new("SANDBOXED TOOLS")
                                .size(10.5)
                                .strong()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(8.0);

                        for (label, flag) in [
                            ("read_file", &mut self.tool_enabled_read),
                            ("write_file", &mut self.tool_enabled_write),
                            ("edit_file", &mut self.tool_enabled_edit),
                            ("run_command", &mut self.tool_enabled_cmd),
                            ("grep_search", &mut self.tool_enabled_grep),
                            ("web_search", &mut self.tool_enabled_web),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(label)
                                        .size(12.0)
                                        .family(FontFamily::Monospace)
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        pill_switch(ui, flag);
                                    },
                                );
                            });
                            ui.add_space(4.0);
                        }

                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("FRONTIER & AST TOOLS")
                                .size(10.5)
                                .strong()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(8.0);
                        for tool_name in [
                            "todo_write",
                            "apply_patch",
                            "lsp_diagnostics",
                            "ast_edit_symbol",
                            "ast_rename_symbol",
                            "git_archeology",
                            "debug_trace",
                            "synthesize_tool",
                            "shadow_sandbox",
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(tool_name)
                                        .size(11.5)
                                        .family(FontFamily::Monospace)
                                        .color(theme::TEXT_SECONDARY),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new("Active")
                                                .size(10.5)
                                                .color(theme::ACCENT_EMERALD),
                                        );
                                    },
                                );
                            });
                            ui.add_space(3.0);
                        }
                    });
            });
    }

    /// Renders the Center Chat Workbench matching `media_1794094798213_77679872.jpg` 1:1.
    fn render_chat_workbench(&mut self, ui: &mut Ui) {
        let conv = self.hm.active_conversation();
        let cur_mode = self.hm.agent_mode();
        let ws_path_str = self
            .hm
            .active_workspace_path()
            .to_string_lossy()
            .to_string();

        // 1. Sub-Header Bar (`52px` height)
        egui::Frame::new()
            .fill(theme::BG_MAIN)
            .inner_margin(Margin::symmetric(18, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            // Sky-blue vertical bar `|`
                            let (bar_r, _) =
                                ui.allocate_exact_size(egui::vec2(3.0, 14.0), Sense::hover());
                            ui.painter().rect_filled(
                                bar_r,
                                CornerRadius::same(2),
                                theme::ACCENT_SKY,
                            );

                            if self.renaming_conv_id.as_deref() == Some(&conv.id) {
                                let resp = ui.add_sized(
                                    egui::vec2(180.0, 22.0),
                                    TextEdit::singleline(&mut self.rename_conv_buf),
                                );
                                if resp.lost_focus()
                                    || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    self.hm
                                        .rename_conversation(&conv.id, &self.rename_conv_buf);
                                    self.renaming_conv_id = None;
                                }
                            } else {
                                ui.label(
                                    RichText::new(&conv.title)
                                        .size(14.0)
                                        .strong()
                                        .color(theme::TEXT_WHITE),
                                );
                                let (ed_r, ed_resp) = ui
                                    .allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
                                if ed_resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                paint_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        ed_r.center(),
                                        egui::vec2(11.0, 11.0),
                                    ),
                                    Icon::Edit3,
                                    if ed_resp.hovered() {
                                        theme::TEXT_WHITE
                                    } else {
                                        theme::TEXT_DIM
                                    },
                                );
                                if ed_resp.clicked() {
                                    self.renaming_conv_id = Some(conv.id.clone());
                                    self.rename_conv_buf = conv.title.clone();
                                }
                            }

                            // Mode capsule pill (`[zap Plan Mode]`)
                            let mode_pill_txt = match cur_mode.as_str() {
                                "Plan" => "Plan Mode",
                                "Supervised" => "Supervised Mode",
                                "Duel" => "Security Duel",
                                _ => "Auto Mode",
                            };
                            egui::Frame::new()
                                .fill(Color32::from_rgb(20, 20, 24))
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(5))
                                .inner_margin(Margin::symmetric(7, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        let (ic_r, _) = ui.allocate_exact_size(
                                            egui::vec2(10.0, 10.0),
                                            Sense::hover(),
                                        );
                                        paint_icon(
                                            ui.painter(),
                                            ic_r,
                                            Icon::FileText,
                                            theme::TEXT_SECONDARY,
                                        );
                                        ui.label(
                                            RichText::new(mode_pill_txt)
                                                .size(10.5)
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                    });
                                });
                        });

                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(format!("Working in: {ws_path_str}"))
                                .size(11.0)
                                .family(FontFamily::Monospace)
                                .color(theme::TEXT_DIM),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if icon_btn(
                            ui,
                            Icon::RotateCcw,
                            false,
                            "Clear Active Conversation",
                        )
                        .clicked()
                        {
                            self.hm.clear_active_conversation();
                        }
                        if dark_action_btn(ui, Some(Icon::Plus), "New Chat", false).clicked() {
                            self.hm.new_conversation(Some("New Chat"));
                        }
                    });
                });
            });

        let (sep_sub, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().line_segment(
            [sep_sub.left_center(), sep_sub.right_center()],
            Stroke::new(1.0, theme::BORDER_SUBTLE),
        );

        // 2. Scrollable Centered Message Feed (`max_width: 780px`, fills all space above the bottom composer dock)
        ScrollArea::vertical()
            .id_salt("chat_messages_feed")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(18.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(780.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        let live = self.hm.live_turn_snapshot();
                        if conv.messages.is_empty() && !live.running {
                            self.render_empty_chat_welcome(ui);
                        } else {
                            for (idx, msg) in conv.messages.iter().enumerate() {
                                if msg.role == "user" {
                                    self.render_user_message_bubble(ui, msg);
                                } else {
                                    self.render_agent_message_card(ui, idx, &conv.id, msg);
                                }
                                ui.add_space(16.0);
                            }

                            if live.running && live.conv_id == conv.id {
                                self.render_live_streaming_card(ui, &live);
                                ui.add_space(16.0);
                            }
                        }
                    });
                });
                ui.add_space(12.0);
            });
    }

    fn render_empty_chat_welcome(&mut self, ui: &mut Ui) {
        ui.add_space(60.0);
        egui::Frame::new()
            .fill(theme::BG_AGENT_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_CARD))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(24))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (ic_r, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                    paint_icon(ui.painter(), ic_r, Icon::Bot, theme::ACCENT_EMERALD);
                    ui.label(
                        RichText::new("Unjess Agent Ready")
                            .size(15.0)
                            .strong()
                            .color(theme::TEXT_WHITE),
                    );
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Select a mode below (Auto, Plan, Supervised, or Duel) and describe a feature, bug, or security audit task.")
                        .size(13.0)
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    for prompt in [
                        "Inspect this workspace architecture and summarize key modules",
                        "Run a Red Team vs. White Team security audit on this repository",
                        "Find performance bottlenecks and propose clean refactors",
                    ] {
                        if dark_action_btn(ui, Some(Icon::Sparkles), prompt, false).clicked() {
                            self.hm.submit_prompt(prompt.to_string());
                        }
                    }
                });
            });
    }

    fn render_user_message_bubble(&self, ui: &mut Ui, msg: &ChatMessage) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            egui::Frame::new()
                .fill(theme::BG_USER_BUBBLE)
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_max_width(540.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&msg.content)
                                    .size(13.5)
                                    .color(theme::TEXT_PRIMARY),
                            )
                            .wrap(),
                        );
                    });
                });
        });
    }

    fn render_agent_message_card(
        &mut self,
        ui: &mut Ui,
        msg_idx: usize,
        conv_id: &str,
        msg: &ChatMessage,
    ) {
        let card_w = ui.available_width().min(780.0);
        egui::Frame::new()
            .fill(theme::BG_AGENT_CARD)
            .stroke(Stroke::new(1.0, theme::BORDER_CARD))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                ui.set_width(card_w);
                ui.set_max_width(card_w);

                // Card Header Row (`[bot] Unjess Agent [kimi-k3.1-pro:cloud] ... [Undo] 22:44:52`)
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.set_width(card_w - 32.0);
                        ui.set_max_width(card_w - 32.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            // Bot icon badge
                            let (bot_box, _) =
                                ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                            ui.painter().rect(
                                bot_box,
                                CornerRadius::same(6),
                                Color32::from_rgb(22, 22, 26),
                                Stroke::new(1.0, theme::BORDER_SUBTLE),
                                StrokeKind::Inside,
                            );
                            paint_icon(
                                ui.painter(),
                                egui::Rect::from_center_size(
                                    bot_box.center(),
                                    egui::vec2(13.0, 13.0),
                                ),
                                Icon::Bot,
                                theme::TEXT_SECONDARY,
                            );

                            ui.label(
                                RichText::new("Unjess Agent")
                                    .size(12.5)
                                    .strong()
                                    .color(theme::TEXT_WHITE),
                            );

                            let model_name = msg
                                .model
                                .as_deref()
                                .unwrap_or(&self.settings_draft.model);
                            egui::Frame::new()
                                .fill(Color32::from_rgb(24, 24, 28))
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(model_name)
                                            .size(11.0)
                                            .family(FontFamily::Monospace)
                                            .color(theme::TEXT_SECONDARY),
                                    );
                                });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    if !msg.timestamp.is_empty() {
                                        ui.label(
                                            RichText::new(&msg.timestamp)
                                                .size(11.0)
                                                .family(FontFamily::Monospace)
                                                .color(theme::TEXT_DIM),
                                        );
                                    }

                                    // `[Undo]` button
                                    let (u_rect, u_resp) = ui.allocate_exact_size(
                                        egui::vec2(54.0, 22.0),
                                        Sense::click(),
                                    );
                                    if u_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.painter().rect(
                                        u_rect,
                                        CornerRadius::same(5),
                                        if u_resp.hovered() {
                                            theme::BG_CARD_HOVER
                                        } else {
                                            Color32::from_rgb(20, 20, 24)
                                        },
                                        Stroke::new(1.0, theme::BORDER_SUBTLE),
                                        StrokeKind::Inside,
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        egui::Rect::from_center_size(
                                            egui::pos2(u_rect.left() + 12.0, u_rect.center().y),
                                            egui::vec2(10.0, 10.0),
                                        ),
                                        Icon::RotateCcw,
                                        theme::TEXT_SECONDARY,
                                    );
                                    ui.painter().text(
                                        egui::pos2(u_rect.left() + 21.0, u_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        "Undo",
                                        FontId::new(11.0, FontFamily::Proportional),
                                        theme::TEXT_SECONDARY,
                                    );
                                    if u_resp.clicked() {
                                        self.hm.undo_last_turn(conv_id);
                                    }
                                },
                            );
                        });
                    });

                let (sep_hdr, _) = ui
                    .allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().line_segment(
                    [sep_hdr.left_center(), sep_hdr.right_center()],
                    Stroke::new(1.0, theme::BORDER_SUBTLE),
                );

                // Card Body
                let body_w = (card_w - 36.0).max(200.0);
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(18, 14))
                    .show(ui, |ui| {
                        ui.set_width(body_w);
                        ui.set_max_width(body_w);
                        // Collapsible `● Agent Activity • Ran N commands   Show details v` box
                        if !msg.tool_calls.is_empty()
                            || msg.thinking.as_ref().is_some_and(|t| !t.trim().is_empty())
                        {
                            let is_expanded = self.expanded_activity_msgs.contains(&msg_idx);
                            let act_w = (body_w - 24.0).max(180.0);
                            egui::Frame::new()
                                .fill(theme::BG_ACTIVITY)
                                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_width(act_w);
                                    ui.set_max_width(act_w);
                                    let (act_rect, act_resp) = ui.allocate_exact_size(
                                        egui::vec2(act_w, 20.0),
                                        Sense::click(),
                                    );
                                    if act_resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.painter().circle_filled(
                                        egui::pos2(act_rect.left() + 5.0, act_rect.center().y),
                                        3.5,
                                        theme::ACCENT_EMERALD,
                                    );
                                    ui.painter().text(
                                        egui::pos2(act_rect.left() + 16.0, act_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        "Agent Activity",
                                        FontId::new(12.0, FontFamily::Proportional),
                                        theme::TEXT_WHITE,
                                    );
                                    let summary_txt = format!(
                                        "•  Ran {} command{}",
                                        msg.tool_calls.len().max(1),
                                        if msg.tool_calls.len() == 1 { "" } else { "s" }
                                    );
                                    ui.painter().text(
                                        egui::pos2(act_rect.left() + 104.0, act_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        summary_txt,
                                        FontId::new(11.5, FontFamily::Monospace),
                                        theme::TEXT_MUTED,
                                    );

                                    let toggle_txt = if is_expanded {
                                        "Hide details"
                                    } else {
                                        "Show details"
                                    };
                                    ui.painter().text(
                                        egui::pos2(act_rect.right() - 16.0, act_rect.center().y),
                                        egui::Align2::RIGHT_CENTER,
                                        toggle_txt,
                                        FontId::new(11.0, FontFamily::Proportional),
                                        theme::TEXT_MUTED,
                                    );
                                    paint_icon(
                                        ui.painter(),
                                        egui::Rect::from_center_size(
                                            egui::pos2(
                                                act_rect.right() - 6.0,
                                                act_rect.center().y,
                                            ),
                                            egui::vec2(10.0, 10.0),
                                        ),
                                        if is_expanded {
                                            Icon::ChevronUp
                                        } else {
                                            Icon::ChevronDown
                                        },
                                        theme::TEXT_MUTED,
                                    );

                                    if act_resp.clicked() {
                                        if is_expanded {
                                            self.expanded_activity_msgs.remove(&msg_idx);
                                        } else {
                                            self.expanded_activity_msgs.insert(msg_idx);
                                        }
                                    }

                                    if is_expanded {
                                        ui.add_space(10.0);
                                        if let Some(ref thought) = msg.thinking {
                                            let clean_thought = thought.trim();
                                            if !clean_thought.is_empty() {
                                                egui::Frame::new()
                                                    .fill(Color32::from_rgb(10, 12, 16))
                                                    .stroke(Stroke::new(
                                                        1.0,
                                                        Color32::from_rgb(28, 34, 44),
                                                    ))
                                                    .corner_radius(CornerRadius::same(6))
                                                    .inner_margin(Margin::symmetric(10, 8))
                                                    .show(ui, |ui| {
                                                        ui.set_width(ui.available_width());
                                                        ui.label(
                                                            RichText::new("Reasoning / Step Plan:")
                                                                .size(10.5)
                                                                .strong()
                                                                .color(theme::ACCENT_SKY),
                                                        );
                                                        ui.add_space(2.0);
                                                        let preview: String = clean_thought
                                                            .chars()
                                                            .take(800)
                                                            .collect();
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(preview)
                                                                    .size(11.5)
                                                                    .color(theme::TEXT_SECONDARY),
                                                            )
                                                            .wrap(),
                                                        );
                                                    });
                                                ui.add_space(8.0);
                                            }
                                        }

                                        let step_w = (act_w - 20.0).max(160.0);
                                        for (step_i, tc) in msg.tool_calls.iter().enumerate() {
                                            egui::Frame::new()
                                                .fill(Color32::from_rgb(10, 10, 12))
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    Color32::from_rgb(26, 26, 30),
                                                ))
                                                .corner_radius(CornerRadius::same(6))
                                                .inner_margin(Margin::symmetric(10, 8))
                                                .show(ui, |ui| {
                                                    ui.set_width(step_w);
                                                    ui.set_max_width(step_w);
                                                    ui.horizontal(|ui| {
                                                        let status_ok = tc.status != "error";
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "Step #{}  {}",
                                                                step_i + 1,
                                                                if status_ok { "✓" } else { "✗" }
                                                            ))
                                                            .size(10.5)
                                                            .strong()
                                                            .family(FontFamily::Monospace)
                                                            .color(if status_ok {
                                                                theme::ACCENT_EMERALD
                                                            } else {
                                                                theme::ACCENT_RED
                                                            }),
                                                        );
                                                        ui.label(
                                                            RichText::new(&tc.name)
                                                                .size(11.5)
                                                                .strong()
                                                                .family(FontFamily::Monospace)
                                                                .color(theme::TEXT_WHITE),
                                                        );
                                                        // Keep argument preview compact (max 46 chars) so it never overlaps the right-aligned duration or stretches the step card!
                                                        let raw_args =
                                                            tc.arguments.to_string().replace('\n', " ");
                                                        let args_short =
                                                            if raw_args.chars().count() > 48 {
                                                                format!(
                                                                    "{}...",
                                                                    raw_args
                                                                        .chars()
                                                                        .take(45)
                                                                        .collect::<String>()
                                                                )
                                                            } else {
                                                                raw_args
                                                            };
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(args_short)
                                                                    .size(11.0)
                                                                    .family(FontFamily::Monospace)
                                                                    .color(theme::TEXT_MUTED),
                                                            )
                                                            .truncate(),
                                                        );
                                                        if tc.duration_ms > 0 {
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.label(
                                                                        RichText::new(format!(
                                                                            "{} ms",
                                                                            tc.duration_ms
                                                                        ))
                                                                        .size(10.0)
                                                                        .family(
                                                                            FontFamily::Monospace,
                                                                        )
                                                                        .color(theme::TEXT_DIM),
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    });

                                                    let res_trim = tc.result.trim();
                                                    if !res_trim.is_empty() {
                                                        ui.add_space(4.0);
                                                        let preview_lines: Vec<&str> =
                                                            res_trim.lines().take(6).collect();
                                                        let mut preview =
                                                            preview_lines.join("\n");
                                                        if preview.chars().count() > 380 {
                                                            preview = preview
                                                                .chars()
                                                                .take(380)
                                                                .collect::<String>()
                                                                + "...";
                                                        } else if res_trim.lines().count() > 6 {
                                                            preview.push_str("\n...");
                                                        }
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(preview)
                                                                    .size(10.5)
                                                                    .family(FontFamily::Monospace)
                                                                    .color(theme::TEXT_SECONDARY),
                                                            )
                                                            .wrap(),
                                                        );
                                                    }
                                                });
                                            ui.add_space(6.0);
                                        }
                                    }
                                });
                            ui.add_space(12.0);
                        }

                        // Rich Markdown Body
                        render_markdown(ui, &msg.content);
                    });
            });
    }

    fn render_live_streaming_card(&mut self, ui: &mut Ui, live: &unjess_core::LiveTurnSnapshot) {
        egui::Frame::new()
            .fill(theme::BG_AGENT_CARD)
            .stroke(Stroke::new(1.0, theme::ACCENT_EMERALD))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (dr, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
                    ui.painter()
                        .circle_filled(dr.center(), 4.0, theme::ACCENT_EMERALD);
                    ui.label(
                        RichText::new(format!(
                            "Unjess Agent — {} ({:.1}s)",
                            if live.current_step.is_empty() {
                                "Thinking & Executing..."
                            } else {
                                &live.current_step
                            },
                            live.elapsed_s
                        ))
                        .size(12.5)
                        .strong()
                        .color(theme::TEXT_WHITE),
                    );
                });

                ui.add_space(10.0);

                // Live collapsible `● Agent Activity` step-by-step inspector (hidden by default, toggleable while working!)
                let is_expanded = self.show_live_activity_details;
                egui::Frame::new()
                    .fill(theme::BG_ACTIVITY)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let (act_rect, act_resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 20.0),
                            Sense::click(),
                        );
                        if act_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        ui.painter().circle_filled(
                            egui::pos2(act_rect.left() + 5.0, act_rect.center().y),
                            3.5,
                            theme::ACCENT_EMERALD,
                        );
                        ui.painter().text(
                            egui::pos2(act_rect.left() + 16.0, act_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Agent Activity (Live)",
                            FontId::new(12.0, FontFamily::Proportional),
                            theme::TEXT_WHITE,
                        );
                        let summary_txt = format!(
                            "•  {} command{} executed  •  {}",
                            live.tool_calls.len(),
                            if live.tool_calls.len() == 1 { "" } else { "s" },
                            if live.current_step.is_empty() {
                                "Working..."
                            } else {
                                &live.current_step
                            }
                        );
                        ui.painter().text(
                            egui::pos2(act_rect.left() + 140.0, act_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            summary_txt,
                            FontId::new(11.0, FontFamily::Monospace),
                            theme::TEXT_MUTED,
                        );

                        let toggle_txt = if is_expanded {
                            "Hide steps"
                        } else {
                            "Show steps"
                        };
                        ui.painter().text(
                            egui::pos2(act_rect.right() - 16.0, act_rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            toggle_txt,
                            FontId::new(11.0, FontFamily::Proportional),
                            theme::ACCENT_SKY,
                        );
                        paint_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(act_rect.right() - 6.0, act_rect.center().y),
                                egui::vec2(10.0, 10.0),
                            ),
                            if is_expanded {
                                Icon::ChevronUp
                            } else {
                                Icon::ChevronDown
                            },
                            theme::ACCENT_SKY,
                        );

                        if act_resp.clicked() {
                            self.show_live_activity_details = !self.show_live_activity_details;
                        }

                        if is_expanded {
                            ui.add_space(8.0);
                            if !live.thinking.trim().is_empty() {
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(10, 12, 16))
                                    .stroke(Stroke::new(1.0, Color32::from_rgb(28, 34, 44)))
                                    .corner_radius(CornerRadius::same(6))
                                    .inner_margin(Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(
                                            RichText::new("Live Thinking / Reasoning:")
                                                .size(10.5)
                                                .strong()
                                                .color(theme::ACCENT_SKY),
                                        );
                                        ui.add_space(2.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(live.thinking.trim())
                                                    .size(11.5)
                                                    .color(theme::TEXT_SECONDARY),
                                            )
                                            .wrap(),
                                        );
                                    });
                                ui.add_space(6.0);
                            }

                            if live.tool_calls.is_empty() {
                                ui.label(
                                    RichText::new(format!(
                                        "▸ Current step: {}",
                                        if live.current_step.is_empty() {
                                            "Planning next action..."
                                        } else {
                                            &live.current_step
                                        }
                                    ))
                                    .size(11.0)
                                    .family(FontFamily::Monospace)
                                    .color(theme::TEXT_SECONDARY),
                                );
                            } else {
                                for (step_i, tc) in live.tool_calls.iter().enumerate() {
                                    egui::Frame::new()
                                        .fill(Color32::from_rgb(10, 10, 12))
                                        .stroke(Stroke::new(1.0, Color32::from_rgb(26, 26, 30)))
                                        .corner_radius(CornerRadius::same(6))
                                        .inner_margin(Margin::symmetric(10, 8))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                let badge = match tc.status.as_str() {
                                                    "running" => "…",
                                                    "error" => "✗",
                                                    _ => "✓",
                                                };
                                                let badge_col = match tc.status.as_str() {
                                                    "running" => theme::ACCENT_AMBER,
                                                    "error" => theme::ACCENT_RED,
                                                    _ => theme::ACCENT_EMERALD,
                                                };
                                                ui.label(
                                                    RichText::new(format!(
                                                        "Step #{}  {}",
                                                        step_i + 1,
                                                        badge
                                                    ))
                                                    .size(10.5)
                                                    .strong()
                                                    .family(FontFamily::Monospace)
                                                    .color(badge_col),
                                                );
                                                ui.label(
                                                    RichText::new(&tc.name)
                                                        .size(11.5)
                                                        .strong()
                                                        .family(FontFamily::Monospace)
                                                        .color(theme::TEXT_WHITE),
                                                );
                                                let raw_args =
                                                    tc.arguments.to_string().replace('\n', " ");
                                                let args_short = if raw_args.chars().count() > 48 {
                                                    format!(
                                                        "{}...",
                                                        raw_args
                                                            .chars()
                                                            .take(45)
                                                            .collect::<String>()
                                                    )
                                                } else {
                                                    raw_args
                                                };
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(args_short)
                                                            .size(11.0)
                                                            .family(FontFamily::Monospace)
                                                            .color(theme::TEXT_MUTED),
                                                    )
                                                    .truncate(),
                                                );
                                            });
                                            let res_trim = tc.result.trim();
                                            if !res_trim.is_empty() {
                                                ui.add_space(4.0);
                                                let preview: String =
                                                    res_trim.chars().take(280).collect();
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(preview)
                                                            .size(10.5)
                                                            .family(FontFamily::Monospace)
                                                            .color(theme::TEXT_SECONDARY),
                                                    )
                                                    .wrap(),
                                                );
                                            }
                                        });
                                    ui.add_space(4.0);
                                }
                            }
                        }
                    });

                if !live.streaming_text.is_empty() {
                    ui.add_space(10.0);
                    render_markdown(ui, &live.streaming_text);
                }
            });
    }

    /// Renders the Bottom Floating Composer Dock matching `media_1794094798213_77679872.jpg` 1:1.
    fn render_bottom_composer_dock(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.set_max_width(780.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                // Row 1: Mode Segmented Bar on Left (`Auto | Plan | Supervised | Duel`) + Modifiers on Right (`>_ Caveman | Ponytail | Compact`)
                ui.horizontal(|ui| {
                    let cur_mode = self.hm.agent_mode();
                    egui::Frame::new()
                        .fill(Color32::from_rgb(12, 12, 14))
                        .stroke(Stroke::new(1.0, theme::BORDER_CARD))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(3))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if mode_segment_pill(
                                ui,
                                Icon::Zap,
                                "Auto",
                                cur_mode.eq_ignore_ascii_case("Autonomous")
                                    || cur_mode.eq_ignore_ascii_case("Auto"),
                            )
                            .clicked()
                            {
                                self.hm.set_agent_mode("Autonomous");
                            }
                            if mode_segment_pill(
                                ui,
                                Icon::FileText,
                                "Plan",
                                cur_mode.eq_ignore_ascii_case("Plan"),
                            )
                            .clicked()
                            {
                                self.hm.set_agent_mode("Plan");
                            }
                            if mode_segment_pill(
                                ui,
                                Icon::Shield,
                                "Supervised",
                                cur_mode.eq_ignore_ascii_case("Supervised"),
                            )
                            .clicked()
                            {
                                self.hm.set_agent_mode("Supervised");
                            }
                            if mode_segment_pill(
                                ui,
                                Icon::Swords,
                                "Duel",
                                cur_mode.eq_ignore_ascii_case("Duel"),
                            )
                            .clicked()
                            {
                                self.hm.set_agent_mode("Duel");
                            }
                        });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if modifier_pill_btn(ui, Icon::Minimize2, "Compact", false).clicked() {
                            self.hm.compact_active_conversation();
                            self.status_toast = Some((
                                "Compacted conversation context".to_string(),
                                Instant::now(),
                            ));
                        }
                        let ponytail = self.hm.ponytail_mode();
                        if modifier_pill_btn(ui, Icon::Sparkles, "Ponytail", ponytail).clicked() {
                            self.hm.set_ponytail_mode(!ponytail);
                        }
                        let caveman = self.hm.caveman_mode();
                        if modifier_pill_btn(ui, Icon::Terminal, "Caveman", caveman).clicked() {
                            self.hm.set_caveman_mode(!caveman);
                        }
                    });
                });

                ui.add_space(6.0);

                // Live Running Timers & Watchers Status Bar (if any active)
                let live_bg = SchedulerManager::list_live_tasks();
                let running_bg: Vec<_> = live_bg
                    .iter()
                    .filter(|t| t.status == "running")
                    .cloned()
                    .collect();
                if !running_bg.is_empty() {
                    ui.ctx().request_repaint_after(Duration::from_millis(500));
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(16, 185, 129, 14))
                        .stroke(Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(16, 185, 129, 75),
                        ))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (dr, _) =
                                    ui.allocate_exact_size(egui::vec2(7.0, 7.0), Sense::hover());
                                ui.painter()
                                    .circle_filled(dr.center(), 3.5, theme::ACCENT_EMERALD);
                                let summary: Vec<String> = running_bg
                                    .iter()
                                    .take(2)
                                    .map(|t| {
                                        if t.kind == "timer" {
                                            format!("⏰ {} ({}s left)", t.id, t.remaining_secs)
                                        } else {
                                            format!(
                                                "👁 {} ({}x, next poll {}s)",
                                                t.label, t.trigger_count, t.remaining_secs
                                            )
                                        }
                                    })
                                    .collect();
                                ui.label(
                                    RichText::new(summary.join("   •   "))
                                        .size(11.5)
                                        .family(FontFamily::Monospace)
                                        .color(theme::ACCENT_EMERALD),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if dark_action_btn(ui, Some(Icon::X), "Stop All", false)
                                            .clicked()
                                        {
                                            let n = SchedulerManager::cancel_live_task("all");
                                            self.status_toast = Some((
                                                format!("Stopped {} background task(s)", n),
                                                Instant::now(),
                                            ));
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }

                // Live Floating `/` Slash Command Palette (appears whenever prompt starts with `/`)
                let trimmed_input = self.prompt_input.trim_start().to_string();
                let show_slash_palette = trimmed_input.starts_with('/') && !trimmed_input.contains('\n');
                let mut slash_executed = false;
                if show_slash_palette {
                    let cmd_token = trimmed_input
                        .split_whitespace()
                        .next()
                        .unwrap_or("/")
                        .to_lowercase();
                    let all_cmds = Self::gui_slash_commands();
                    let filtered: Vec<_> = all_cmds
                        .into_iter()
                        .filter(|(cmd, _, _, _)| {
                            cmd_token == "/"
                                || cmd.starts_with(&cmd_token)
                                || cmd.contains(cmd_token.trim_start_matches('/'))
                        })
                        .collect();

                    if !filtered.is_empty() {
                        if self.slash_selected_idx >= filtered.len() {
                            self.slash_selected_idx = 0;
                        }
                        let nav_down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                        let nav_up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                        let tab_pressed = ui.input(|i| i.key_pressed(egui::Key::Tab));
                        if nav_down {
                            self.slash_selected_idx = (self.slash_selected_idx + 1) % filtered.len();
                        }
                        if nav_up {
                            self.slash_selected_idx = if self.slash_selected_idx == 0 {
                                filtered.len() - 1
                            } else {
                                self.slash_selected_idx - 1
                            };
                        }
                        if tab_pressed {
                            let (sel_cmd, _, _, takes_args) = filtered[self.slash_selected_idx];
                            self.prompt_input = if takes_args {
                                format!("{} ", sel_cmd)
                            } else {
                                sel_cmd.to_string()
                            };
                        }

                        egui::Frame::new()
                            .fill(Color32::from_rgb(15, 15, 19))
                            .stroke(Stroke::new(1.0, Color32::from_rgb(44, 46, 56)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("SLASH COMMANDS")
                                            .size(10.5)
                                            .strong()
                                            .family(FontFamily::Monospace)
                                            .color(theme::ACCENT_EMERALD),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(
                                                    "↑↓ Navigate  •  Tab Complete  •  Enter Run  •  Click to Execute",
                                                )
                                                .size(10.5)
                                                .color(theme::TEXT_DIM),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                ScrollArea::vertical()
                                    .id_salt("gui_slash_cmd_palette_scroll")
                                    .max_height(210.0)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        for (idx, (cmd, usage, desc, takes_args)) in
                                            filtered.iter().enumerate()
                                        {
                                            let is_sel = idx == self.slash_selected_idx;
                                            let row_w = ui.available_width();
                                            let (r_rect, r_resp) = ui.allocate_exact_size(
                                                egui::vec2(row_w, 28.0),
                                                Sense::click(),
                                            );
                                            if r_resp.hovered() {
                                                ui.ctx().set_cursor_icon(
                                                    egui::CursorIcon::PointingHand,
                                                );
                                                self.slash_selected_idx = idx;
                                            }
                                            if is_sel || r_resp.hovered() {
                                                ui.painter().rect(
                                                    r_rect,
                                                    CornerRadius::same(6),
                                                    Color32::from_rgb(26, 28, 36),
                                                    Stroke::new(
                                                        1.0,
                                                        Color32::from_rgba_unmultiplied(
                                                            16, 185, 129, 80,
                                                        ),
                                                    ),
                                                    StrokeKind::Inside,
                                                );
                                            }
                                            ui.painter().text(
                                                egui::pos2(
                                                    r_rect.left() + 10.0,
                                                    r_rect.center().y,
                                                ),
                                                egui::Align2::LEFT_CENTER,
                                                *usage,
                                                FontId::new(12.0, FontFamily::Monospace),
                                                if is_sel {
                                                    theme::ACCENT_EMERALD
                                                } else {
                                                    theme::TEXT_WHITE
                                                },
                                            );
                                            ui.painter().text(
                                                egui::pos2(
                                                    r_rect.left() + 235.0,
                                                    r_rect.center().y,
                                                ),
                                                egui::Align2::LEFT_CENTER,
                                                *desc,
                                                FontId::new(11.5, FontFamily::Proportional),
                                                theme::TEXT_SECONDARY,
                                            );
                                            if r_resp.clicked() {
                                                if *takes_args
                                                    && !trimmed_input.contains(' ')
                                                {
                                                    self.prompt_input = format!("{} ", cmd);
                                                } else {
                                                    let to_run = if *takes_args {
                                                        trimmed_input.to_string()
                                                    } else {
                                                        cmd.to_string()
                                                    };
                                                    self.prompt_input.clear();
                                                    self.dispatch_composer_input(to_run);
                                                    slash_executed = true;
                                                }
                                            }
                                        }
                                    });
                            });
                        ui.add_space(6.0);
                    }
                }

                // Row 2: Main Rounded Composer Input Box (`#111113` fill, `14px` corner radius)
                egui::Frame::new()
                    .fill(theme::BG_CARD)
                    .stroke(Stroke::new(1.0, Color32::from_rgb(34, 34, 38)))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let cur_mode = self.hm.agent_mode();
                        let placeholder = match cur_mode.as_str() {
                            "Plan" => "Plan Mode: Describe feature or goal (type / for commands, ! for shell)...",
                            "Supervised" => "Supervised Mode: Describe task (type / for commands, ! for shell)...",
                            "Duel" => "Security Duel Mode: Red Team vs. White Team audit (type / for commands)...",
                            _ => "Ask Unjess to architect, refactor, or audit... (Type / for commands, !cmd for shell, Enter to send)",
                        };

                        let enter_to_send = ui.input(|i| {
                            i.key_pressed(egui::Key::Enter) && !i.modifiers.shift
                        });

                        // Intercept Ctrl+V / Cmd+V / Shift+Insert paste for clipboard screenshot images
                        let paste_image_detected = ui.input(|i| {
                            ((i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::V))
                                || (i.modifiers.shift && i.key_pressed(egui::Key::Insert))
                                || i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                        });
                        if paste_image_detected {
                            let ws_path = self.hm.active_workspace_path();
                            if let Some(att) = Self::try_paste_clipboard_image(&ws_path) {
                                let toast = format!(
                                    "Attached screenshot `{}` ({}×{})",
                                    att.filename, att.width, att.height
                                );
                                self.attached_images.push(att);
                                self.status_toast = Some((toast, Instant::now()));
                                ui.input_mut(|i| {
                                    i.events.retain(|e| !matches!(e, egui::Event::Paste(_) | egui::Event::Key { key: egui::Key::V, .. }));
                                });
                            }
                        }

                        // Attached screenshots and images chips bar
                        if !self.attached_images.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                                let mut remove_idx = None;
                                for (idx, att) in self.attached_images.iter().enumerate() {
                                    egui::Frame::new()
                                        .fill(theme::BG_CARD_HOVER)
                                        .stroke(Stroke::new(1.0, theme::BORDER_CARD))
                                        .corner_radius(CornerRadius::same(6))
                                        .inner_margin(Margin::symmetric(8, 4))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new("📷").size(12.0));
                                                let label_text = if att.width > 0 && att.height > 0 {
                                                    format!("{} ({}×{})", att.filename, att.width, att.height)
                                                } else {
                                                    att.filename.clone()
                                                };
                                                ui.label(
                                                    RichText::new(label_text)
                                                        .size(11.5)
                                                        .color(theme::TEXT_PRIMARY),
                                                );
                                                if ui.add(egui::Button::new(RichText::new("✕").size(10.0)).frame(false)).clicked() {
                                                    remove_idx = Some(idx);
                                                }
                                            });
                                        });
                                }
                                if let Some(idx) = remove_idx {
                                    self.attached_images.remove(idx);
                                }
                            });
                            ui.add_space(4.0);
                        }

                        // Scrollable bounded multiline prompt input so typing long prompts expands UPWARD (up to 135px) while keeping the bottom toolbar pinned!
                        let mut submitted_via_enter = false;
                        ScrollArea::vertical()
                            .id_salt("composer_prompt_scroll")
                            .max_height(135.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let text_resp = ui.add_sized(
                                    egui::vec2(ui.available_width(), 42.0),
                                    TextEdit::multiline(&mut self.prompt_input)
                                        .frame(false)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(2)
                                        .hint_text(
                                            RichText::new(placeholder)
                                                .size(13.0)
                                                .color(theme::TEXT_DIM),
                                        ),
                                );
                                if enter_to_send && text_resp.has_focus() && !slash_executed {
                                    submitted_via_enter = true;
                                }
                            });

                        if submitted_via_enter {
                            self.submit_current_composer_prompt();
                        }

                        ui.add_space(6.0);

                        // Bottom Composer Toolbar Row
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if icon_btn(ui, Icon::Paperclip, false, "Attach File Path or Image").clicked() {
                                if let Some(path) = rfd::FileDialog::new().pick_file() {
                                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                                    if ["png", "jpg", "jpeg", "webp", "bmp", "ico"].contains(&ext.as_str()) {
                                        let (width, height) = image::image_dimensions(&path).unwrap_or((0, 0));
                                        let filename = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image.png".to_string());
                                        let toast = format!("Attached image `{}` ({}×{})", filename, width, height);
                                        self.attached_images.push(AttachedImage {
                                            path,
                                            filename,
                                            width,
                                            height,
                                        });
                                        self.status_toast = Some((toast, Instant::now()));
                                    } else {
                                        self.prompt_input
                                            .push_str(&format!(" `{}` ", path.to_string_lossy()));
                                    }
                                }
                            }
                            if icon_btn(ui, Icon::Image, false, "Paste Screenshot from Clipboard (Ctrl+V)").clicked() {
                                let ws_path = self.hm.active_workspace_path();
                                if let Some(att) = Self::try_paste_clipboard_image(&ws_path) {
                                    let toast = format!(
                                        "Attached screenshot `{}` ({}×{})",
                                        att.filename, att.width, att.height
                                    );
                                    self.attached_images.push(att);
                                    self.status_toast = Some((toast, Instant::now()));
                                } else {
                                    self.status_toast = Some((
                                        "Clipboard does not contain an image or screenshot".to_string(),
                                        Instant::now(),
                                    ));
                                }
                            }
                            if icon_btn(ui, Icon::FileCode, false, "Browse Workspace Files")
                                .clicked()
                            {
                                self.active_tab = MainTab::Files;
                            }
                            if icon_btn(ui, Icon::Globe, false, "Web Research & Search").clicked() {
                                self.active_tab = MainTab::Browser;
                            }
                            if icon_btn(
                                ui,
                                Icon::Terminal,
                                self.show_terminal_drawer,
                                "Toggle Sandboxed Terminal",
                            )
                            .clicked()
                            {
                                self.show_terminal_drawer = !self.show_terminal_drawer;
                            }

                            // Right: `[kimi-k3.1-pro:cloud HIGH v]` + Solid White `[Send]` Button
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let live = self.hm.live_turn_snapshot();

                                    // Solid White 28x28 Send Button (or Red Stop Button when running)
                                    let (send_rect, send_resp) = ui.allocate_exact_size(
                                        egui::vec2(28.0, 28.0),
                                        Sense::click(),
                                    );
                                    if send_resp.hovered() {
                                        ui.ctx()
                                            .set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if live.running {
                                        ui.painter().rect_filled(
                                            send_rect,
                                            CornerRadius::same(7),
                                            theme::ACCENT_RED,
                                        );
                                        paint_icon(
                                            ui.painter(),
                                            egui::Rect::from_center_size(
                                                send_rect.center(),
                                                egui::vec2(12.0, 12.0),
                                            ),
                                            Icon::Square,
                                            Color32::WHITE,
                                        );
                                        if send_resp.clicked() {
                                            self.hm.abort_turn();
                                        }
                                    } else {
                                        ui.painter().rect_filled(
                                            send_rect,
                                            CornerRadius::same(7),
                                            if send_resp.hovered() {
                                                Color32::WHITE
                                            } else {
                                                theme::ACCENT_WHITE
                                            },
                                        );
                                        paint_icon(
                                            ui.painter(),
                                            egui::Rect::from_center_size(
                                                send_rect.center(),
                                                egui::vec2(13.0, 13.0),
                                            ),
                                            Icon::Send,
                                            theme::TEXT_DARK_ON_WHITE,
                                        );
                                        if send_resp.clicked() {
                                            self.submit_current_composer_prompt();
                                        }
                                    }

                                    // Model + Effort Selector Pill (`kimi-k3.1-pro:cloud [HIGH] v`)
                                    let active_model = self.settings_draft.model.clone();
                                    let effort_up =
                                        self.settings_draft.reasoning_effort.to_uppercase();
                                    let pill_w = (active_model.len() as f32 * 6.8 + 76.0)
                                        .clamp(160.0, 290.0);
                                    let (m_rect, m_resp) = ui.allocate_exact_size(
                                        egui::vec2(pill_w, 26.0),
                                        Sense::click(),
                                    );
                                    if m_resp.hovered() {
                                        ui.ctx()
                                            .set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    ui.painter().rect(
                                        m_rect,
                                        CornerRadius::same(6),
                                        if m_resp.hovered() {
                                            Color32::from_rgb(28, 28, 33)
                                        } else {
                                            Color32::from_rgb(22, 22, 26)
                                        },
                                        Stroke::new(1.0, theme::BORDER_SUBTLE),
                                        StrokeKind::Inside,
                                    );

                                    // Model ID text
                                    ui.painter().text(
                                        egui::pos2(m_rect.left() + 10.0, m_rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        &active_model,
                                        FontId::new(11.5, FontFamily::Monospace),
                                        theme::TEXT_PRIMARY,
                                    );

                                    // Sky-blue `HIGH` effort badge inside pill
                                    let eff_rect = egui::Rect::from_center_size(
                                        egui::pos2(m_rect.right() - 36.0, m_rect.center().y),
                                        egui::vec2(34.0, 16.0),
                                    );
                                    ui.painter().rect_filled(
                                        eff_rect,
                                        CornerRadius::same(4),
                                        Color32::from_rgb(12, 45, 72),
                                    );
                                    ui.painter().text(
                                        eff_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        &effort_up,
                                        FontId::new(9.5, FontFamily::Monospace),
                                        theme::ACCENT_SKY,
                                    );

                                    // Chevron down
                                    paint_icon(
                                        ui.painter(),
                                        egui::Rect::from_center_size(
                                            egui::pos2(m_rect.right() - 10.0, m_rect.center().y),
                                            egui::vec2(10.0, 10.0),
                                        ),
                                        Icon::ChevronDown,
                                        theme::TEXT_MUTED,
                                    );

                                    let model_popup_id =
                                        ui.make_persistent_id("composer_model_popup");
                                    if m_resp.clicked() {
                                        ui.memory_mut(|mem| mem.toggle_popup(model_popup_id));
                                    }
                                    egui::popup::popup_above_or_below_widget(
                                        ui,
                                        model_popup_id,
                                        &m_resp,
                                        egui::AboveOrBelow::Above,
                                        egui::popup::PopupCloseBehavior::CloseOnClickOutside,
                                        |ui| {
                                            ui.set_min_width(250.0);
                                            ui.label(
                                                RichText::new("Reasoning Effort")
                                                    .size(11.0)
                                                    .strong()
                                                    .color(theme::TEXT_MUTED),
                                            );
                                            ui.horizontal(|ui| {
                                                for eff in ["low", "medium", "high", "max"] {
                                                    if ui
                                                        .selectable_label(
                                                            self.settings_draft.reasoning_effort
                                                                == eff,
                                                            eff.to_uppercase(),
                                                        )
                                                        .clicked()
                                                    {
                                                        self.settings_draft.reasoning_effort =
                                                            eff.to_string();
                                                        self.hm.update_settings(
                                                            self.settings_draft.clone(),
                                                        );
                                                    }
                                                }
                                            });
                                            ui.separator();
                                            ui.label(
                                                RichText::new("Select Active Model")
                                                    .size(11.0)
                                                    .strong()
                                                    .color(theme::TEXT_MUTED),
                                            );
                                            ScrollArea::vertical()
                                                .max_height(240.0)
                                                .show(ui, |ui| {
                                                    for m in &self.discovered_models {
                                                        if ui
                                                            .selectable_label(
                                                                self.settings_draft.model == m.id,
                                                                format!(
                                                                    "{} ({})",
                                                                    m.id, m.provider
                                                                ),
                                                            )
                                                            .clicked()
                                                        {
                                                            self.settings_draft.model =
                                                                m.id.clone();
                                                            self.hm.update_settings(
                                                                self.settings_draft.clone(),
                                                            );
                                                        }
                                                    }
                                                });
                                        },
                                    );
                                },
                            );
                        });
                    });
            });
        });
    }
}

impl eframe::App for UnjessDesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep UI responsive during live agent streaming or background probes/update checks
        let mcp_busy = self
            .mcp_probing_target
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .is_some();
        let upd_busy = self
            .update_checking
            .load(std::sync::atomic::Ordering::Relaxed);

        if self.hm.live_turn_snapshot().running || mcp_busy || upd_busy {
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if self.last_cache_refresh.elapsed() > Duration::from_secs(12) {
            self.refresh_workspace_cache();
        }

        // Top Global Header Bar
        self.render_top_header(ctx);

        // Left Navigation & Projects Sidebar
        self.render_left_sidebar(ctx);

        // Right Changes & Diffs / Controls Inspector
        self.render_right_inspector(ctx);

        // Bottom Pinned Composer Dock (expands UPWARD when typing multi-line prompts, keeping toolbar & Send button anchored at the bottom!)
        if self.active_tab == MainTab::Chat {
            egui::TopBottomPanel::bottom("chat_bottom_composer_dock")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(theme::BG_MAIN)
                        .inner_margin(Margin::symmetric(18, 10)),
                )
                .show(ctx, |ui| {
                    self.render_bottom_composer_dock(ui);
                });
        }

        // Center Main Content Panel
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_MAIN).inner_margin(Margin::same(0)))
            .show(ctx, |ui| match self.active_tab {
                MainTab::Chat => self.render_chat_workbench(ui),
                MainTab::Agents => self.render_agents_tab(ui),
                MainTab::Workflows => self.render_workflows_tab(ui),
                MainTab::Schedules => self.render_schedules_tab(ui),
                MainTab::Files => self.render_files_tab(ui),
                MainTab::Browser => self.render_browser_tab(ui),
                MainTab::Settings => self.render_settings_tab(ui),
            });

        // Floating Modals & Terminal Drawer
        self.render_modals(ctx);

        // Drain live timer/watcher notifications into the floating toast banner
        let bg_notes = SchedulerManager::drain_notifications();
        if let Some(last_note) = bg_notes.last() {
            self.status_toast = Some((last_note.clone(), Instant::now()));
        }

        // Floating Status Toast Notification (bottom-right)
        if let Some((msg, ts)) = self.status_toast.clone() {
            if ts.elapsed() < Duration::from_secs(5) {
                ctx.request_repaint_after(Duration::from_millis(200));
                let is_err = msg.contains("failed")
                    || msg.contains("Error")
                    || msg.contains("offline")
                    || msg.contains("Could not");
                let accent = if is_err {
                    theme::ACCENT_RED
                } else {
                    theme::ACCENT_EMERALD
                };
                egui::Area::new(egui::Id::new("global_status_toast_area"))
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -24.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(18, 20, 24))
                            .stroke(Stroke::new(1.2, accent))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(16, 11))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (dr, _) = ui.allocate_exact_size(
                                        egui::vec2(8.0, 8.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dr.center(), 4.0, accent);
                                    ui.label(
                                        RichText::new(&msg)
                                            .size(12.5)
                                            .strong()
                                            .color(theme::TEXT_WHITE),
                                    );
                                    ui.add_space(6.0);
                                    if icon_btn(ui, Icon::X, false, "Dismiss").clicked() {
                                        self.status_toast = None;
                                    }
                                });
                            });
                    });
            } else {
                self.status_toast = None;
            }
        }
    }
}

fn load_window_icon() -> Option<Arc<egui::IconData>> {
    let width = 32u32;
    let height = 32u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let in_border = x >= 2 && x < 30 && y >= 2 && y < 30;
            if !in_border {
                continue;
            }
            // Dark obsidian background (#0a0a0b)
            let mut r = 10u8;
            let mut g = 10u8;
            let mut b = 12u8;

            // Crisp emerald/sky "U" emblem
            let u_left = (8..=11).contains(&x) && (8..=21).contains(&y);
            let u_right = (20..=23).contains(&x) && (8..=21).contains(&y);
            let u_bottom = (8..=23).contains(&x) && (20..=23).contains(&y);
            if u_left || u_right || u_bottom {
                r = 52;
                g = 211;
                b = 153;
            }
            rgba[idx] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = 255;
        }
    }
    Some(Arc::new(egui::IconData {
        rgba,
        width,
        height,
    }))
}

fn main() -> eframe::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");
    let _rt_guard = rt.enter();

    let hm = Arc::new(HarnessManager::new());

    // Start background scheduler ticker
    SchedulerManager::start_background_ticker(Arc::clone(&hm));

    // Start embedded Axum REST API server on 127.0.0.1:9758 for CLI & external tool interoperability
    let api_hm = Arc::clone(&hm);
    rt.spawn(async move {
        let _ = start_api_server(api_hm, "127.0.0.1", 9758).await;
    });

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Unjess")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 620.0]);

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Unjess",
        native_options,
        Box::new(move |cc| Ok(Box::new(UnjessDesktopApp::new(cc, hm)))),
    )
}
