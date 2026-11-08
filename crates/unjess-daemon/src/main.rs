use clap::Parser;
use std::sync::Arc;
use unjess_core::agent::HarnessManager;
use unjess_core::api::start_api_server;
use unjess_core::version::CURRENT_VERSION;

#[derive(Parser, Debug)]
#[command(
    name = "unjessd",
    version = CURRENT_VERSION,
    about = "Unjess 4.0 Native Rust Autonomous AI Coding & Security Daemon"
)]
struct DaemonArgs {
    /// Host address to bind the HTTP/REST API server
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind the HTTP/REST API server (defaults to 9758)
    #[arg(short, long, default_value_t = 9758)]
    port: u16,

    /// Optional workspace directory to activate on startup
    #[arg(short, long)]
    workspace: Option<String>,

    /// Verify daemon configuration and workspace readiness, then exit 0
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = DaemonArgs::parse();
    let hm = Arc::new(HarnessManager::new());
    if let Some(ws) = &args.workspace {
        let id = hm.add_workspace(ws);
        hm.set_active_workspace(&id);
    }
    let enabled_schedules = unjess_core::scheduler::SchedulerManager::load()
        .into_iter()
        .filter(|s| s.enabled)
        .count();

    if args.check {
        println!(
            "Unjess {} daemon check OK — workspace={} schedules={}",
            CURRENT_VERSION,
            hm.active_workspace_path().display(),
            enabled_schedules
        );
        return Ok(());
    }

    println!(
        "============================================================\n  Unjess {} — Native Rust Autonomous Daemon (unjessd)\n  Listening on http://{}:{}\n  Active Workspace: {}\n  Enabled Schedules: {}\n============================================================",
        CURRENT_VERSION,
        args.host,
        args.port,
        hm.active_workspace_path().display(),
        enabled_schedules
    );

    unjess_core::scheduler::SchedulerManager::start_background_ticker(Arc::clone(&hm));
    tokio::select! {
        res = start_api_server(hm, &args.host, args.port) => {
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\n[*] Received Ctrl+C — shutting down Unjess Rust Daemon cleanly.");
        }
    }
    Ok(())
}
