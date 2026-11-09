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

fn stop_running_processes(target_dir: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for proc_name in ["Unjess.exe", "unjessd.exe", "unjess-cli.exe", "unjess.exe"] {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/IM", proc_name])
                .creation_flags(0x08000000)
                .status();
        }

        // Wait up to 5 seconds for target executables to be fully unlocked by Windows
        let exes = ["Unjess.exe", "unjessd.exe", "unjess-cli.exe"];
        for _ in 0..25 {
            let mut all_unlocked = true;
            for name in &exes {
                let p = target_dir.join(name);
                if p.exists() {
                    match fs::OpenOptions::new().write(true).open(&p) {
                        Ok(_) => {},
                        Err(_) => {
                            all_unlocked = false;
                            break;
                        }
                    }
                }
            }
            if all_unlocked {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

fn safe_write_or_copy(src: Option<&Path>, bytes: Option<&[u8]>, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // If dst exists and is locked, try renaming to .old first
    let old_backup = dst.with_extension("old");
    let _ = fs::remove_file(&old_backup);
    if dst.exists() {
        let _ = fs::rename(dst, &old_backup);
    }

    let mut last_err = None;
    for _ in 0..10 {
        let res = if let Some(s) = src {
            fs::copy(s, dst).map(|_| ()).map_err(|e| e.to_string())
        } else if let Some(b) = bytes {
            fs::write(dst, b).map_err(|e| e.to_string())
        } else {
            return Err("No source or bytes provided".to_string());
        };
        match res {
            Ok(_) => {
                let _ = fs::remove_file(&old_backup);
                return Ok(());
            }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "Unknown error".to_string()))
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

fn download_file(url: &str, dst: &Path) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent("Unjess-Installer/4.1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let mut last_err = None;
    for _attempt in 0..3 {
        let resp = match client.get(url).send() {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(e.to_string());
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
        };
        if !resp.status().is_success() {
            last_err = Some(format!("HTTP {}", resp.status()));
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                last_err = Some(e.to_string());
                continue;
            }
        };
        return safe_write_or_copy(None, Some(&bytes), dst);
    }
    Err(last_err.unwrap_or_else(|| "Failed after 3 attempts".to_string()))
}

fn main() {
    println!("====================================================");
    println!(
        "  Unjess {} (100% Native Rust Engine) — Setup",
        CURRENT_VERSION
    );
    println!("====================================================");

    let target_dir = install_dir();
    stop_running_processes(&target_dir);
    if let Err(e) = fs::create_dir_all(&target_dir) {
        eprintln!(
            "[!] Failed to create install directory {}: {}",
            target_dir.display(),
            e
        );
        std::thread::sleep(std::time::Duration::from_secs(4));
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
        "version.json",
    ];

    let mut installed_count = 0usize;
    for bin_name in binaries {
        let candidates = [
            exe_dir.join(bin_name),
            exe_dir.join("dist").join(bin_name),
            PathBuf::from("dist").join(bin_name),
            PathBuf::from("target").join("release").join(bin_name),
        ];
        let dst = target_dir.join(bin_name);

        let mut found_src = None;
        for c in candidates {
            if c.exists() {
                if let (Ok(can_c), Ok(can_d)) = (c.canonicalize(), dst.canonicalize()) {
                    if can_c == can_d {
                        continue;
                    }
                }
                found_src = Some(c);
                break;
            }
        }

        if let Some(src) = found_src {
            match safe_write_or_copy(Some(&src), None, &dst) {
                Ok(_) => {
                    installed_count += 1;
                    println!("[+] Installed {} -> {}", bin_name, dst.display());
                }
                Err(e) => eprintln!("[!] Could not copy {}: {}", bin_name, e),
            }
        } else {
            // Attempt to download directly from GitHub raw dist if not found locally
            println!("[*] Downloading {} from GitHub...", bin_name);
            let dl_url = format!(
                "https://github.com/Bexx1107/Unjess-Rust/raw/main/dist/{}",
                bin_name
            );
            match download_file(&dl_url, &dst) {
                Ok(_) => {
                    installed_count += 1;
                    println!("[+] Downloaded & installed {} -> {}", bin_name, dst.display());
                }
                Err(e) => {
                    println!(
                        "[*] Note: {} not found locally and download skipped ({})",
                        bin_name, e
                    );
                }
            }
        }
    }

    if installed_count == 0 && !target_dir.join("Unjess.exe").exists() {
        eprintln!(
            "[!] Error: No Unjess binaries were found next to installer ({}) and could not be fetched.",
            exe_dir.display()
        );
        println!("\nPress Enter to exit...");
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
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
            let legacy_root = PathBuf::from(local_app_data)
                .join("Programs")
                .join("Unjess");
            if legacy_root.exists() {
                for name in [
                    "Unjess.exe",
                    "unjessd.exe",
                    "unjess-cli.exe",
                    "Unjess-Uninstall.exe",
                    "version.json",
                ] {
                    let src = target_dir.join(name);
                    if src.exists() {
                        let _ = fs::copy(&src, legacy_root.join(name));
                    }
                }
                let legacy_bin_dir = legacy_root.join("bin");
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
            "{{\n  \"version\": \"{}\",\n  \"engine\": \"100% Native Rust (eframe/egui + Axum + Tokio)\",\n  \"port\": 9758,\n  \"binaries\": {{\n{}\n  }}\n}}\n",
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

    if !env::args().any(|a| a == "--no-launch") {
        println!("[+] Spawning Unjess GUI...");
        match Command::new(&desktop_exe)
            .current_dir(&target_dir)
            .spawn()
        {
            Ok(child) => {
                println!("[+] Unjess GUI successfully launched (PID: {})", child.id());
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
            Err(e) => {
                eprintln!("[!] Warning: Could not auto-launch Unjess GUI: {}", e);
                println!("    Please launch Unjess from your Desktop shortcut or: {}", desktop_exe.display());
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
}
