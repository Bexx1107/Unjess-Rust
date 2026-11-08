use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

fn main() {
    println!("====================================================");
    println!("  Unjess 4.0 (Native Rust) — Uninstaller");
    println!("====================================================");

    stop_running_processes();

    let target_dir = install_dir();
    println!("[*] Target install directory: {}", target_dir.display());

    // Remove Desktop and Start Menu shortcuts if present
    if let Some(desktop) = dirs::desktop_dir() {
        let shortcut = desktop.join("Unjess 4.0 (Native Rust).lnk");
        if shortcut.exists() {
            let _ = fs::remove_file(&shortcut);
            println!("[+] Removed Desktop shortcut: {}", shortcut.display());
        }
    }

    if let Ok(appdata) = env::var("APPDATA") {
        let start_menu = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Unjess 4.0 (Native Rust).lnk");
        if start_menu.exists() {
            let _ = fs::remove_file(&start_menu);
            println!("[+] Removed Start Menu shortcut: {}", start_menu.display());
        }
    }

    // Remove binaries in target directory
    for bin_name in [
        "Unjess.exe",
        "unjessd.exe",
        "unjess-cli.exe",
        "Unjess-Uninstall.exe",
        "WebView2Loader.dll",
        "version.json",
    ] {
        let p = target_dir.join(bin_name);
        if p.exists() {
            match fs::remove_file(&p) {
                Ok(_) => println!("[+] Removed {}", p.display()),
                Err(e) => eprintln!("[!] Could not remove {}: {}", p.display(), e),
            }
        }
    }

    // Also clean up isolated WebView2 data directory in %LOCALAPPDATA%\UnjessRust
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let wv_dir = PathBuf::from(local_app_data).join("UnjessRust");
        if wv_dir.exists() {
            let _ = fs::remove_dir_all(&wv_dir);
            println!("[+] Removed WebView cache: {}", wv_dir.display());
        }
    }

    // Schedule self-cleanup if running from inside target_dir
    if target_dir.exists() {
        let mut cmd = Command::new("cmd");
        cmd.args([
            "/C",
            &format!(
                "ping 127.0.0.1 -n 2 > nul & rmdir /s /q \"{}\"",
                target_dir.display()
            ),
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let _ = cmd.spawn();
    }

    println!("[+] Unjess 4.0 (Native Rust) uninstallation complete.");
}
