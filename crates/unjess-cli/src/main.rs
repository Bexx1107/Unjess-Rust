use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use unjess_core::agent::HarnessManager;
use unjess_core::codeintel::{find_callers, generate_repo_map, outline_file};
use unjess_core::config::Settings;
use unjess_core::llm::resolve_provider_for_model;
use unjess_core::mcp::McpManager;
use unjess_core::memory::MemoryStore;
use unjess_core::scheduler::SchedulerManager;
use unjess_core::security::scan_workspace;
use unjess_core::skills::SkillsManager;
use unjess_core::tools::ToolEngine;
use unjess_core::version::{builtin_changelog, CURRENT_VERSION};
use unjess_core::workspace::Workspace;

// ============================================================================
// ANSI / VT100 Theme Palette & Windows Console Initialization
// ============================================================================

const C_RESET: &str = "\x1b[0m";
const C_BOLD: &str = "\x1b[1m";
const C_DIM: &str = "\x1b[2m";
const C_ITALIC: &str = "\x1b[3m";

const C_GREEN: &str = "\x1b[38;5;48m";
const C_GREEN_STD: &str = "\x1b[32m";
const C_CYAN: &str = "\x1b[38;5;81m";
const C_CYAN_DIM: &str = "\x1b[36m";
const C_YELLOW: &str = "\x1b[38;5;220m";
const C_RED: &str = "\x1b[38;5;203m";
const C_MAGENTA: &str = "\x1b[38;5;177m";
const C_BLUE: &str = "\x1b[38;5;75m";
const C_WHITE: &str = "\x1b[97m";
const C_GRAY: &str = "\x1b[38;5;243m";
const C_BORDER: &str = "\x1b[38;5;240m";

/// Thinking display mode: 0 = compact (up to 4 lines), 1 = full, 2 = hidden
static THINKING_DISPLAY_MODE: AtomicU8 = AtomicU8::new(0);

#[cfg(windows)]
fn enable_windows_ansi_and_utf8() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut core::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut core::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut core::ffi::c_void, dwMode: u32) -> i32;
        fn SetConsoleOutputCP(wCodePageID: u32) -> i32;
        fn SetConsoleCP(wCodePageID: u32) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
    const CP_UTF8: u32 = 65001;

    unsafe {
        let _ = SetConsoleOutputCP(CP_UTF8);
        let _ = SetConsoleCP(CP_UTF8);
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(std_handle);
            if !handle.is_null() {
                let mut mode: u32 = 0;
                if GetConsoleMode(handle, &mut mode) != 0 {
                    let _ = SetConsoleMode(
                        handle,
                        mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                    );
                }
            }
        }
    }
}

#[cfg(not(windows))]
fn enable_windows_ansi_and_utf8() {}

// ============================================================================
// CLI Argument Definitions
// ============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "unjess",
    version = CURRENT_VERSION,
    about = "Unjess 4.0 — 100% Native Rust Autonomous AI Coding & Security CLI"
)]
struct CliArgs {
    /// Optional single-shot prompt to execute immediately
    #[arg(value_name = "PROMPT", long = "prompt")]
    prompt: Option<String>,

    /// Read prompt or additional context from one or more files (repeatable `-f`)
    #[arg(short = 'f', long = "file")]
    files: Vec<PathBuf>,

    /// Override active model ID (in-memory for this session)
    #[arg(short, long, global = true)]
    model: Option<String>,

    /// Override active provider ID (in-memory for this session)
    #[arg(short = 'p', long = "provider", global = true)]
    provider: Option<String>,

    /// Override reasoning effort (low, medium, high, max)
    #[arg(short = 'e', long = "effort", global = true)]
    effort: Option<String>,

    /// Override workspace directory
    #[arg(short, long, global = true)]
    workspace: Option<String>,

    /// Override agent mode (auto, plan, supervised, duel)
    #[arg(long, global = true)]
    mode: Option<String>,

    /// Auto-confirm all prompts and actions without prompting (drop-in compatibility)
    #[arg(short = 'y', long = "yes", global = true)]
    yes: bool,

    /// Enable ultra-dense Caveman persona mode
    #[arg(long, global = true)]
    caveman: bool,

    /// Enable high-energy Ponytail systems-architect persona mode
    #[arg(long, global = true)]
    ponytail: bool,

    /// Emit structured JSON output to stdout (for CI/CD and Unix pipelines)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<CliSubcommand>,
}

#[derive(Subcommand, Debug)]
enum CliSubcommand {
    /// Run system, configuration, provider, and workspace diagnostics
    Doctor,
    /// Discover and list all available local and cloud LLM models
    Models,
    /// Run Red Team vs. White Team Security Duel scanner on a workspace
    Duel {
        /// Optional path to scan (defaults to active workspace or current directory)
        path: Option<String>,
        /// Minimum security score (0-100) required to exit with code 0
        #[arg(long)]
        fail_under: Option<u8>,
    },
    /// Generate a structural AST repository map of the workspace
    Map {
        /// Maximum number of files to index
        #[arg(long, default_value_t = 40)]
        max_files: usize,
    },
    /// Extract AST symbol outline for a specific workspace file
    Outline {
        /// Relative path to the file in the workspace
        file: String,
    },
    /// Find all call sites of a symbol across the workspace
    Callers {
        /// Symbol name to search for
        symbol: String,
    },
    /// Locate a symbol definition and print its body span across the workspace
    Symbol {
        /// Symbol name to locate
        symbol: String,
    },
    /// Search workspace files with regex/substring grep
    Grep {
        /// Search query or regular expression
        query: String,
        /// Optional subdirectory or file path filter
        #[arg(long)]
        path: Option<String>,
        /// Optional glob include filter (e.g. `*.rs` or `*.{rs,toml}`)
        #[arg(long)]
        include: Option<String>,
        /// Number of surrounding context lines (`0..=5`)
        #[arg(short = 'C', long)]
        context: Option<usize>,
    },
    /// List installed Unjess Skill Plugins
    Skills,
    /// List and optionally probe configured MCP servers
    Mcp {
        /// Probe each server via JSON-RPC 2.0 `initialize` + `tools/list`
        #[arg(long)]
        probe: bool,
    },
    /// Inspect or search persistent cross-session memory entries
    Memory {
        /// Optional search query
        query: Option<String>,
    },
    /// Show version and changelog information
    Version,
}

#[tokio::main]
async fn main() {
    enable_windows_ansi_and_utf8();

    let args = CliArgs::parse();
    let hm = Arc::new(HarnessManager::new());

    let target_ws = args.workspace.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string())
    });
    let id = hm.add_workspace(&target_ws);
    hm.set_active_workspace(&id);

    // Apply CLI flag overrides in-memory without mutating persistent config.yaml on disk
    if args.model.is_some() || args.provider.is_some() || args.effort.is_some() || args.yes {
        let mut s = hm.settings();
        if let Some(m) = &args.model {
            s.model = m.clone();
        }
        if let Some(p) = &args.provider {
            s.provider = p.clone();
        }
        if let Some(eff) = &args.effort {
            s.reasoning_effort = eff.clone();
        }
        if args.yes {
            s.confirm_commands = false;
        }
        hm.update_settings_in_memory(s);
    }

    if let Some(mode) = &args.mode {
        hm.set_agent_mode(mode);
    }
    if args.caveman {
        hm.set_caveman_mode(true);
    }
    if args.ponytail {
        hm.set_ponytail_mode(true);
    }

    if let Some(cmd) = args.command {
        match cmd {
            CliSubcommand::Doctor => run_doctor(&hm, args.json),
            CliSubcommand::Models => run_models(&hm, args.json, None),
            CliSubcommand::Duel { path, fail_under } => {
                let target = path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| hm.active_workspace_path());
                run_duel(&target, args.json, fail_under);
            }
            CliSubcommand::Map { max_files } => {
                run_map(&hm.active_workspace_path(), max_files, args.json)
            }
            CliSubcommand::Outline { file } => {
                run_outline(&hm.active_workspace_path(), &file, args.json, true)
            }
            CliSubcommand::Callers { symbol } => {
                run_callers(&hm.active_workspace_path(), &symbol, args.json)
            }
            CliSubcommand::Symbol { symbol } => {
                run_symbol(&hm.active_workspace_path(), &symbol, args.json)
            }
            CliSubcommand::Grep {
                query,
                path,
                include,
                context,
            } => run_grep(
                &hm.active_workspace_path(),
                &query,
                path.as_deref(),
                include.as_deref(),
                context.unwrap_or(0),
                args.json,
            ),
            CliSubcommand::Skills => run_skills(&hm, args.json, None),
            CliSubcommand::Mcp { probe } => run_mcp(probe, args.json),
            CliSubcommand::Memory { query } => run_memory(query.as_deref(), args.json),
            CliSubcommand::Version => run_version(args.json),
        }
        return;
    }

    // Build prompt from piped stdin, repeatable `-f/--file`, and/or positional argument
    let mut combined_prompt = String::new();
    if !io::stdin().is_terminal() {
        let mut piped = String::new();
        if io::stdin().read_to_string(&mut piped).is_ok() && !piped.trim().is_empty() {
            let non_empty_lines: Vec<&str> = piped
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();
            if args.prompt.is_none()
                && args.files.is_empty()
                && !non_empty_lines.is_empty()
                && non_empty_lines.iter().all(|l| l.starts_with('/'))
            {
                for line in non_empty_lines {
                    if !handle_repl_line(&hm, line).await {
                        break;
                    }
                }
                return;
            }
            if args.prompt.is_some() || !args.files.is_empty() {
                combined_prompt.push_str("```stdin\n");
                combined_prompt.push_str(piped.trim());
                combined_prompt.push_str("\n```\n\n");
            } else {
                combined_prompt.push_str(piped.trim());
            }
        }
    }
    for file_path in &args.files {
        match fs::read_to_string(file_path) {
            Ok(content) => {
                combined_prompt.push_str(&format!(
                    "```file:{}\n{}\n```\n\n",
                    file_path.display(),
                    content.trim()
                ));
            }
            Err(e) => {
                eprintln!(
                    "{}[!] Error reading file '{}': {}{}",
                    C_RED,
                    file_path.display(),
                    e,
                    C_RESET
                );
                std::process::exit(1);
            }
        }
    }
    if let Some(p) = args.prompt {
        combined_prompt.push_str(&p);
    }

    if !combined_prompt.trim().is_empty() {
        execute_turn_and_wait(&hm, combined_prompt, args.json, true).await;
        return;
    }

    run_repl(hm).await;
}

// ============================================================================
// Terminal Visual Renderers (Banner, Markdown, Diffs, Context Bar)
// ============================================================================

