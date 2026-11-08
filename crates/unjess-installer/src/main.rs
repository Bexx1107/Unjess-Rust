use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use unjess_core::config::Settings;
use unjess_core::version::CURRENT_VERSION;

fn install_dir() -> PathBuf {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        PathBuf::from(local_app_data)
            .join("Programs")
            .join("UnjessRust")
    } else if let Some(dir) = dirs::data_local_dir() {
        dir.join("Programs").join("UnjessRust")
    } else {
        PathBuf::from(r"C:\UnjessRust")
    }
}

fn stop_running_processes() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for proc_name in ["Unjess.exe", "unjessd.exe", "unjess-cli.exe"] {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/IM", proc_name])
                .creation_flags(0x08000000)
                .status();
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn escape_ps_single_quotes(s: &str) -> String {
    s.replace('\'', "''")
}

fn create_shortcut(shortcut_path: &Path, target_exe: &Path, working_dir: &Path) {
    let lnk = escape_ps_single_quotes(&shortcut_path.to_string_lossy());
    let exe = escape_ps_single_quotes(&target_exe.to_string_lossy());
    let wd = escape_ps_single_quotes(&working_dir.to_string_lossy());
    let ps_script = format!(
        "$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut('{}'); $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; $s.Description = 'Unjess {} Autonomous AI Coding Harness'; $s.Save()",
        lnk,
        exe,
        wd,
        exe,
        CURRENT_VERSION
    );
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &ps_script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let _ = cmd.status();
}

fn main() {
    println!("====================================================");
    println!(
        "  Unjess {} (100% Native Rust Engine) — Setup",
        CURRENT_VERSION
    );
    println!("====================================================");

    stop_running_processes();

    let target_dir = install_dir();
    if let Err(e) = fs::create_dir_all(&target_dir) {
        eprintln!(
            "[!] Failed to create install directory {}: {}",
            target_dir.display(),
            e
        );
        std::process::exit(1);
    }
    println!("[+] Created install directory: {}", target_dir.display());

    // Locate sibling binaries next to Unjess-Setup.exe or in current working directory
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let binaries = [
        "Unjess.exe",
        "unjessd.exe",
        "unjess-cli.exe",
        "Unjess-Uninstall.exe",
        "WebView2Loader.dll",
        "version.json",
    ];

    let mut installed_count = 0usize;
    for bin_name in binaries {
        let candidates = [
            exe_dir.join(bin_name),
            PathBuf::from("dist").join(bin_name),
            PathBuf::from("target").join("release").join(bin_name),
            PathBuf::from(bin_name),
        ];
        if let Some(src) = candidates.iter().find(|p| p.exists()) {
            let dst = target_dir.join(bin_name);
            if src != &dst {
                match fs::copy(src, &dst) {
                    Ok(_) => {
                        installed_count += 1;
                        println!("[+] Installed {} -> {}", bin_name, dst.display());
                    }
                    Err(e) => eprintln!("[!] Could not copy {}: {}", bin_name, e),
                }
            } else {
                installed_count += 1;
            }
        } else {
            println!(
                "[*] Note: {} not found next to installer (skipping copy)",
                bin_name
            );
        }
    }

    if installed_count == 0 && !target_dir.join("Unjess.exe").exists() {
        eprintln!(
            "[!] Error: No Unjess binaries were found next to installer ({}). Run build-all.ps1 first.",
            exe_dir.display()
        );
        std::process::exit(1);
    }

    // Install CLI alias `bin\unjess.exe` so `unjess` in cmd.exe / PowerShell launches the Rust CLI
    let cli_bin = target_dir.join("unjess-cli.exe");
    if cli_bin.exists() {
        let bin_subdir = target_dir.join("bin");
        if fs::create_dir_all(&bin_subdir).is_ok() {
            let alias_dst = bin_subdir.join("unjess.exe");
            let _ = fs::copy(&cli_bin, &alias_dst);
            println!("[+] Installed CLI command alias -> {}", alias_dst.display());
        }
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            let legacy_bin_dir = PathBuf::from(local_app_data)
                .join("Programs")
                .join("Unjess")
                .join("bin");
            if legacy_bin_dir.exists() {
                let legacy_cli = legacy_bin_dir.join("unjess.exe");
                if fs::copy(&cli_bin, &legacy_cli).is_ok() {
                    println!(
                        "[+] Updated PATH CLI binary -> {}",
                        legacy_cli.display()
                    );
                }
            }
        }
    }

    // Generate version.json with real SHA-256 hashes if not already present in target_dir
    let version_json_path = target_dir.join("version.json");
    if !version_json_path.exists() {
        let mut hash_entries = Vec::new();
        for name in [
            "Unjess.exe",
            "unjessd.exe",
            "unjess-cli.exe",
            "Unjess-Uninstall.exe",
        ] {
            let p = target_dir.join(name);
            if let Ok(bytes) = fs::read(&p) {
                if let Ok(sha) = unjess_core::updater::verify_sha256_bytes(&bytes, "") {
                    hash_entries.push(format!("    \"{}\": \"{}\"", name, sha));
                }
            }
        }
        let manifest = format!(
            "{{\n  \"version\": \"{}\",\n  \"engine\": \"100% Native Rust (WebView2 + Axum + Tokio)\",\n  \"port\": 9758,\n  \"hashes\": {{\n{}\n  }}\n}}\n",
            CURRENT_VERSION,
            hash_entries.join(",\n")
        );
        let _ = fs::write(&version_json_path, manifest);
    }

    // Initialize default settings if not present
    let settings = Settings::load();
    let _ = settings.save();
    println!("[+] Initialized Unjess Rust configuration.");

    // Create Desktop & Start Menu shortcuts
    let desktop_exe = target_dir.join("Unjess.exe");
    if let Some(desktop) = dirs::desktop_dir() {
        let lnk = desktop.join("Unjess 4.0 (Native Rust).lnk");
        create_shortcut(&lnk, &desktop_exe, &target_dir);
        println!("[+] Created Desktop shortcut: {}", lnk.display());
    }

    if let Ok(appdata) = env::var("APPDATA") {
        let start_menu = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Unjess 4.0 (Native Rust).lnk");
        create_shortcut(&start_menu, &desktop_exe, &target_dir);
        println!("[+] Created Start Menu shortcut: {}", start_menu.display());
    }

    println!("====================================================");
    println!(
        "  Installation Complete! Installed to: {}",
        target_dir.display()
    );
    println!("====================================================");
}
