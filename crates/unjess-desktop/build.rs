fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../assets/unjess_icon.ico");
        res.set("ProductName", "Unjess");
        res.set("FileDescription", "Unjess");
        res.set("CompanyName", "Unjess");
        res.set("LegalCopyright", "Copyright (c) 2026 Unjess. All rights reserved.");
        res.set("FileVersion", "4.0.0.0");
        res.set("ProductVersion", "4.0.0.0");
        res.set("OriginalFilename", "Unjess.exe");
        res.set("InternalName", "Unjess");
        res.set_windres_path("x86_64-w64-mingw32-windres");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=Failed to compile Windows resources: {}", e);
        }
        let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
        let o_path = std::path::Path::new(&out_dir).join("resource.o");
        if o_path.is_file() {
            println!("cargo:rustc-link-arg-bins={}", o_path.display());
        }
    }
}