fn mode_badge(mode: &str) -> String {
    match mode.to_lowercase().as_str() {
        "plan" => format!("{}{}📋 Plan{}", C_BOLD, C_CYAN, C_RESET),
        "supervised" => format!("{}{}🛡 Supervised{}", C_BOLD, C_YELLOW, C_RESET),
        "duel" => format!("{}{}⚔ Security Duel{}", C_BOLD, C_RED, C_RESET),
        _ => format!("{}{}⚡ Autonomous{}", C_BOLD, C_GREEN, C_RESET),
    }
}

fn print_banner(hm: &HarnessManager) {
    let settings = hm.settings();
    let provider = resolve_provider_for_model(&settings, &settings.model);
    let ws_path = hm.active_workspace_path();
    let git_info = Workspace::new(&ws_path)
        .map(|w| w.git_info())
        .unwrap_or_default();
    let conv = hm.active_conversation();

    println!();
    println!(
        "{}{}  ██╗   ██╗███╗   ██╗     ██╗███████╗███████╗███████╗{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!(
        "{}{}  ██║   ██║████╗  ██║     ██║██╔════╝██╔════╝██╔════╝{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!(
        "{}{}  ██║   ██║██╔██╗ ██║     ██║█████╗  ███████╗███████╗{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!(
        "{}{}  ██║   ██║██║╚██╗██║██   ██║██╔══╝  ╚════██║╚════██║{}",
        C_BOLD, C_CYAN, C_RESET
    );
    println!(
        "{}{}  ╚██████╔╝██║ ╚████║╚█████╔╝███████╗███████║███████║{}",
        C_BOLD, C_CYAN, C_RESET
    );
    println!(
        "{}{}   ╚═════╝ ╚═╝  ╚═══╝ ╚════╝ ╚══════╝╚══════╝╚══════╝{}  {}v{} — 100% Native Rust Harness{}",
        C_BOLD, C_CYAN, C_RESET, C_GRAY, CURRENT_VERSION, C_RESET
    );
    println!(
        "  {}╭──────────────────────────────────────────────────────────────────────────────╮{}",
        C_BORDER, C_RESET
    );

    let mut personas = Vec::new();
    if hm.caveman_mode() {
        personas.push(format!("{}[🪨 Caveman]{}", C_YELLOW, C_RESET));
    }
    if hm.ponytail_mode() {
        personas.push(format!("{}[⚡ Ponytail]{}", C_MAGENTA, C_RESET));
    }
    let persona_str = if personas.is_empty() {
        String::new()
    } else {
        format!("  {}", personas.join(" "))
    };

    println!(
        "  {}│{}  {}Model:{} {}{}{} {}({}){}   {}Mode:{} {}   {}Effort:{} {}{}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BOLD,
        C_WHITE,
        settings.model,
        C_CYAN_DIM,
        provider,
        C_RESET,
        C_GRAY,
        C_RESET,
        mode_badge(&hm.agent_mode()),
        C_GRAY,
        C_RESET,
        C_MAGENTA,
        settings.reasoning_effort,
        C_RESET,
        persona_str
    );

    let git_str = if git_info.branch.is_empty() {
        String::new()
    } else {
        let dirty_mark = if git_info.dirty { "*" } else { "" };
        format!(" {}({}{}){}", C_YELLOW, git_info.branch, dirty_mark, C_RESET)
    };

    println!(
        "  {}│{}  {}Workspace:{} {}{}{}{}   {}Chat:{} {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BLUE,
        ws_path.display(),
        C_RESET,
        git_str,
        C_GRAY,
        C_RESET,
        C_WHITE,
        conv.title,
        C_RESET
    );
    println!(
        "  {}│{}  {}Tip:{} Type {}/{} for all commands · {}/context{} for token & cost usage · {}/gui{} for desktop UI",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_GREEN,
        C_RESET,
        C_CYAN,
        C_RESET,
        C_MAGENTA,
        C_RESET
    );
    println!(
        "  {}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BORDER, C_RESET
    );
}

fn print_slash_command_palette() {
    println!();
    println!(
        "  {}{}╭─ Unjess {} Slash Command Palette ────────────────────────────────────────────╮{}",
        C_BOLD, C_GREEN, CURRENT_VERSION, C_RESET
    );

    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Session & Context",
            &[
                ("/context", "Show token usage bar, message breakdown, and session USD cost"),
                ("/status", "Display full session, model, workspace, and git status card"),
                ("/new [title]", "Start a fresh conversation in the active workspace"),
                ("/convs [idx|id]", "List conversations or switch to a conversation by index/ID"),
                ("/compact", "Summarize older turns to free up context window tokens"),
                ("/clear", "Clear all messages in the active conversation"),
                ("/budget [usd|off]", "View or configure session USD spending limit"),
            ],
        ),
        (
            "Models, Reasoning & Modes",
            &[
                ("/model [id]", "Show or switch active LLM model (auto-detects provider)"),
                ("/models [filter]", "Discover & list all available local and cloud models"),
                ("/provider [id]", "Show or switch active LLM provider"),
                ("/effort [level]", "Switch reasoning effort (low | medium | high | max)"),
                ("/mode [mode]", "Switch agent mode (auto | plan | supervised | duel)"),
                ("/caveman [on|off]", "Toggle ultra-dense Caveman persona mode"),
                ("/ponytail [on|off]", "Toggle high-energy Ponytail systems-architect mode"),
                ("/thinking [mode]", "Set reasoning display (compact | full | off)"),
            ],
        ),
        (
            "Workspace, Files, Shell & Watchers",
            &[
                ("/run <cmd>", "Execute a shell command directly in the workspace (alias: !<cmd>)"),
                ("/timer <sec> <task>", "Schedule a background countdown timer (e.g. /timer 10 !cargo check)"),
                ("/watch <path|!cmd>", "Start a live background file or command watcher (e.g. /watch src)"),
                ("/tasks", "List active background timers, file watchers & cron schedules"),
                ("/stop-tasks [id]", "Cancel running background timers & watchers"),
                ("/cd <path>", "Switch active workspace directory (alias: /workspace)"),
                ("/files [filter]", "List tracked files in the active workspace"),
                ("/git", "Show workspace git branch, working tree status, and recent commits"),
                ("/diff", "Render colorized unified diffs from the most recent turn"),
                ("/undo", "Revert file modifications from the last turn (alias: /revert)"),
            ],
        ),
        (
            "Code Intelligence & Security",
            &[
                ("/duel [path]", "Run Red Team vs. White Team Security Duel scanner"),
                ("/map", "Generate structural AST repository map of the workspace"),
                ("/outline <file>", "Extract AST symbol outline for a workspace file"),
                ("/callers <symbol>", "Find all call sites of a symbol across the workspace"),
                ("/symbol <symbol>", "Locate a symbol definition and print its body span"),
                ("/grep <query>", "Search workspace files with regex/substring grep"),
            ],
        ),
        (
            "Skills, MCP, Subagents & System",
            &[
                ("/skills [filter]", "List installed SKILL.md agent skills"),
                ("/mcp [probe]", "List configured MCP servers (or `/mcp probe` to live-test)"),
                ("/agents", "List autonomous subagents and their status"),
                ("/workflows", "List reusable multi-step prompt workflows"),
                ("/memory [query]", "Inspect or search persistent cross-session memory"),
                ("/doctor", "Run system, configuration, provider, and workspace diagnostics"),
                ("/gui", "Launch the native Rust GPU Desktop application (Unjess.exe)"),
                ("/version", "Show Unjess Rust version and latest changelog"),
                ("/exit", "Exit the CLI REPL (alias: /quit)"),
            ],
        ),
    ];

    for (title, cmds) in sections {
        println!(
            "  {}│{}  {}{}{}{}",
            C_BORDER, C_RESET, C_BOLD, C_YELLOW, title, C_RESET
        );
        for (cmd, desc) in *cmds {
            println!(
                "  {}│{}    {}{:<20}{} {}{}{}",
                C_BORDER, C_RESET, C_CYAN, cmd, C_RESET, C_GRAY, desc, C_RESET
            );
        }
        println!("  {}│{}", C_BORDER, C_RESET);
    }

    println!(
        "  {}{}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!();
}

fn print_context_card(hm: &HarnessManager) {
    let settings = hm.settings();
    let provider = resolve_provider_for_model(&settings, &settings.model);
    let conv = hm.active_conversation();

    let mut user_msgs = 0usize;
    let mut assistant_msgs = 0usize;
    let mut tool_msgs = 0usize;
    let mut total_tool_calls = 0usize;
    let mut total_diffs = 0usize;
    let mut char_estimate = 0usize;

    for m in &conv.messages {
        char_estimate += m.content.len();
        total_tool_calls += m.tool_calls.len();
        total_diffs += m.diffs.len();
        match m.role.as_str() {
            "user" => user_msgs += 1,
            "assistant" => assistant_msgs += 1,
            _ => tool_msgs += 1,
        }
    }

    let est_active_tokens = if conv.total_tokens > 0 {
        conv.total_tokens
    } else {
        char_estimate / 4
    };
    let context_limit = 128_000usize;
    let pct = ((est_active_tokens as f64 / context_limit as f64) * 100.0).min(100.0);
    let bar_width = 28usize;
    let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width.saturating_sub(filled);

    let bar_color = if pct > 80.0 {
        C_RED
    } else if pct > 50.0 {
        C_YELLOW
    } else {
        C_GREEN
    };
    let bar_str = format!(
        "{}{}{}{}{}",
        bar_color,
        "█".repeat(filled),
        C_BORDER,
        "░".repeat(empty),
        C_RESET
    );

    println!();
    println!(
        "  {}{}╭─ Session Context & Token Telemetry ──────────────────────────────────────────╮{}",
        C_BOLD, C_CYAN, C_RESET
    );
    println!(
        "  {}│{}  {}Session   :{} {}{}{} {}({}){}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BOLD,
        conv.title,
        C_RESET,
        C_GRAY,
        &conv.id[..conv.id.len().min(8)],
        C_RESET
    );
    println!(
        "  {}│{}  {}Model     :{} {}{}{} ({}) · Effort: {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_CYAN,
        settings.model,
        C_RESET,
        provider,
        C_MAGENTA,
        settings.reasoning_effort,
        C_RESET
    );
    println!(
        "  {}│{}  {}Context   :{} [{}] {}{:.1}%{} ({} / {} tokens)",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        bar_str,
        C_BOLD,
        pct,
        C_RESET,
        est_active_tokens,
        context_limit
    );
    println!(
        "  {}│{}  {}Tokens    :{} Input: {}{}{} · Output: {}{}{} · Total: {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_WHITE,
        conv.total_input_tokens,
        C_RESET,
        C_WHITE,
        conv.total_output_tokens,
        C_RESET,
        C_GREEN,
        conv.total_tokens,
        C_RESET
    );
    println!(
        "  {}│{}  {}Messages  :{} {} total ({} user, {} assistant, {} system/tool)",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        conv.messages.len(),
        user_msgs,
        assistant_msgs,
        tool_msgs
    );
    println!(
        "  {}│{}  {}Activity  :{} {} tool calls executed · {} file diffs recorded",
        C_BORDER, C_RESET, C_GRAY, C_RESET, total_tool_calls, total_diffs
    );
    let budget_str = if settings.max_cost_per_session > 0.0 {
        format!("${:.2} limit", settings.max_cost_per_session)
    } else {
        "unlimited".to_string()
    };
    println!(
        "  {}│{}  {}Cost (USD):{} {}${:.4}{} ({})",
        C_BORDER, C_RESET, C_GRAY, C_RESET, C_GREEN, conv.total_cost_usd, C_RESET, budget_str
    );
    println!(
        "  {}{}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BOLD, C_CYAN, C_RESET
    );
    println!();
}

fn print_status_card(hm: &HarnessManager) {
    let settings = hm.settings();
    let provider = resolve_provider_for_model(&settings, &settings.model);
    let ws_path = hm.active_workspace_path();
    let git_info = Workspace::new(&ws_path)
        .map(|w| w.git_info())
        .unwrap_or_default();
    let conv = hm.active_conversation();

    println!();
    println!(
        "  {}{}╭─ Unjess {} Session Status ───────────────────────────────────────────────────╮{}",
        C_BOLD, C_GREEN, CURRENT_VERSION, C_RESET
    );
    println!(
        "  {}│{}  {}Workspace :{} {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BLUE,
        ws_path.display(),
        C_RESET
    );
    println!(
        "  {}│{}  {}Git Branch:{} {}{}{} ({} changed files)",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_YELLOW,
        if git_info.branch.is_empty() {
            "(not a git repo)"
        } else {
            &git_info.branch
        },
        C_RESET,
        git_info.changed_files
    );
    println!(
        "  {}│{}  {}Model     :{} {}{}{} (provider: {}{}{})",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BOLD,
        settings.model,
        C_RESET,
        C_CYAN,
        provider,
        C_RESET
    );
    println!(
        "  {}│{}  {}Mode      :{} {} · Effort: {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        mode_badge(&hm.agent_mode()),
        C_MAGENTA,
        settings.reasoning_effort,
        C_RESET
    );
    println!(
        "  {}│{}  {}Personas  :{} Caveman={} · Ponytail={}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        if hm.caveman_mode() { "ON" } else { "off" },
        if hm.ponytail_mode() { "ON" } else { "off" }
    );
    println!(
        "  {}│{}  {}Session   :{} {} ({} msgs · {} tokens · ${:.4})",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        conv.title,
        conv.messages.len(),
        conv.total_tokens,
        conv.total_cost_usd
    );
    println!(
        "  {}{}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!();
}

fn render_inline_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 32);
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Inline code `...`
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                out.push_str(&format!("{}`{}`{}", C_CYAN, code, C_RESET));
                i += end + 2;
                continue;
            }
        }
        // Bold **...**
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = chars[i + 2..]
                .windows(2)
                .position(|w| w[0] == '*' && w[1] == '*')
            {
                let bold_txt: String = chars[i + 2..i + 2 + end].iter().collect();
                out.push_str(&format!("{}{}{}{}", C_BOLD, C_WHITE, bold_txt, C_RESET));
                i += end + 4;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn render_terminal_markdown(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut code_lang = String::new();

    for raw_line in md.lines() {
        let trimmed = raw_line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if !in_code {
                in_code = true;
                code_lang = rest.trim().to_string();
                let label = if code_lang.is_empty() {
                    "code".to_string()
                } else {
                    code_lang.clone()
                };
                out.push_str(&format!(
                    "  {}╭── [{}] ──────────────────────────────────────────{}\n",
                    C_BORDER, label, C_RESET
                ));
            } else {
                in_code = false;
                code_lang.clear();
                out.push_str(&format!(
                    "  {}╰────────────────────────────────────────────────────{}\n",
                    C_BORDER, C_RESET
                ));
            }
            continue;
        }

        if in_code {
            out.push_str(&format!(
                "  {}│{} {}{}{}\n",
                C_BORDER, C_RESET, C_CYAN, raw_line, C_RESET
            ));
            continue;
        }

        if let Some(h) = trimmed.strip_prefix("### ") {
            out.push_str(&format!(
                "\n  {}{}▍ {}{}\n",
                C_BOLD, C_CYAN, h, C_RESET
            ));
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            out.push_str(&format!(
                "\n  {}{}◆ {}{}\n",
                C_BOLD, C_GREEN, h, C_RESET
            ));
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            out.push_str(&format!(
                "\n  {}{}━━ {} ━━{}\n",
                C_BOLD, C_GREEN, h, C_RESET
            ));
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            out.push_str(&format!(
                "  {}┃{} {}{}{}\n",
                C_MAGENTA,
                C_RESET,
                C_ITALIC,
                render_inline_markdown(quote),
                C_RESET
            ));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let indent = raw_line.len().saturating_sub(raw_line.trim_start().len());
            let pad = " ".repeat(indent);
            out.push_str(&format!(
                "  {}{}•{} {}\n",
                pad,
                C_GREEN,
                C_RESET,
                render_inline_markdown(item)
            ));
        } else {
            out.push_str(&format!("  {}\n", render_inline_markdown(raw_line)));
        }
    }

    if in_code {
        out.push_str(&format!(
            "  {}╰────────────────────────────────────────────────────{}\n",
            C_BORDER, C_RESET
        ));
    }

    out
}

fn render_colorized_diff(diff: &str) -> String {
    let mut out = String::new();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            out.push_str(&format!("  {}{}{}\n", C_BOLD, line, C_RESET));
        } else if line.starts_with("@@") {
            out.push_str(&format!("  {}{}{}\n", C_CYAN, line, C_RESET));
        } else if line.starts_with('+') {
            out.push_str(&format!("  {}{}{}\n", C_GREEN_STD, line, C_RESET));
        } else if line.starts_with('-') {
            out.push_str(&format!("  {}{}{}\n", C_RED, line, C_RESET));
        } else {
            out.push_str(&format!("  {}{}{}\n", C_GRAY, line, C_RESET));
        }
    }
    out
}

fn print_thinking_block(thinking: &str) {
    let mode = THINKING_DISPLAY_MODE.load(Ordering::Relaxed);
    if mode == 2 {
        return;
    }
    let clean = thinking.trim();
    if clean.is_empty() {
        return;
    }
    let lines: Vec<&str> = clean.lines().collect();
    let max_lines = if mode == 0 { 4 } else { lines.len() };
    let shown = lines.len().min(max_lines);
    let omitted = lines.len().saturating_sub(shown);

    eprintln!(
        "\n  {}╭─ 💭 Reasoning ({} line{} · `/thinking full|off` to configure) ─────────{}",
        C_BORDER,
        lines.len(),
        if lines.len() == 1 { "" } else { "s" },
        C_RESET
    );
    for line in lines.iter().take(shown) {
        let clipped: String = line.chars().take(105).collect();
        let suffix = if line.chars().count() > 105 { "…" } else { "" };
        eprintln!(
            "  {}│{} {}{}{}{}",
            C_BORDER, C_RESET, C_DIM, C_ITALIC, clipped, suffix
        );
    }
    if omitted > 0 {
        eprintln!(
            "  {}│{} {}… (+{} more reasoning lines — type `/thinking full` to expand){}",
            C_BORDER, C_RESET, C_GRAY, omitted, C_RESET
        );
    }
    eprintln!(
        "  {}╰─────────────────────────────────────────────────────────────────────────{}",
        C_BORDER, C_RESET
    );
}

// ============================================================================
// Subcommand Implementations
// ============================================================================

fn run_doctor(hm: &HarnessManager, json_out: bool) {
    let settings = hm.settings();
    let ws_path = hm.active_workspace_path();
    let git_info = Workspace::new(&ws_path)
        .map(|w| w.git_info())
        .unwrap_or_default();
    let mcp_servers = McpManager::load();
    let skills = SkillsManager::list_skills_for_workspace(Some(&ws_path));
    let memories = MemoryStore::load();
    let configured_keys: Vec<String> = settings
        .api_keys
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, _)| k.clone())
        .collect();

    if json_out {
        let out = serde_json::json!({
            "version": CURRENT_VERSION,
            "config_dir": Settings::config_dir(),
            "workspace": ws_path,
            "git": git_info,
            "provider": settings.provider,
            "model": settings.model,
            "reasoning_effort": settings.reasoning_effort,
            "agent_mode": hm.agent_mode(),
            "configured_keys": configured_keys,
            "mcp_servers_count": mcp_servers.len(),
            "skills_count": skills.len(),
            "memories_count": memories.len(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return;
    }

    println!();
    println!(
        "  {}{}╭─ Unjess {} — Native Rust System Doctor ───────────────────────────────────╮{}",
        C_BOLD, C_GREEN, CURRENT_VERSION, C_RESET
    );
    println!(
        "  {}│{}  {}Config Dir      :{} {}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        Settings::config_dir().display()
    );
    println!(
        "  {}│{}  {}Workspace       :{} {}{}{}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_BLUE,
        ws_path.display(),
        C_RESET
    );
    println!(
        "  {}│{}  {}Git Status      :{} branch={}{}{} dirty={} changed={}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        C_YELLOW,
        if git_info.branch.is_empty() {
            "(none)"
        } else {
            &git_info.branch
        },
        C_RESET,
        git_info.dirty,
        git_info.changed_files
    );
    println!(
        "  {}│{}  {}Active Provider :{} {}{}{}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, C_CYAN, settings.provider, C_RESET
    );
    println!(
        "  {}│{}  {}Active Model    :{} {}{}{}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, C_BOLD, settings.model, C_RESET
    );
    println!(
        "  {}│{}  {}Reasoning Effort:{} {}{}{}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, C_MAGENTA, settings.reasoning_effort, C_RESET
    );
    println!(
        "  {}│{}  {}Agent Mode      :{} {}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        mode_badge(&hm.agent_mode())
    );
    println!(
        "  {}│{}  {}Ollama URL      :{} {}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, settings.ollama_base_url
    );
    println!(
        "  {}│{}  {}llama.cpp URL   :{} {}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, settings.llamacpp_base_url
    );
    println!(
        "  {}│{}  {}LM Studio URL   :{} {}",
        C_BORDER, C_RESET, C_GRAY, C_RESET, settings.lmstudio_base_url
    );
    println!(
        "  {}│{}  {}Configured Keys :{} {}",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        if configured_keys.is_empty() {
            "none (local-ready)".to_string()
        } else {
            configured_keys.join(", ")
        }
    );
    println!(
        "  {}│{}  {}MCP Servers     :{} {} configured",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        mcp_servers.len()
    );
    println!(
        "  {}│{}  {}Skills Loaded   :{} {} available",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        skills.len()
    );
    println!(
        "  {}│{}  {}Memory Entries  :{} {} stored",
        C_BORDER,
        C_RESET,
        C_GRAY,
        C_RESET,
        memories.len()
    );
    println!(
        "  {}{}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BOLD, C_GREEN, C_RESET
    );
    println!();
}

fn run_models(hm: &HarnessManager, json_out: bool, filter: Option<&str>) {
    let settings = hm.settings();
    let mut models = settings.discover_models();
    if let Some(q) = filter.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        models.retain(|m| {
            m.id.to_lowercase().contains(&q)
                || m.provider.to_lowercase().contains(&q)
                || m.desc.to_lowercase().contains(&q)
        });
    }
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&models).unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}Discovered {} models{} (Active: {}{}{}):",
        C_BOLD,
        models.len(),
        C_RESET,
        C_GREEN,
        settings.model,
        C_RESET
    );
    println!(
        "  {}{:<34} {:<16} {:<8} {:<8} {}{}",
        C_GRAY, "MODEL ID", "PROVIDER", "TOOLS", "THINK", "DESCRIPTION", C_RESET
    );
    println!("  {}{}{}", C_BORDER, "─".repeat(92), C_RESET);
    for m in models {
        let is_active = m.id == settings.model;
        let marker = if is_active {
            format!("{}● {}", C_GREEN, C_RESET)
        } else {
            "  ".to_string()
        };
        let id_col = if is_active { C_GREEN } else { C_CYAN };
        println!(
            "  {}{}{:<32}{} {:<16} {:<8} {:<8} {}{}{}",
            marker,
            id_col,
            m.id,
            C_RESET,
            m.provider,
            if m.has_tools { "✓" } else { "·" },
            if m.has_thinking { "✓" } else { "·" },
            C_GRAY,
            m.desc,
            C_RESET
        );
    }
    println!();
}

fn run_duel(path: &PathBuf, json_out: bool, fail_under: Option<u8>) {
    let report = scan_workspace(path);
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        if let Some(threshold) = fail_under {
            if report.score < threshold {
                std::process::exit(1);
            }
        }
        return;
    }
    let score_col = if report.score >= 85 {
        C_GREEN
    } else if report.score >= 60 {
        C_YELLOW
    } else {
        C_RED
    };
    println!(
        "\n  {}⚔ Running Red Team vs. White Team Security Duel on {}{}{}...",
        C_CYAN,
        C_BOLD,
        path.display(),
        C_RESET
    );
    println!(
        "  {}╭──────────────────────────────────────────────────────────────────────────────╮{}",
        C_BORDER, C_RESET
    );
    println!(
        "  {}│{}  {}SECURITY DUEL SCORE:{} {}{}{}/100{} — {}",
        C_BORDER,
        C_RESET,
        C_BOLD,
        C_RESET,
        C_BOLD,
        score_col,
        report.score,
        C_RESET,
        report.summary
    );
    println!(
        "  {}╰──────────────────────────────────────────────────────────────────────────────╯{}",
        C_BORDER, C_RESET
    );
    if report.red_team_findings.is_empty() {
        println!(
            "  {}✓ Zero vulnerabilities detected by Red Team scanner.{}",
            C_GREEN, C_RESET
        );
    } else {
        println!(
            "  {}{}Red Team Findings ({}):{}",
            C_BOLD,
            C_RED,
            report.red_team_findings.len(),
            C_RESET
        );
        for f in &report.red_team_findings {
            println!(
                "    {}• [{}]{} {} ({}:{}) -> {}{}{}",
                C_RED,
                f.severity.to_uppercase(),
                C_RESET,
                f.title,
                f.file,
                f.line,
                C_GRAY,
                f.remediation,
                C_RESET
            );
        }
    }
    if !report.white_team_patches.is_empty() {
        println!("\n  {}{}White Team Defensive Patches:{}", C_BOLD, C_GREEN, C_RESET);
        for p in &report.white_team_patches {
            println!("    {}✓{} {}", C_GREEN, C_RESET, p);
        }
    }
    println!();
    if let Some(threshold) = fail_under {
        if report.score < threshold {
            eprintln!(
                "\n{}[!] Security score {} is below required threshold {}.{}",
                C_RED, report.score, threshold, C_RESET
            );
            std::process::exit(1);
        }
    }
}

fn run_map(ws_path: &PathBuf, max_files: usize, json_out: bool) {
    let map_str = generate_repo_map(ws_path, max_files, 8_000);
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "workspace": ws_path,
                "repo_map": map_str,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("{}", render_terminal_markdown(&map_str));
    }
}

fn run_outline(ws_path: &PathBuf, rel_file: &str, json_out: bool, exit_on_err: bool) {
    let Ok(ws) = Workspace::new(ws_path) else {
        if json_out {
            println!(
                "{}",
                serde_json::json!({ "status": "error", "error": format!("Could not open workspace '{}'", ws_path.display()) })
            );
        } else {
            eprintln!(
                "{}[!] Could not open workspace '{}'{}",
                C_RED,
                ws_path.display(),
                C_RESET
            );
        }
        if exit_on_err {
            std::process::exit(1);
        }
        return;
    };
    let Ok(resolved) = ws.resolve(rel_file) else {
        if json_out {
            println!(
                "{}",
                serde_json::json!({ "status": "error", "error": format!("Could not resolve file '{}'", rel_file) })
            );
        } else {
            eprintln!("{}[!] Could not resolve file '{}'{}", C_RED, rel_file, C_RESET);
        }
        if exit_on_err {
            std::process::exit(1);
        }
        return;
    };
    if !resolved.is_file() {
        if json_out {
            println!(
                "{}",
                serde_json::json!({ "status": "error", "error": format!("File '{}' does not exist", rel_file) })
            );
        } else {
            eprintln!("{}[!] File '{}' does not exist{}", C_RED, rel_file, C_RESET);
        }
        if exit_on_err {
            std::process::exit(1);
        }
        return;
    }
    let symbols = outline_file(&resolved);
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&symbols).unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}AST Outline for {}{}{} ({} symbols):{}",
        C_BOLD,
        C_CYAN,
        rel_file,
        C_RESET,
        symbols.len(),
        C_RESET
    );
    for sym in symbols {
        println!(
            "  {}[L{:>4}-L{:>4}]{} {}{:<10}{} {}{:<26}{} │ {}{}{}",
            C_GRAY,
            sym.line,
            sym.end_line,
            C_RESET,
            C_MAGENTA,
            sym.kind,
            C_RESET,
            C_BOLD,
            sym.name,
            C_RESET,
            C_CYAN_DIM,
            sym.signature,
            C_RESET
        );
    }
    println!();
}

fn run_callers(ws_path: &PathBuf, symbol: &str, json_out: bool) {
    let callers = find_callers(ws_path, symbol);
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "symbol": symbol,
                "callers": callers,
            }))
            .unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}Call sites for `{}{}{}` ({}):{}",
        C_BOLD,
        C_CYAN,
        symbol,
        C_RESET,
        callers.len(),
        C_RESET
    );
    for c in callers {
        println!("    {}•{} {}", C_GREEN, C_RESET, c);
    }
    println!();
}

fn run_symbol(ws_path: &PathBuf, symbol: &str, json_out: bool) {
    let Ok(ws) = Workspace::new(ws_path) else {
        eprintln!(
            "{}[!] Could not open workspace '{}'{}",
            C_RED,
            ws_path.display(),
            C_RESET
        );
        return;
    };
    let mut te = ToolEngine::new(ws);
    let (res, is_ok, _) = te.execute_tool(
        "find_symbol",
        &serde_json::json!({ "symbol": symbol, "max_lines": 80 }),
    );
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "symbol": symbol,
                "ok": is_ok,
                "result": res,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("\n{}\n", res);
    }
}

fn run_grep(
    ws_path: &PathBuf,
    query: &str,
    subpath: Option<&str>,
    include: Option<&str>,
    context: usize,
    json_out: bool,
) {
    let Ok(ws) = Workspace::new(ws_path) else {
        eprintln!(
            "{}[!] Could not open workspace '{}'{}",
            C_RED,
            ws_path.display(),
            C_RESET
        );
        return;
    };
    let te = ToolEngine::new(ws);
    match te.grep_search_filtered(query, subpath, include, Some(context)) {
        Ok(output) => {
            let lines: Vec<&str> = output.lines().collect();
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "query": query,
                        "count": lines.len(),
                        "matches": lines,
                    }))
                    .unwrap_or_default()
                );
            } else if output.trim().is_empty() {
                println!("  {}No matches found for `{}`.{}" , C_GRAY, query, C_RESET);
            } else {
                println!("\n{}\n", output);
            }
        }
        Err(e) => {
            if json_out {
                println!(
                    "{}",
                    serde_json::json!({ "status": "error", "error": e.to_string() })
                );
            } else {
                eprintln!("{}[!] Grep error: {}{}", C_RED, e, C_RESET);
            }
        }
    }
}

fn run_skills(hm: &HarnessManager, json_out: bool, filter: Option<&str>) {
    let ws = hm.active_workspace_path();
    let mut skills = SkillsManager::list_skills_for_workspace(Some(&ws));
    if let Some(q) = filter.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        skills.retain(|sk| {
            sk.id.to_lowercase().contains(&q)
                || sk.name.to_lowercase().contains(&q)
                || sk.description.to_lowercase().contains(&q)
        });
    }
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&skills).unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}{}Available Skills ({}):{}",
        C_BOLD,
        C_GREEN,
        skills.len(),
        C_RESET
    );
    for sk in skills {
        let desc: String = sk.description.chars().take(78).collect();
        let ellipsis = if sk.description.chars().count() > 78 {
            "…"
        } else {
            ""
        };
        println!(
            "  {}•{} {}{:<28}{} {}[{}]{} {}{}{}",
            C_GREEN,
            C_RESET,
            C_CYAN,
            sk.id,
            C_RESET,
            C_MAGENTA,
            sk.scope,
            C_RESET,
            C_GRAY,
            desc,
            ellipsis
        );
    }
    println!();
}

fn run_mcp(probe: bool, json_out: bool) {
    let servers = McpManager::load();
    if probe {
        let results: Vec<_> = servers.iter().map(McpManager::probe_server).collect();
        if json_out {
            println!(
                "{}",
                serde_json::to_string_pretty(&results).unwrap_or_default()
            );
            return;
        }
        println!(
            "\n  {}{}MCP Server Probe Results ({}):{}",
            C_BOLD,
            C_CYAN,
            results.len(),
            C_RESET
        );
        for r in results {
            let status_badge = if r.ok {
                format!("{}✓ online{}", C_GREEN, C_RESET)
            } else {
                format!("{}✗ offline{}", C_RED, C_RESET)
            };
            println!(
                "  • {}{:<22}{} [{}] ({} ms) — {} tools",
                C_BOLD,
                r.name,
                C_RESET,
                status_badge,
                r.latency_ms,
                r.tools.len()
            );
        }
        println!();
        return;
    }
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&servers).unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}{}Configured MCP Servers ({}):{}",
        C_BOLD,
        C_CYAN,
        servers.len(),
        C_RESET
    );
    for srv in servers {
        let badge = if srv.enabled {
            format!("{}enabled{}", C_GREEN, C_RESET)
        } else {
            format!("{}disabled{}", C_GRAY, C_RESET)
        };
        println!(
            "  • {}{:<22}{} [{}] {}{}{}",
            C_BOLD, srv.name, C_RESET, badge, C_GRAY, srv.description, C_RESET
        );
    }
    println!();
}

fn run_memory(query: Option<&str>, json_out: bool) {
    let entries = match query {
        Some(q) if !q.trim().is_empty() => MemoryStore::recall(q),
        _ => MemoryStore::load(),
    };
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&entries).unwrap_or_default()
        );
        return;
    }
    println!(
        "\n  {}{}Persistent Memory Entries ({}):{}",
        C_BOLD,
        C_CYAN,
        entries.len(),
        C_RESET
    );
    if entries.is_empty() {
        println!("  {}No memory entries stored yet.{}", C_GRAY, C_RESET);
    }
    for m in entries {
        println!(
            "  {}•{} {}[{}]{} {}{}{}: {}",
            C_GREEN,
            C_RESET,
            C_MAGENTA,
            m.category,
            C_RESET,
            C_BOLD,
            m.key,
            C_RESET,
            m.content
        );
    }
    println!();
}

fn run_version(json_out: bool) {
    if json_out {
        let out = serde_json::json!({
            "version": CURRENT_VERSION,
            "changelog": builtin_changelog(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return;
    }
    println!(
        "\n  {}{}Unjess {} (100% Native Rust Engine){}",
        C_BOLD, C_GREEN, CURRENT_VERSION, C_RESET
    );
    if let Some(latest) = builtin_changelog().first() {
        println!(
            "  {}Latest Release:{} {} ({}) — {}{}{}",
            C_GRAY, C_RESET, latest.version, latest.date, C_CYAN, latest.title, C_RESET
        );
        for item in &latest.changes {
            println!("    {}•{} {}", C_GREEN, C_RESET, item);
        }
    }
    println!();
}

// ============================================================================
// Autonomous Turn Execution & Live Terminal Telemetry
// ============================================================================

fn summarize_tool_args(args: &serde_json::Value) -> String {
    if let Some(obj) = args.as_object() {
        if obj.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for (k, v) in obj.iter().take(2) {
            let val_str = match v {
                serde_json::Value::String(s) => {
                    let line = s.lines().next().unwrap_or("");
                    let clipped: String = line.chars().take(42).collect();
                    if line.chars().count() > 42 {
                        format!("\"{}…\"", clipped)
                    } else {
                        format!("\"{}\"", clipped)
                    }
                }
                other => {
                    let s = other.to_string();
                    let clipped: String = s.chars().take(32).collect();
                    if s.chars().count() > 32 {
                        format!("{}…", clipped)
                    } else {
                        clipped
                    }
                }
            };
            parts.push(format!("{}={}", k, val_str));
        }
        return format!("({})", parts.join(", "));
    }
    String::new()
}

async fn execute_turn_and_wait(
    hm: &Arc<HarnessManager>,
    prompt: String,
    json_out: bool,
    exit_on_error: bool,
) {
    hm.submit_prompt(prompt);
    let mut last_step = String::new();
    let mut printed_tools = 0usize;

    let hm_abort = Arc::clone(hm);
    let ctrl_c_handle = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!(
                "\n  {}[!] Ctrl+C received — aborting active turn...{}",
                C_YELLOW, C_RESET
            );
            hm_abort.abort_turn();
        }
    });

    loop {
        tokio::time::sleep(Duration::from_millis(75)).await;
        let snap = hm.live_turn_snapshot();
        if !json_out {
            if !snap.current_step.is_empty()
                && snap.current_step != last_step
                && snap.current_step != "Ready"
            {
                eprintln!(
                    "  {}◌{} {}{}{}",
                    C_CYAN, C_RESET, C_GRAY, snap.current_step, C_RESET
                );
                last_step = snap.current_step.clone();
            }
            while printed_tools < snap.tool_calls.len() {
                let tc = &snap.tool_calls[printed_tools];
                let status_icon = if tc.status == "ok" {
                    format!("{}✓{}", C_GREEN, C_RESET)
                } else {
                    format!("{}✗{}", C_RED, C_RESET)
                };
                let args_preview = summarize_tool_args(&tc.arguments);
                eprintln!(
                    "  {}⚡ Step #{:<2}{} {}{}{} {}{}{} {} {}({} ms){}",
                    C_YELLOW,
                    printed_tools + 1,
                    C_RESET,
                    C_BOLD,
                    tc.name,
                    C_RESET,
                    C_GRAY,
                    args_preview,
                    C_RESET,
                    status_icon,
                    C_GRAY,
                    tc.duration_ms,
                    C_RESET
                );
                printed_tools += 1;
            }
        }
        if !snap.running {
            break;
        }
    }
    ctrl_c_handle.abort();

    let conv = hm.active_conversation();
    if let Some(last_msg) = conv.messages.last() {
        let is_provider_err = last_msg.content.starts_with("**LLM Provider Error");
        if json_out {
            println!(
                "{}",
                serde_json::to_string_pretty(last_msg).unwrap_or_default()
            );
            if exit_on_error && is_provider_err {
                std::process::exit(1);
            }
            return;
        }
        if let Some(thinking) = &last_msg.thinking {
            print_thinking_block(thinking);
        }

        println!();
        print!("{}", render_terminal_markdown(&last_msg.content));

        if !last_msg.diffs.is_empty() {
            eprintln!(
                "\n  {}{}Modified Files ({}):{}",
                C_BOLD,
                C_CYAN,
                last_msg.diffs.len(),
                C_RESET
            );
            for d in &last_msg.diffs {
                eprintln!(
                    "    {}📄 {}{} [{}] {}+{}{} {}-{}{}",
                    C_WHITE,
                    d.path,
                    C_RESET,
                    d.status,
                    C_GREEN_STD,
                    d.additions,
                    C_RESET,
                    C_RED,
                    d.deletions,
                    C_RESET
                );
            }
        }

        let model_label = last_msg
            .model
            .clone()
            .unwrap_or_else(|| hm.settings().model);
        eprintln!(
            "  {}─ ✓ Turn complete · {} tokens (${:.4}) · {}{}",
            C_GRAY, last_msg.tokens, last_msg.cost_usd, model_label, C_RESET
        );

        if exit_on_error && is_provider_err {
            std::process::exit(1);
        }
    }
}

// ============================================================================
// Interactive REPL & Slash Command Dispatcher
// ============================================================================

/// Processes a single REPL input line. Returns `false` if the REPL should exit (`/exit` or `/quit`).
async fn handle_repl_line(hm: &Arc<HarnessManager>, raw_input: &str) -> bool {
    let input = raw_input.trim();
    if input.is_empty() {
        return true;
    }

    // Direct workspace shell execution via `!<cmd>`
    if let Some(shell_cmd) = input.strip_prefix('!') {
        let shell_cmd = shell_cmd.trim();
        if shell_cmd.is_empty() {
            println!("  {}Usage: !<shell_command>{}", C_YELLOW, C_RESET);
            return true;
        }
        let ws_path = hm.active_workspace_path();
        if let Ok(ws) = Workspace::new(&ws_path) {
            let te = ToolEngine::new(ws);
            println!(
                "  {}⚡ Running `{}` in {}...{}",
                C_CYAN,
                shell_cmd,
                ws_path.display(),
                C_RESET
            );
            match te.run_command(shell_cmd, 60) {
                Ok(out) => println!("\n{}\n", out.trim_end()),
                Err(e) => eprintln!("  {}[!] Command error: {}{}", C_RED, e, C_RESET),
            }
        }
        return true;
    }

    if input.starts_with('/') {
        let mut parts = input.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.next().unwrap_or("").trim();

        match cmd.as_str() {
            "/" | "/?" | "/help" | "/commands" | "/menu" => {
                print_slash_command_palette();
            }
            "/exit" | "/quit" | "/q" => {
                println!("  {}Goodbye! 👋{}", C_GREEN, C_RESET);
                return false;
            }
            "/context" | "/tokens" | "/usage" | "/cost" => {
                print_context_card(hm);
            }
            "/status" | "/info" => {
                print_status_card(hm);
            }
            "/new" => {
                let title_opt = if arg.is_empty() { None } else { Some(arg) };
                let new_id = hm.new_conversation(title_opt);
                let conv = hm.conversation_by_id(&new_id);
                println!(
                    "  {}✓ Started new conversation: {}{}{} ({})",
                    C_GREEN,
                    C_BOLD,
                    conv.title,
                    C_RESET,
                    &conv.id[..conv.id.len().min(8)]
                );
            }
            "/convs" | "/chats" | "/history" => {
                let convs = hm.conversations();
                let active_id = hm.active_conversation_id();
                if arg.is_empty() {
                    println!(
                        "\n  {}{}Conversations ({}):{}",
                        C_BOLD,
                        C_CYAN,
                        convs.len(),
                        C_RESET
                    );
                    for (idx, c) in convs.iter().enumerate() {
                        let marker = if c.id == active_id {
                            format!("{}●{}", C_GREEN, C_RESET)
                        } else {
                            " ".to_string()
                        };
                        println!(
                            "  {} [{}] {}{:<28}{} {}({} msgs · {} tokens · {}){}",
                            marker,
                            idx + 1,
                            C_BOLD,
                            c.title,
                            C_RESET,
                            C_GRAY,
                            c.messages.len(),
                            c.total_tokens,
                            &c.id[..c.id.len().min(8)],
                            C_RESET
                        );
                    }
                    println!(
                        "  {}Tip: Type `/convs <number>` to switch conversations.{}\n",
                        C_GRAY, C_RESET
                    );
                } else if let Ok(num) = arg.parse::<usize>() {
                    if num >= 1 && num <= convs.len() {
                        let target = &convs[num - 1];
                        hm.set_active_conversation(&target.id);
                        println!(
                            "  {}✓ Switched to conversation [{}]: {}{}{}",
                            C_GREEN, num, C_BOLD, target.title, C_RESET
                        );
                    } else {
                        println!(
                            "  {}[!] Invalid conversation index {}.{}",
                            C_RED, num, C_RESET
                        );
                    }
                } else if let Some(target) = convs.iter().find(|c| {
                    c.id.starts_with(arg) || c.title.to_lowercase().contains(&arg.to_lowercase())
                }) {
                    hm.set_active_conversation(&target.id);
                    println!(
                        "  {}✓ Switched to conversation: {}{}{}",
                        C_GREEN, C_BOLD, target.title, C_RESET
                    );
                } else {
                    println!(
                        "  {}[!] No conversation matching '{}'.{}",
                        C_RED, arg, C_RESET
                    );
                }
            }
            "/model" => {
                if arg.is_empty() {
                    let s = hm.settings();
                    let prov = resolve_provider_for_model(&s, &s.model);
                    println!(
                        "  {}Active model:{} {}{}{} (provider: {}{}{})",
                        C_GRAY, C_RESET, C_BOLD, s.model, C_RESET, C_CYAN, prov, C_RESET
                    );
                } else {
                    let mut s = hm.settings();
                    s.model = arg.to_string();
                    s.sync_provider_with_model();
                    let prov = s.provider.clone();
                    hm.update_settings(s);
                    println!(
                        "  {}✓ Switched active model to {}{}{} (provider: {}{}{})",
                        C_GREEN, C_BOLD, arg, C_RESET, C_CYAN, prov, C_RESET
                    );
                }
            }
            "/provider" => {
                if arg.is_empty() {
                    let s = hm.settings();
                    println!(
                        "  {}Active provider:{} {}{}{} (model: {})",
                        C_GRAY, C_RESET, C_CYAN, s.provider, C_RESET, s.model
                    );
                } else {
                    let mut s = hm.settings();
                    s.provider = arg.to_lowercase();
                    hm.update_settings(s.clone());
                    println!(
                        "  {}✓ Active provider set to {}{}{}",
                        C_GREEN, C_BOLD, s.provider, C_RESET
                    );
                }
            }
            "/effort" => {
                if arg.is_empty() {
                    println!(
                        "  {}Reasoning effort:{} {}{}{}",
                        C_GRAY,
                        C_RESET,
                        C_MAGENTA,
                        hm.settings().reasoning_effort,
                        C_RESET
                    );
                } else {
                    let mut s = hm.settings();
                    s.reasoning_effort = arg.to_lowercase();
                    hm.update_settings(s.clone());
                    println!(
                        "  {}✓ Reasoning effort set to {}{}{}",
                        C_GREEN, C_MAGENTA, s.reasoning_effort, C_RESET
                    );
                }
            }
            "/thinking" => {
                match arg.to_lowercase().as_str() {
                    "full" | "on" | "all" => {
                        THINKING_DISPLAY_MODE.store(1, Ordering::Relaxed);
                        println!("  {}✓ Thinking display set to FULL.{}", C_GREEN, C_RESET);
                    }
                    "off" | "hide" | "none" => {
                        THINKING_DISPLAY_MODE.store(2, Ordering::Relaxed);
                        println!("  {}✓ Thinking display set to HIDDEN.{}", C_GREEN, C_RESET);
                    }
                    "compact" | "short" => {
                        THINKING_DISPLAY_MODE.store(0, Ordering::Relaxed);
                        println!("  {}✓ Thinking display set to COMPACT (up to 4 lines).{}", C_GREEN, C_RESET);
                    }
                    _ => {
                        let cur = match THINKING_DISPLAY_MODE.load(Ordering::Relaxed) {
                            1 => "full",
                            2 => "off",
                            _ => "compact",
                        };
                        println!(
                            "  {}Thinking display mode:{} {}{}{} (use `/thinking compact|full|off`)",
                            C_GRAY, C_RESET, C_CYAN, cur, C_RESET
                        );
                    }
                }
            }
            "/models" => {
                let filter = if arg.is_empty() { None } else { Some(arg) };
                run_models(hm, false, filter);
            }
            "/mode" => {
                if arg.is_empty() {
                    println!(
                        "  {}Active mode:{} {}",
                        C_GRAY,
                        C_RESET,
                        mode_badge(&hm.agent_mode())
                    );
                } else {
                    hm.set_agent_mode(arg);
                    println!(
                        "  {}✓ Agent mode set to {}",
                        C_GREEN,
                        mode_badge(&hm.agent_mode())
                    );
                }
            }
            "/cd" | "/workspace" => {
                if arg.is_empty() {
                    println!(
                        "  {}Active workspace:{} {}{}{}",
                        C_GRAY,
                        C_RESET,
                        C_BLUE,
                        hm.active_workspace_path().display(),
                        C_RESET
                    );
                } else {
                    let id = hm.add_workspace(arg);
                    hm.set_active_workspace(&id);
                    println!(
                        "  {}✓ Switched workspace to {}{}{}",
                        C_GREEN,
                        C_BLUE,
                        hm.active_workspace_path().display(),
                        C_RESET
                    );
                }
            }
            "/files" => {
                let ws_path = hm.active_workspace_path();
                if let Ok(ws) = Workspace::new(&ws_path) {
                    let mut files = ws.list_all_files(200);
                    if !arg.is_empty() {
                        let q = arg.to_lowercase();
                        files.retain(|f: &String| f.to_lowercase().contains(&q));
                    }
                    println!(
                        "\n  {}{}Workspace Files ({} shown):{}",
                        C_BOLD,
                        C_CYAN,
                        files.len(),
                        C_RESET
                    );
                    for f in files.iter().take(60) {
                        println!("    {}📄{} {}", C_BLUE, C_RESET, f);
                    }
                    if files.len() > 60 {
                        println!(
                            "    {}… (+{} more files — use `/files <filter>` to narrow){}",
                            C_GRAY,
                            files.len() - 60,
                            C_RESET
                        );
                    }
                    println!();
                }
            }
            "/git" => {
                let ws_path = hm.active_workspace_path();
                if let Ok(ws) = Workspace::new(&ws_path) {
                    let info = ws.git_info();
                    let te = ToolEngine::new(ws);
                    println!(
                        "\n  {}{}Git Status ({}):{} branch={}{}{} dirty={} changed={}",
                        C_BOLD,
                        C_CYAN,
                        ws_path.display(),
                        C_RESET,
                        C_YELLOW,
                        if info.branch.is_empty() {
                            "(none)"
                        } else {
                            &info.branch
                        },
                        C_RESET,
                        info.dirty,
                        info.changed_files
                    );
                    if let Ok(status_out) = te.run_command("git status --short", 15) {
                        if !status_out.trim().is_empty() {
                            println!("{}", status_out.trim_end());
                        }
                    }
                    println!();
                }
            }
            "/diff" => {
                let conv = hm.active_conversation();
                if let Some(last) = conv.messages.iter().rev().find(|m| !m.diffs.is_empty()) {
                    println!();
                    for d in &last.diffs {
                        println!(
                            "  {}{}📄 {} [{}] (+{} / -{}){}",
                            C_BOLD, C_WHITE, d.path, d.status, d.additions, d.deletions, C_RESET
                        );
                        print!("{}", render_colorized_diff(&d.diff));
                    }
                    println!();
                } else {
                    println!(
                        "  {}No diffs recorded in the current conversation.{}",
                        C_GRAY, C_RESET
                    );
                }
            }
            "/revert" | "/undo" => {
                let (restored, deleted) = hm.undo_last_turn("");
                println!(
                    "  {}✓ Reverted last turn: restored {} file(s), removed {} created file(s).{}",
                    C_GREEN,
                    restored.len(),
                    deleted.len(),
                    C_RESET
                );
            }
            "/doctor" => run_doctor(hm, false),
            "/compact" => {
                hm.compact_active_conversation();
                println!("  {}✓ Conversation context compacted.{}", C_GREEN, C_RESET);
            }
            "/mcp" => {
                let probe = arg.eq_ignore_ascii_case("probe") || arg.eq_ignore_ascii_case("--probe");
                run_mcp(probe, false);
            }
            "/skills" => {
                let filter = if arg.is_empty() { None } else { Some(arg) };
                run_skills(hm, false, filter);
            }
            "/agents" | "/subagents" => {
                let subs = hm.subagents();
                println!(
                    "\n  {}{}Autonomous Subagents ({}):{}",
                    C_BOLD,
                    C_CYAN,
                    subs.len(),
                    C_RESET
                );
                for s in subs {
                    println!(
                        "  • {}{:<20}{} [{}] — {} (model: {})",
                        C_BOLD, s.name, C_RESET, s.status, s.role, s.model
                    );
                }
                println!();
            }
            "/workflows" => {
                let wfs = hm.workflows();
                println!(
                    "\n  {}{}Multi-Step Workflows ({}):{}",
                    C_BOLD,
                    C_CYAN,
                    wfs.len(),
                    C_RESET
                );
                for w in wfs {
                    println!(
                        "  • {}{:<28}{} [{}] — {}",
                        C_BOLD, w.title, C_RESET, w.category, w.description
                    );
                }
                println!();
            }
            "/gui" | "/desktop" | "/app" => {
                let exe_dir = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."));
                let mut candidates = vec![
                    exe_dir.join("Unjess.exe"),
                    exe_dir.join("..").join("Unjess.exe"),
                ];
                if let Ok(local) = std::env::var("LOCALAPPDATA") {
                    candidates.push(
                        PathBuf::from(&local)
                            .join("Programs")
                            .join("UnjessRust")
                            .join("Unjess.exe"),
                    );
                }
                if let Some(gui_exe) = candidates.into_iter().find(|p| p.exists()) {
                    let _ = std::process::Command::new(&gui_exe).spawn();
                    println!(
                        "  {}✓ Launched Unjess Desktop GUI ({}){}",
                        C_GREEN,
                        gui_exe.display(),
                        C_RESET
                    );
                } else {
                    println!(
                        "  {}[!] Could not locate Unjess.exe desktop binary.{}",
                        C_RED, C_RESET
                    );
                }
            }
            "/duel" => {
                if arg.is_empty() {
                    let ws = hm.active_workspace_path();
                    run_duel(&ws, false, None);
                } else {
                    let target = PathBuf::from(arg);
                    if target.exists() {
                        run_duel(&target, false, None);
                    } else {
                        execute_turn_and_wait(hm, input.to_string(), false, false).await;
                    }
                }
            }
            "/map" => {
                let ws = hm.active_workspace_path();
                run_map(&ws, 40, false);
            }
            "/outline" => {
                if arg.is_empty() {
                    println!("  {}Usage: /outline <relative-file-path>{}", C_YELLOW, C_RESET);
                } else {
                    let ws = hm.active_workspace_path();
                    run_outline(&ws, arg, false, false);
                }
            }
            "/callers" => {
                if arg.is_empty() {
                    println!("  {}Usage: /callers <symbol>{}", C_YELLOW, C_RESET);
                } else {
                    let ws = hm.active_workspace_path();
                    run_callers(&ws, arg, false);
                }
            }
            "/symbol" => {
                if arg.is_empty() {
                    println!("  {}Usage: /symbol <symbol>{}", C_YELLOW, C_RESET);
                } else {
                    let ws = hm.active_workspace_path();
                    run_symbol(&ws, arg, false);
                }
            }
            "/grep" => {
                if arg.is_empty() {
                    println!("  {}Usage: /grep <query>{}", C_YELLOW, C_RESET);
                } else {
                    let ws = hm.active_workspace_path();
                    run_grep(&ws, arg, None, None, 0, false);
                }
            }
            "/memory" => run_memory(Some(arg), false),
            "/clear" => {
                hm.clear_active_conversation();
                println!("  {}✓ Cleared active conversation.{}", C_GREEN, C_RESET);
            }
            "/version" | "/v" | "/changelog" => run_version(false),
            "/run" => {
                if arg.is_empty() {
                    println!(
                        "  {}Usage: /run <shell_command> (or !<shell_command>){}",
                        C_YELLOW, C_RESET
                    );
                } else {
                    let ws_path = hm.active_workspace_path();
                    if let Ok(ws) = Workspace::new(&ws_path) {
                        let te = ToolEngine::new(ws);
                        println!(
                            "  {}⚡ Running `{}` in {}...{}",
                            C_CYAN,
                            arg,
                            ws_path.display(),
                            C_RESET
                        );
                        match te.run_command(arg, 60) {
                            Ok(out) => println!("\n{}\n", out.trim_end()),
                            Err(e) => eprintln!("  {}[!] Command error: {}{}", C_RED, e, C_RESET),
                        }
                    }
                }
            }
            "/timer" => {
                if arg.is_empty() {
                    println!(
                        "  {}Usage: /timer <seconds> <prompt_or_!command>  (e.g. `/timer 10 !cargo check`){}",
                        C_YELLOW, C_RESET
                    );
                } else {
                    let mut tparts = arg.splitn(2, char::is_whitespace);
                    let sec_str = tparts.next().unwrap_or("10").trim_end_matches('s');
                    let secs = sec_str.parse::<u64>().unwrap_or(10).max(1);
                    let payload = tparts.next().unwrap_or("!echo Timer done").trim();
                    let ws_str = hm.active_workspace_path().to_string_lossy().to_string();
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
                        Some(Arc::clone(hm)),
                    );
                    println!(
                        "  {}⏰ Started {}s background timer ({}) -> {}{}{}",
                        C_GREEN, secs, task.id, C_CYAN, payload, C_RESET
                    );
                }
            }
            "/watch" => {
                let target = if arg.is_empty() { "." } else { arg };
                let mut wparts = target.splitn(2, "->");
                let watch_target = wparts.next().unwrap_or(".").trim();
                let on_change = wparts.next().unwrap_or("").trim();
                let ws_str = hm.active_workspace_path().to_string_lossy().to_string();
                let task = SchedulerManager::start_watcher(
                    if watch_target.is_empty() {
                        "."
                    } else {
                        watch_target
                    },
                    4,
                    on_change,
                    &ws_str,
                    Some(Arc::clone(hm)),
                );
                println!(
                    "  {}👁 Started live background watcher ({}) on `{}`{}",
                    C_GREEN, task.id, task.label, C_RESET
                );
            }
            "/tasks" | "/schedules" | "/schedule" => {
                let live = SchedulerManager::list_live_tasks();
                let cron = SchedulerManager::load();
                println!(
                    "\n  {}{}╭─ Active Background Timers, Watchers & Schedules ─────────────────────────────╮{}",
                    C_BOLD, C_CYAN, C_RESET
                );
                if live.is_empty() && cron.is_empty() {
                    println!(
                        "  {}│{}  {}No active background tasks. Start one with `/timer 10 !cmd` or `/watch src`.{}",
                        C_BORDER, C_RESET, C_GRAY, C_RESET
                    );
                }
                for t in &live {
                    let badge_col = if t.status == "running" {
                        C_GREEN
                    } else if t.status == "completed" {
                        C_CYAN
                    } else {
                        C_YELLOW
                    };
                    println!(
                        "  {}│{}  {}[{:<9}]{} {}{:<14}{} {:<32} {}{}{}",
                        C_BORDER,
                        C_RESET,
                        badge_col,
                        t.status,
                        C_RESET,
                        C_BOLD,
                        t.id,
                        C_RESET,
                        t.label,
                        C_GRAY,
                        t.last_event,
                        C_RESET
                    );
                }
                for s in &cron {
                    println!(
                        "  {}│{}  {}[cron     ]{} {}{:<14}{} {:<32} {}({}){}",
                        C_BORDER,
                        C_RESET,
                        if s.enabled { C_GREEN } else { C_GRAY },
                        C_RESET,
                        C_BOLD,
                        s.id,
                        C_RESET,
                        s.title,
                        C_GRAY,
                        s.cron_expression,
                        C_RESET
                    );
                }
                println!(
                    "  {}{}╰──────────────────────────────────────────────────────────────────────────────╯{}\n",
                    C_BOLD, C_CYAN, C_RESET
                );
            }
            "/stop-tasks" | "/cancel" => {
                let target = if arg.is_empty() { "all" } else { arg };
                let n = SchedulerManager::cancel_live_task(target);
                println!(
                    "  {}✓ Cancelled {} running background task(s).{}",
                    C_GREEN, n, C_RESET
                );
            }
            "/caveman" | "/ponytail" | "/goal" | "/budget" | "/plan" | "/auto"
            | "/autonomous" | "/supervised" | "/skill" => {
                execute_turn_and_wait(hm, input.to_string(), false, false).await;
            }
            _ => {
                // Find matching slash commands by prefix or substring
                let known_cmds = [
                    ("/context", "Show token usage bar, message count, and USD cost"),
                    ("/status", "Display session, model, workspace, and git status"),
                    ("/help", "Show all available slash commands"),
                    ("/model", "Show or switch active LLM model"),
                    ("/models", "Discover and list all available models"),
                    ("/provider", "Show or switch active LLM provider"),
                    ("/effort", "Switch reasoning effort (low|medium|high|max)"),
                    ("/mode", "Switch agent mode (auto|plan|supervised|duel)"),
                    ("/thinking", "Configure reasoning display (compact|full|off)"),
                    ("/new", "Start a new conversation"),
                    ("/convs", "List or switch conversations"),
                    ("/compact", "Compact conversation context"),
                    ("/clear", "Clear active conversation"),
                    ("/budget", "Configure session USD budget limit"),
                    ("/cd", "Switch active workspace directory"),
                    ("/files", "List workspace files"),
                    ("/git", "Show workspace git status"),
                    ("/diff", "Show unified diffs from the last turn"),
                    ("/undo", "Revert file modifications from the last turn"),
                    ("/duel", "Run Red Team vs. White Team Security Duel"),
                    ("/map", "Generate AST repository map"),
                    ("/outline", "Extract AST symbol outline for a file"),
                    ("/callers", "Find all call sites of a symbol"),
                    ("/symbol", "Locate a symbol definition"),
                    ("/grep", "Search workspace files"),
                    ("/skills", "List installed agent skills"),
                    ("/mcp", "List or probe MCP servers"),
                    ("/agents", "List autonomous subagents"),
                    ("/workflows", "List reusable workflows"),
                    ("/memory", "Inspect or search cross-session memory"),
                    ("/doctor", "Run system diagnostics"),
                    ("/gui", "Launch native Rust Desktop GUI"),
                    ("/version", "Show version & changelog"),
                    ("/exit", "Exit the CLI REPL"),
                ];
                let q = cmd.trim_start_matches('/');
                let matches: Vec<_> = known_cmds
                    .iter()
                    .filter(|(c, _)| c.trim_start_matches('/').contains(q))
                    .collect();

                println!(
                    "  {}[!] Unknown slash command '{}'.{} Type {}/{} or {}/help{} to view all commands.",
                    C_YELLOW, cmd, C_RESET, C_GREEN, C_RESET, C_GREEN, C_RESET
                );
                if !matches.is_empty() {
                    println!("  {}Did you mean:{}", C_GRAY, C_RESET);
                    for (m_cmd, m_desc) in matches.iter().take(6) {
                        println!(
                            "    {}{:<16}{} {}{}{}",
                            C_CYAN, m_cmd, C_RESET, C_GRAY, m_desc, C_RESET
                        );
                    }
                }
            }
        }
        return true;
    }

    execute_turn_and_wait(hm, input.to_string(), false, false).await;
    true
}

fn cli_slash_catalog() -> &'static [(&'static str, &'static str, &'static str, bool)] {
    &[
        ("/help", "/help", "Show all slash commands, timers, watchers & shortcuts", false),
        ("/context", "/context", "Show token usage bar, message count & session USD cost", false),
        ("/status", "/status", "Display session, model, workspace, and git status card", false),
        ("/timer", "/timer <sec> <cmd|prompt>", "Schedule a background countdown timer (e.g. /timer 10 !cargo check)", true),
        ("/watch", "/watch <path|!cmd>", "Start a live background file or command watcher (e.g. /watch src)", true),
        ("/tasks", "/tasks", "List active background timers, file watchers & cron schedules", false),
        ("/stop-tasks", "/stop-tasks", "Cancel running background timers & watchers", false),
        ("/run", "/run <cmd>", "Execute a shell command directly in the workspace (or !<cmd>)", true),
        ("/model", "/model [id]", "Show or switch active LLM model", true),
        ("/models", "/models [filter]", "Discover & list all available local and cloud models", false),
        ("/mode", "/mode <auto|plan|supervised|duel>", "Switch agent execution mode", true),
        ("/effort", "/effort <low|medium|high|max>", "Switch reasoning effort level", true),
        ("/caveman", "/caveman", "Toggle ultra-concise Caveman token-saving mode", false),
        ("/ponytail", "/ponytail", "Toggle expressive Ponytail pair-programming mode", false),
        ("/compact", "/compact", "Compress conversation context to free up token window", false),
        ("/new", "/new [title]", "Start a fresh conversation", false),
        ("/convs", "/convs [num]", "List or switch conversations", false),
        ("/clear", "/clear", "Clear active conversation messages", false),
        ("/files", "/files [filter]", "List tracked files in the active workspace", false),
        ("/git", "/git", "Show workspace git branch, working tree status & commits", false),
        ("/diff", "/diff", "Show colorized unified diffs from the last turn", false),
        ("/undo", "/undo", "Revert file modifications from the last turn", false),
        ("/duel", "/duel", "Run Red Team vs. White Team Security Duel scanner", false),
        ("/skills", "/skills [filter]", "List installed SKILL.md agent skills (47 skills)", false),
        ("/mcp", "/mcp [probe]", "List or live-probe configured MCP servers", false),
        ("/agents", "/agents", "List autonomous subagent fleet", false),
        ("/gui", "/gui", "Launch the native Rust GPU Desktop application (Unjess.exe)", false),
        ("/exit", "/exit", "Exit the CLI REPL", false),
    ]
}

#[cfg(windows)]
unsafe extern "C" {
    fn _kbhit() -> i32;
    fn _getwch() -> u16;
}

/// Interactive TUI line reader on Windows consoles:
/// - Immediately displays a live floating Slash Command Dropdown below the prompt the instant `/` is typed!
/// - Supports `↑`/`↓` arrow keys to highlight a slash command, `Tab` to complete, and `Enter` to execute.
/// - Polls background timers and watchers every 80ms while idle so notifications appear live!
fn read_interactive_tui_line() -> Option<String> {
    if !io::stdin().is_terminal() {
        let mut line = String::new();
        return match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        };
    }

    #[cfg(not(windows))]
    {
        let mut line = String::new();
        return match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        };
    }

    #[cfg(windows)]
    {
        let mut buf = String::new();
        let mut sel_idx: usize = 0;
        let mut menu_lines_drawn: usize = 0;

        let clear_menu = |drawn: &mut usize| {
            if *drawn > 0 {
                // Save cursor, move down 1 line, clear to end of screen, restore cursor
                print!("\x1b[s\r\n\x1b[J\x1b[u");
                let _ = io::stdout().flush();
                *drawn = 0;
            }
        };

        let render_menu = |current: &str, sel: &mut usize, drawn: &mut usize| {
            let trimmed = current.trim_start();
            if !trimmed.starts_with('/') || trimmed.contains(' ') {
                clear_menu(drawn);
                return;
            }
            let q = trimmed.to_lowercase();
            let matches: Vec<_> = cli_slash_catalog()
                .iter()
                .filter(|(cmd, _, _, _)| {
                    q == "/" || cmd.starts_with(&q) || cmd.contains(q.trim_start_matches('/'))
                })
                .take(8)
                .collect();

            if matches.is_empty() {
                clear_menu(drawn);
                return;
            }
            if *sel >= matches.len() {
                *sel = 0;
            }

            let mut out = String::from("\x1b[s\r\n\x1b[J");
            out.push_str(&format!(
                "  {}╭─ {}Slash Commands{} {}(↑/↓ select · Tab complete · Enter run){} ─╮{}\r\n",
                C_BORDER, C_GREEN, C_RESET, C_GRAY, C_BORDER, C_RESET
            ));
            for (i, (_, usage, desc, _)) in matches.iter().enumerate() {
                if i == *sel {
                    out.push_str(&format!(
                        "  {}│{} {}▸ {:<26}{} {}{}{}\r\n",
                        C_BORDER, C_RESET, C_GREEN, usage, C_RESET, C_WHITE, desc, C_RESET
                    ));
                } else {
                    out.push_str(&format!(
                        "  {}│{}   {}{:<26}{} {}{}{}\r\n",
                        C_BORDER, C_RESET, C_CYAN, usage, C_RESET, C_GRAY, desc, C_RESET
                    ));
                }
            }
            out.push_str(&format!(
                "  {}╰────────────────────────────────────────────────────────────╯{}\x1b[u",
                C_BORDER, C_RESET
            ));
            print!("{}", out);
            let _ = io::stdout().flush();
            *drawn = matches.len() + 2;
        };

        loop {
            // Check for background timer / watcher notifications while waiting for keystrokes
            let notes = SchedulerManager::drain_notifications();
            if !notes.is_empty() {
                clear_menu(&mut menu_lines_drawn);
                for note in notes {
                    print!(
                        "\r\x1b[K  {}{}{}\r\n{}╰─{}❯{} {}",
                        C_GREEN, note, C_RESET, C_BORDER, C_GREEN, C_RESET, buf
                    );
                }
                let _ = io::stdout().flush();
                render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
            }

            let has_key = unsafe { _kbhit() != 0 };
            if !has_key {
                std::thread::sleep(Duration::from_millis(40));
                continue;
            }

            let ch = unsafe { _getwch() };
            // Ctrl+C (3) or Ctrl+D (4)
            if ch == 3 {
                clear_menu(&mut menu_lines_drawn);
                println!("^C");
                return Some(String::new());
            }
            if ch == 4 {
                clear_menu(&mut menu_lines_drawn);
                println!();
                return None;
            }

            // Extended / Arrow key prefix (0 or 0xE0)
            if ch == 0 || ch == 0xE0 {
                let ext = unsafe { _getwch() };
                let trimmed = buf.trim_start();
                if trimmed.starts_with('/') && !trimmed.contains(' ') {
                    let q = trimmed.to_lowercase();
                    let count = cli_slash_catalog()
                        .iter()
                        .filter(|(cmd, _, _, _)| {
                            q == "/"
                                || cmd.starts_with(&q)
                                || cmd.contains(q.trim_start_matches('/'))
                        })
                        .take(8)
                        .count();
                    if count > 0 {
                        if ext == 72 {
                            // Up arrow
                            sel_idx = if sel_idx == 0 { count - 1 } else { sel_idx - 1 };
                            render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
                        } else if ext == 80 {
                            // Down arrow
                            sel_idx = (sel_idx + 1) % count;
                            render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
                        }
                    }
                }
                continue;
            }

            // Enter (\r = 13 or \n = 10)
            if ch == 13 || ch == 10 {
                clear_menu(&mut menu_lines_drawn);
                let trimmed = buf.trim();
                if trimmed.starts_with('/') && !trimmed.contains(' ') && trimmed != "/" {
                    let q = trimmed.to_lowercase();
                    let matches: Vec<_> = cli_slash_catalog()
                        .iter()
                        .filter(|(cmd, _, _, _)| {
                            cmd.starts_with(&q) || cmd.contains(q.trim_start_matches('/'))
                        })
                        .take(8)
                        .collect();
                    if let Some(&(sel_cmd, _, _, takes_args)) =
                        matches.get(sel_idx).or_else(|| matches.first())
                    {
                        if *takes_args && trimmed != *sel_cmd {
                            buf = format!("{} ", sel_cmd);
                            print!(
                                "\r\x1b[K{}╰─{}❯{} {}",
                                C_BORDER, C_GREEN, C_RESET, buf
                            );
                            let _ = io::stdout().flush();
                            continue;
                        } else {
                            buf = sel_cmd.to_string();
                            print!(
                                "\r\x1b[K{}╰─{}❯{} {}",
                                C_BORDER, C_GREEN, C_RESET, buf
                            );
                        }
                    }
                }
                println!();
                return Some(buf);
            }

            // Tab (9) -> Autocomplete highlighted slash command
            if ch == 9 {
                let trimmed = buf.trim_start();
                if trimmed.starts_with('/') && !trimmed.contains(' ') {
                    let q = trimmed.to_lowercase();
                    let matches: Vec<_> = cli_slash_catalog()
                        .iter()
                        .filter(|(cmd, _, _, _)| {
                            q == "/"
                                || cmd.starts_with(&q)
                                || cmd.contains(q.trim_start_matches('/'))
                        })
                        .take(8)
                        .collect();
                    if let Some(&(sel_cmd, _, _, takes_args)) =
                        matches.get(sel_idx).or_else(|| matches.first())
                    {
                        buf = if *takes_args {
                            format!("{} ", sel_cmd)
                        } else {
                            sel_cmd.to_string()
                        };
                        sel_idx = 0;
                        print!(
                            "\r\x1b[K{}╰─{}❯{} {}",
                            C_BORDER, C_GREEN, C_RESET, buf
                        );
                        let _ = io::stdout().flush();
                        render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
                    }
                }
                continue;
            }

            // Escape (27) -> Clear menu or input
            if ch == 27 {
                clear_menu(&mut menu_lines_drawn);
                buf.clear();
                sel_idx = 0;
                print!("\r\x1b[K{}╰─{}❯{} ", C_BORDER, C_GREEN, C_RESET);
                let _ = io::stdout().flush();
                continue;
            }

            // Backspace (8 or 127)
            if ch == 8 || ch == 127 {
                if buf.pop().is_some() {
                    sel_idx = 0;
                    print!(
                        "\r\x1b[K{}╰─{}❯{} {}",
                        C_BORDER, C_GREEN, C_RESET, buf
                    );
                    let _ = io::stdout().flush();
                    render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
                }
                continue;
            }

            if let Some(c) = char::from_u32(ch as u32) {
                if !c.is_control() {
                    buf.push(c);
                    sel_idx = 0;
                    print!("{}", c);
                    let _ = io::stdout().flush();
                    render_menu(&buf, &mut sel_idx, &mut menu_lines_drawn);
                }
            }
        }
    }
}

async fn run_repl(hm: Arc<HarnessManager>) {
    print_banner(&hm);

    loop {
        let settings = hm.settings();
        let ws_path = hm.active_workspace_path();
        let ws_name = ws_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| ws_path.to_string_lossy().to_string());
        let git_info = Workspace::new(&ws_path)
            .map(|w| w.git_info())
            .unwrap_or_default();
        let git_badge = if git_info.branch.is_empty() {
            String::new()
        } else {
            let dirty = if git_info.dirty { "*" } else { "" };
            format!(" {}({}{}){}", C_YELLOW, git_info.branch, dirty, C_RESET)
        };

        let running_tasks = SchedulerManager::list_live_tasks()
            .into_iter()
            .filter(|t| t.status == "running")
            .count();
        let tasks_badge = if running_tasks > 0 {
            format!(" {}[⏰ {} active]{}", C_GREEN, running_tasks, C_RESET)
        } else {
            String::new()
        };

        print!(
            "\n{}╭─{} {}[📂 {}]{}{}{} {}[{} │ {}{}{} │ {}{}{}]{}\n{}╰─{}❯{} ",
            C_BORDER,
            C_RESET,
            C_BLUE,
            ws_name,
            C_RESET,
            git_badge,
            tasks_badge,
            C_GRAY,
            mode_badge(&hm.agent_mode()),
            C_CYAN,
            settings.model,
            C_GRAY,
            C_MAGENTA,
            settings.reasoning_effort,
            C_GRAY,
            C_RESET,
            C_BORDER,
            C_GREEN,
            C_RESET
        );
        let _ = io::stdout().flush();

        let Some(line) = read_interactive_tui_line() else {
            break;
        };
        if !handle_repl_line(&hm, &line).await {
            break;
        }
    }
}
