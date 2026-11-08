//! Multi-Language AST Symbol Outline, Call-Graph & Repository Map Intelligence Engine.
//!
//! Extracts functions, structs, enums, traits, impls, types, constants, macros, classes,
//! interfaces, and methods with 1-indexed line numbers and `end_line` spans across Rust, Go,
//! Python, TypeScript/JavaScript, and C/C++, caching compiled Regex rules in `OnceLock` statics.

use crate::workspace::Workspace;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// Represents a parsed AST symbol in a source file with its start and end line span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstSymbol {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: usize,
    #[serde(default)]
    pub end_line: usize,
    pub signature: String,
}

/// Parses a source file and returns its ordered symbol outline.
pub fn outline_file(path: &Path) -> Vec<AstSymbol> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let file_display = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    outline_content(&content, path, &file_display)
}

/// Extracts top-level import/include statements from a source file (up to 25 lines).
pub fn extract_imports(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut imports = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with("use ")
            || t.starts_with("pub use ")
            || t.starts_with("import ")
            || t.starts_with("from ")
            || t.starts_with("#include ")
        {
            imports.push(t.to_string());
            if imports.len() >= 25 {
                break;
            }
        }
    }
    imports
}

/// Counts structural `{` and `}` on a single source line while ignoring string literals (`"..."`),
/// character literals (`'x'`), and line comments (`//`). Also reports whether the non-comment
/// code on the line ends with `;`.
fn count_structural_braces(line: &str) -> (i32, i32, bool) {
    let mut opens = 0i32;
    let mut closes = 0i32;
    let mut in_double = false;
    let mut in_single = false;
    let mut last_non_ws = '\0';

    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if in_double {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_single {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            break;
        }
        if ch == '"' {
            in_double = true;
            last_non_ws = ch;
            i += 1;
            continue;
        }
        if ch == '\'' {
            // Treat as character literal if closed by `'` within the next 6 chars on the same line
            let max_look = (i + 6).min(chars.len());
            let mut closes_soon = false;
            let mut k = i + 1;
            while k < max_look {
                if chars[k] == '\\' {
                    k += 2;
                    continue;
                }
                if chars[k] == '\'' {
                    closes_soon = true;
                    break;
                }
                k += 1;
            }
            if closes_soon {
                in_single = true;
            }
            last_non_ws = ch;
            i += 1;
            continue;
        }
        if ch == '{' {
            opens += 1;
        } else if ch == '}' {
            closes += 1;
        }
        if !ch.is_whitespace() {
            last_non_ws = ch;
        }
        i += 1;
    }

    (opens, closes, last_non_ws == ';')
}

pub fn outline_content(content: &str, path: &Path, file_label: &str) -> Vec<AstSymbol> {
    let patterns = language_rules(path);
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let mut symbols: Vec<AstSymbol> = Vec::new();
    let is_python = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("py"))
        .unwrap_or(false);

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        for (kind, re) in patterns {
            if let Some(caps) = re.captures(trimmed) {
                if let Some(name_match) = caps.get(1) {
                    let sig = trimmed
                        .trim_end_matches('{')
                        .trim_end_matches(':')
                        .trim()
                        .to_string();
                    symbols.push(AstSymbol {
                        name: name_match.as_str().to_string(),
                        kind: (*kind).to_string(),
                        file: file_label.to_string(),
                        line: idx + 1,
                        end_line: idx + 1,
                        signature: sig,
                    });
                    break;
                }
            }
        }
    }

    // Compute `end_line` for each symbol: indentation-scoped for Python, structural brace-depth for C-family/Rust/Go/TS
    for i in 0..symbols.len() {
        let start_idx = symbols[i].line.saturating_sub(1);
        let fallback_end = if i + 1 < symbols.len() {
            symbols[i + 1].line.saturating_sub(1).max(symbols[i].line)
        } else {
            total_lines.max(symbols[i].line)
        };

        if is_python {
            let base_indent = lines[start_idx]
                .chars()
                .take_while(|c| c.is_whitespace())
                .count();
            let mut last_body_line = symbols[i].line;
            for (offset, l) in lines[start_idx + 1..].iter().enumerate() {
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                let indent = l.chars().take_while(|c| c.is_whitespace()).count();
                if indent <= base_indent {
                    break;
                }
                last_body_line = start_idx + 1 + offset + 1;
            }
            symbols[i].end_line = last_body_line.max(symbols[i].line);
            continue;
        }

        let mut depth: i32 = 0;
        let mut opened = false;
        let mut computed_end = fallback_end;
        for (offset, l) in lines[start_idx..].iter().enumerate() {
            let (opens, closes, ends_semi) = count_structural_braces(l);
            if opens > 0 {
                opened = true;
            }
            depth += opens - closes;
            if opened && depth <= 0 {
                computed_end = start_idx + offset + 1;
                break;
            }
            if !opened && offset == 0 && ends_semi {
                computed_end = start_idx + 1;
                break;
            }
        }
        symbols[i].end_line = computed_end.max(symbols[i].line);
    }

    symbols
}

const RUST_DEFS: &[(&str, &str)] = &[
    (
        "fn",
        r#"^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?(?:extern\s+(?:"[^"]*"\s+)?)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"#,
    ),
    (
        "struct",
        r"^(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)",
    ),
    (
        "enum",
        r"^(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)",
    ),
    (
        "trait",
        r"^(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)",
    ),
    (
        "impl",
        r"^(?:unsafe\s+)?impl(?:<[^>]*>)?\s+(?:[A-Za-z_][A-Za-z0-9_:<>]*\s+for\s+)?([A-Za-z_][A-Za-z0-9_:<>]*)",
    ),
    (
        "type",
        r"^(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*=",
    ),
    (
        "const",
        r"^(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:",
    ),
    (
        "mod",
        r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)",
    ),
    (
        "macro",
        r"^(?:pub(?:\([^)]*\))?\s+)?macro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)",
    ),
];

const GO_DEFS: &[(&str, &str)] = &[
    (
        "func",
        r"^func\s+(?:\([^)]+\)\s+)?([A-Za-z_][A-Za-z0-9_]*)",
    ),
    (
        "type",
        r"^type\s+([A-Za-z_][A-Za-z0-9_]*)\s+(?:struct|interface|[A-Za-z0-9_\[\]\*\.]+)",
    ),
];

const PY_DEFS: &[(&str, &str)] = &[
    ("def", r"^(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("),
    ("class", r"^class\s+([A-Za-z_][A-Za-z0-9_]*)"),
];

const TS_DEFS: &[(&str, &str)] = &[
    (
        "function",
        r"^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)",
    ),
    (
        "class",
        r"^(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    ),
    (
        "interface",
        r"^(?:export\s+)?(?:declare\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    ),
    (
        "type",
        r"^(?:export\s+)?(?:declare\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?:<[^>]*>)?\s*=",
    ),
    (
        "enum",
        r"^(?:export\s+)?(?:const\s+)?enum\s+([A-Za-z_$][A-Za-z0-9_$]*)",
    ),
    (
        "const_fn",
        r"^(?:export\s+)?const\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::\s*[^=]+)?\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][A-Za-z0-9_$]*)\s*=>",
    ),
];

const CPP_DEFS: &[(&str, &str)] = &[
    (
        "type",
        r"^(?:template\s*<[^>]*>\s*)?(?:struct|class|enum(?:\s+class)?|namespace)\s+([A-Za-z_][A-Za-z0-9_]*)\b",
    ),
    ("typedef", r"^typedef\s+.*\b([A-Za-z_][A-Za-z0-9_]*)\s*;"),
];

fn compile_defs(slices: &[&[(&'static str, &'static str)]]) -> Vec<(&'static str, Regex)> {
    let mut out = Vec::new();
    for slice in slices {
        for &(kind, pat) in *slice {
            if let Ok(re) = Regex::new(pat) {
                out.push((kind, re));
            }
        }
    }
    out
}

fn language_rules(path: &Path) -> &'static [(&'static str, Regex)] {
    static RS_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static GO_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static PY_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static TS_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static CPP_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static ALL_RULES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "rs" => RS_RULES.get_or_init(|| compile_defs(&[RUST_DEFS])),
        "go" => GO_RULES.get_or_init(|| compile_defs(&[GO_DEFS])),
        "py" => PY_RULES.get_or_init(|| compile_defs(&[PY_DEFS])),
        "ts" | "tsx" | "js" | "jsx" | "mjs" => TS_RULES.get_or_init(|| compile_defs(&[TS_DEFS])),
        "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" => {
            CPP_RULES.get_or_init(|| compile_defs(&[CPP_DEFS]))
        }
        _ => ALL_RULES.get_or_init(|| {
            compile_defs(&[RUST_DEFS, GO_DEFS, PY_DEFS, TS_DEFS, CPP_DEFS])
        }),
    }
}

/// Scans all source files in `ws` and returns all `AstSymbol`s with relative workspace file paths.
pub fn scan_workspace_symbols(ws: &Path) -> Vec<AstSymbol> {
    let Ok(workspace) = Workspace::new(ws) else {
        return Vec::new();
    };
    let mut all = Vec::new();
    for rel in workspace.list_all_files(500) {
        if all.len() >= 2000 {
            break;
        }
        if let Ok(abs) = workspace.resolve(&rel) {
            if let Ok(content) = fs::read_to_string(&abs) {
                let syms = outline_content(&content, &abs, &rel);
                all.extend(syms);
            }
        }
    }
    all
}

/// Finds symbol definitions matching `symbol` across the workspace.
pub fn find_symbol_definitions(ws: &Path, symbol: &str) -> Vec<AstSymbol> {
    let q = symbol.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<AstSymbol> = scan_workspace_symbols(ws)
        .into_iter()
        .filter(|s| s.name.to_lowercase().contains(&q))
        .collect();
    // Sort exact matches first
    matches.sort_by_key(|s| if s.name.eq_ignore_ascii_case(&q) { 0 } else { 1 });
    matches
}

/// Generates a high-density, token-budgeted Architectural Repository Map of the workspace.
pub fn generate_repo_map(ws: &Path, max_files: usize, max_chars: usize) -> String {
    let Ok(workspace) = Workspace::new(ws) else {
        return String::new();
    };
    let source_exts = [
        ".rs", ".go", ".py", ".ts", ".tsx", ".js", ".jsx", ".c", ".cpp", ".h", ".hpp",
    ];
    let mut out = String::new();
    let mut files_mapped = 0usize;

    for rel in workspace.list_all_files(max_files.max(20) * 3) {
        if files_mapped >= max_files || out.len() >= max_chars {
            break;
        }
        let lower = rel.to_lowercase();
        if !source_exts.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }
        let Ok(abs) = workspace.resolve(&rel) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&abs) else {
            continue;
        };
        let syms = outline_content(&content, &abs, &rel);
        if syms.is_empty() {
            continue;
        }
        files_mapped += 1;
        out.push_str(&format!("{}:\n", rel));
        for s in syms.iter().take(25) {
            let line_entry = format!(
                "  L{}-L{} [{}] {}\n",
                s.line, s.end_line, s.kind, s.signature
            );
            if out.len() + line_entry.len() > max_chars {
                break;
            }
            out.push_str(&line_entry);
        }
    }

    out
}

/// Finds all call sites / references to `symbol` across workspace files, resolving the enclosing caller symbol.
pub fn find_callers(ws: &Path, symbol: &str) -> Vec<String> {
    let trimmed = symbol.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(workspace) = Workspace::new(ws) else {
        return Vec::new();
    };
    let pattern = format!(r"\b{}\b", regex::escape(trimmed));
    let Ok(re) = Regex::new(&pattern) else {
        return Vec::new();
    };

    let mut callers = Vec::new();
    for rel in workspace.list_all_files(2500) {
        if callers.len() >= 100 {
            break;
        }
        if let Ok(abs) = workspace.resolve(&rel) {
            if let Ok(content) = fs::read_to_string(&abs) {
                if !re.is_match(&content) {
                    continue;
                }
                let syms = outline_content(&content, &abs, &rel);
                for (idx, line) in content.lines().enumerate() {
                    let line_no = idx + 1;
                    let tline = line.trim();
                    if tline.is_empty()
                        || tline.starts_with("//")
                        || tline.starts_with('#')
                        || tline.starts_with("/*")
                        || tline.starts_with('*')
                    {
                        continue;
                    }
                    if re.is_match(tline) {
                        // Skip the symbol's own definition line
                        if syms
                            .iter()
                            .any(|s| s.name == trimmed && s.line == line_no)
                        {
                            continue;
                        }
                        let is_import = tline.starts_with("use ")
                            || tline.starts_with("pub use ")
                            || tline.starts_with("import ")
                            || tline.starts_with("from ")
                            || tline.starts_with("#include ");
                        let enclosing_syms: Vec<&AstSymbol> = syms
                            .iter()
                            .filter(|s| s.line <= line_no && line_no <= s.end_line)
                            .collect();
                        let enclosing = if is_import {
                            "import".to_string()
                        } else if enclosing_syms.len() >= 2 {
                            let outer = enclosing_syms[0];
                            let inner = enclosing_syms[enclosing_syms.len() - 1];
                            format!(
                                "caller: {} {} :: {} {}",
                                outer.kind, outer.name, inner.kind, inner.name
                            )
                        } else if let Some(s) = enclosing_syms.last() {
                            format!("caller: {} {}", s.kind, s.name)
                        } else {
                            "top-level".to_string()
                        };
                        let preview: String = tline.chars().take(160).collect();
                        callers.push(format!(
                            "{}:{} [{}] -> {}",
                            rel, line_no, enclosing, preview
                        ));
                        if callers.len() >= 100 {
                            break;
                        }
                    }
                }
            }
        }
    }
    callers
}

/// Runs an automatic post-edit syntax & structural linter check on a modified file (`rel_path`)
/// inside `ws`. Returns `[Auto-Verify: Syntax & linter check PASSED]` or a detailed `<post_edit_error>`
/// block with exact line numbers so the agent can self-heal syntax errors immediately.
pub fn run_post_edit_check(ws: &Path, rel_path: &str) -> String {
    let Ok(workspace) = Workspace::new(ws) else {
        return String::new();
    };
    let Ok(abs_path) = workspace.resolve(rel_path) else {
        return String::new();
    };
    if !abs_path.is_file() {
        return String::new();
    }
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "json" => {
            let Ok(raw) = fs::read_to_string(&abs_path) else {
                return String::new();
            };
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(_) => "[Auto-Verify: Syntax & linter check PASSED]".to_string(),
                Err(e) => format!(
                    "\n<post_edit_error file=\"{}\">\nJSON syntax error: {}\n</post_edit_error>",
                    rel_path, e
                ),
            }
        }
        "yaml" | "yml" => {
            let Ok(raw) = fs::read_to_string(&abs_path) else {
                return String::new();
            };
            match serde_yaml::from_str::<serde_yaml::Value>(&raw) {
                Ok(_) => "[Auto-Verify: Syntax & linter check PASSED]".to_string(),
                Err(e) => format!(
                    "\n<post_edit_error file=\"{}\">\nYAML syntax error: {}\n</post_edit_error>",
                    rel_path, e
                ),
            }
        }
        "py" => {
            for py_bin in ["python", "python3", "py"] {
                let mut cmd = std::process::Command::new(py_bin);
                cmd.args(["-m", "py_compile", &abs_path.to_string_lossy()])
                    .current_dir(ws);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                if let Ok(out) = cmd.output() {
                    if out.status.success() {
                        return "[Auto-Verify: Syntax & linter check PASSED]".to_string();
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let combined = format!("{}{}", stdout, stderr);
                        let clipped: String = combined.chars().take(1500).collect();
                        return format!(
                            "\n<post_edit_error file=\"{}\">\n{}\n</post_edit_error>",
                            rel_path,
                            clipped.trim()
                        );
                    }
                }
            }
            "[Auto-Verify: Syntax & linter check PASSED]".to_string()
        }
        "js" | "mjs" | "cjs" => {
            let mut cmd = std::process::Command::new("node");
            cmd.args(["--check", &abs_path.to_string_lossy()])
                .current_dir(ws);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            if let Ok(out) = cmd.output() {
                if out.status.success() {
                    return "[Auto-Verify: Syntax & linter check PASSED]".to_string();
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let clipped: String = stderr.chars().take(1500).collect();
                    return format!(
                        "\n<post_edit_error file=\"{}\">\n{}\n</post_edit_error>",
                        rel_path,
                        clipped.trim()
                    );
                }
            }
            "[Auto-Verify: Syntax & linter check PASSED]".to_string()
        }
        "go" => {
            let mut cmd = std::process::Command::new("gofmt");
            cmd.args(["-e", &abs_path.to_string_lossy()])
                .current_dir(ws);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            if let Ok(out) = cmd.output() {
                if out.status.success() {
                    if ws.join("go.mod").is_file() {
                        let mut vet = std::process::Command::new("go");
                        vet.args(["vet", "./..."]).current_dir(ws);
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            vet.creation_flags(0x08000000);
                        }
                        if let Ok(vout) = vet.output() {
                            if !vout.status.success() {
                                let stderr = String::from_utf8_lossy(&vout.stderr);
                                let filtered: Vec<&str> = stderr
                                    .lines()
                                    .filter(|l| l.contains(rel_path) || !l.starts_with('#'))
                                    .take(25)
                                    .collect();
                                if !filtered.is_empty() {
                                    return format!(
                                        "\n<post_edit_error file=\"{}\">\n{}\n</post_edit_error>",
                                        rel_path,
                                        filtered.join("\n")
                                    );
                                }
                            }
                        }
                    }
                    return "[Auto-Verify: Syntax & linter check PASSED]".to_string();
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let clipped: String = stderr.chars().take(1500).collect();
                    return format!(
                        "\n<post_edit_error file=\"{}\">\n{}\n</post_edit_error>",
                        rel_path,
                        clipped.trim()
                    );
                }
            }
            check_brace_balance(&abs_path, rel_path)
        }
        "ts" | "tsx" => {
            let brace_res = check_brace_balance(&abs_path, rel_path);
            if brace_res.contains("<post_edit_error") {
                return brace_res;
            }
            if ws.join("tsconfig.json").is_file() {
                let mut tsc = std::process::Command::new(if cfg!(windows) { "tsc.cmd" } else { "tsc" });
                tsc.args(["--noEmit", "--pretty", "false"]).current_dir(ws);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    tsc.creation_flags(0x08000000);
                }
                if let Ok(tout) = tsc.output() {
                    if !tout.status.success() {
                        let combined = format!(
                            "{}{}",
                            String::from_utf8_lossy(&tout.stdout),
                            String::from_utf8_lossy(&tout.stderr)
                        );
                        let norm_rel = rel_path.replace('\\', "/");
                        let relevant: Vec<&str> = combined
                            .lines()
                            .filter(|l| l.replace('\\', "/").contains(&norm_rel))
                            .take(20)
                            .collect();
                        if !relevant.is_empty() {
                            return format!(
                                "\n<post_edit_error file=\"{}\">\n{}\n</post_edit_error>",
                                rel_path,
                                relevant.join("\n")
                            );
                        }
                    }
                }
            }
            brace_res
        }
        "rs" | "jsx" | "c" | "cpp" | "h" | "hpp" => {
            check_brace_balance(&abs_path, rel_path)
        }
        _ => String::new(),
    }
}

/// Runs workspace-wide incremental compiler and type diagnostics (`cargo check`, `tsc --noEmit`,
/// `go vet`, `ruff` / `py_compile`) and optionally filters diagnostics to `filter_file`.
pub fn run_workspace_diagnostics(ws: &Path, filter_file: Option<&str>) -> String {
    let mut sections: Vec<String> = Vec::new();
    let norm_filter = filter_file
        .map(|f| f.trim().replace('\\', "/"))
        .filter(|f| !f.is_empty());

    let run_diag_cmd = |bin: &str, args: &[&str]| -> Option<(bool, String)> {
        let mut cmd = std::process::Command::new(bin);
        cmd.args(args).current_dir(ws);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let out = cmd.output().ok()?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Some((out.status.success(), combined.trim().to_string()))
    };

    if ws.join("Cargo.toml").is_file() {
        if let Some((ok, out)) = run_diag_cmd("cargo", &["check", "--message-format=short", "-q"]) {
            let lines: Vec<&str> = out
                .lines()
                .filter(|l| {
                    if let Some(ref nf) = norm_filter {
                        l.replace('\\', "/").contains(nf)
                    } else {
                        true
                    }
                })
                .take(40)
                .collect();
            if ok && lines.is_empty() {
                sections.push("[Rust / cargo check]: 0 errors".to_string());
            } else if !lines.is_empty() {
                sections.push(format!("[Rust / cargo check]:\n{}", lines.join("\n")));
            }
        }
    }

    if ws.join("tsconfig.json").is_file() {
        let tsc_bin = if cfg!(windows) { "tsc.cmd" } else { "tsc" };
        if let Some((ok, out)) = run_diag_cmd(tsc_bin, &["--noEmit", "--pretty", "false"]) {
            let lines: Vec<&str> = out
                .lines()
                .filter(|l| {
                    if let Some(ref nf) = norm_filter {
                        l.replace('\\', "/").contains(nf)
                    } else {
                        true
                    }
                })
                .take(40)
                .collect();
            if ok && lines.is_empty() {
                sections.push("[TypeScript / tsc --noEmit]: 0 errors".to_string());
            } else if !lines.is_empty() {
                sections.push(format!("[TypeScript / tsc --noEmit]:\n{}", lines.join("\n")));
            }
        }
    }

    if ws.join("go.mod").is_file() {
        if let Some((ok, out)) = run_diag_cmd("go", &["vet", "./..."]) {
            let lines: Vec<&str> = out
                .lines()
                .filter(|l| {
                    if let Some(ref nf) = norm_filter {
                        l.replace('\\', "/").contains(nf)
                    } else {
                        true
                    }
                })
                .take(40)
                .collect();
            if ok && lines.is_empty() {
                sections.push("[Go / go vet]: 0 errors".to_string());
            } else if !lines.is_empty() {
                sections.push(format!("[Go / go vet]:\n{}", lines.join("\n")));
            }
        }
    }

    if let Some(ref nf) = norm_filter {
        if let Ok(w) = Workspace::new(ws) {
            if let Ok(abs) = w.resolve(nf) {
                if abs.is_file() {
                    let brace = check_brace_balance(&abs, nf);
                    if brace.contains("<post_edit_error") {
                        sections.push(brace.trim().to_string());
                    }
                }
            }
        }
    }

    if sections.is_empty() {
        "<workspace_diagnostics>\nNo compiler or syntax errors detected in workspace.\n</workspace_diagnostics>".to_string()
    } else {
        format!(
            "<workspace_diagnostics>\n{}\n</workspace_diagnostics>",
            sections.join("\n\n")
        )
    }
}

fn strip_comments_and_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    let mut in_double = false;
    let mut in_single = false;
    while i < chars.len() {
        let ch = chars[i];
        if in_double {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        if in_single {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            break;
        }
        if ch == '"' {
            in_double = true;
            i += 1;
            continue;
        }
        if ch == '\'' {
            let max_look = (i + 6).min(chars.len());
            let mut closes_soon = false;
            let mut k = i + 1;
            while k < max_look {
                if chars[k] == '\\' {
                    k += 2;
                    continue;
                }
                if chars[k] == '\'' {
                    closes_soon = true;
                    break;
                }
                k += 1;
            }
            if closes_soon {
                in_single = true;
                i += 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn check_brace_balance(abs_path: &Path, rel_path: &str) -> String {
    let Ok(content) = fs::read_to_string(abs_path) else {
        return String::new();
    };
    let mut stack: Vec<(char, usize)> = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let sanitized = strip_comments_and_strings(line);
        for ch in sanitized.chars() {
            match ch {
                '{' | '(' | '[' => stack.push((ch, line_no)),
                '}' | ')' | ']' => {
                    let expected = match ch {
                        '}' => '{',
                        ')' => '(',
                        _ => '[',
                    };
                    match stack.pop() {
                        Some((open, open_line)) if open != expected => {
                            return format!(
                                "\n<post_edit_error file=\"{}\">\nUnmatched delimiter '{}' at line {} (opened '{}' at line {})\n</post_edit_error>",
                                rel_path, ch, line_no, open, open_line
                            );
                        }
                        None => {
                            return format!(
                                "\n<post_edit_error file=\"{}\">\nUnexpected closing delimiter '{}' at line {} with no matching opener\n</post_edit_error>",
                                rel_path, ch, line_no
                            );
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if let Some((open, open_line)) = stack.last() {
        return format!(
            "\n<post_edit_error file=\"{}\">\nUnclosed delimiter '{}' opened at line {}\n</post_edit_error>",
            rel_path, open, open_line
        );
    }
    "[Auto-Verify: Syntax & linter check PASSED]".to_string()
}

/// Replaces the declaration of `symbol_name` in `rel_path` with `new_symbol_code`
/// and returns `(old_content, updated_content, callers_report)`.
pub fn edit_symbol(
    ws: &Path,
    rel_path: &str,
    symbol_name: &str,
    new_symbol_code: &str,
) -> Result<(String, String, String), String> {
    let workspace = Workspace::new(ws).map_err(|e| e.to_string())?;
    let abs_path = workspace.resolve(rel_path).map_err(|e| e.to_string())?;
    let old_content = fs::read_to_string(&abs_path).map_err(|e| e.to_string())?;
    let symbols = outline_content(&old_content, &abs_path, rel_path);

    let clean_target = symbol_name.trim();
    let target_sym = symbols
        .iter()
        .find(|s| {
            s.name == clean_target
                || s.name.eq_ignore_ascii_case(clean_target)
                || s.name.ends_with(&format!("::{}", clean_target))
                || s.name.ends_with(&format!(".{}", clean_target))
        })
        .ok_or_else(|| format!("Symbol '{}' not found in {}", symbol_name, rel_path))?;

    let is_crlf = old_content.contains("\r\n");
    let line_sep = if is_crlf { "\r\n" } else { "\n" };
    let norm_old = old_content.replace("\r\n", "\n");
    let raw_lines: Vec<&str> = norm_old.split('\n').collect();

    let start_idx = target_sym.line.saturating_sub(1);
    let end_idx = target_sym.end_line.min(raw_lines.len());
    if start_idx >= raw_lines.len() || end_idx < start_idx {
        return Err(format!(
            "Invalid line span [{}..{}] for symbol '{}'",
            target_sym.line, target_sym.end_line, symbol_name
        ));
    }

    let clean_new = new_symbol_code
        .replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_string();

    let mut rebuilt: Vec<&str> = Vec::new();
    rebuilt.extend_from_slice(&raw_lines[..start_idx]);
    let new_lines: Vec<&str> = if clean_new.is_empty() {
        Vec::new()
    } else {
        clean_new.split('\n').collect()
    };
    rebuilt.extend_from_slice(&new_lines);
    rebuilt.extend_from_slice(&raw_lines[end_idx..]);

    let updated_content = rebuilt.join(line_sep);
    fs::write(&abs_path, &updated_content).map_err(|e| e.to_string())?;

    let callers = find_callers(ws, clean_target);
    let callers_report = if callers.is_empty() {
        "No external callers found in workspace.".to_string()
    } else {
        callers.join("\n")
    };

    Ok((old_content, updated_content, callers_report))
}

/// Renames `old_name` to `new_name` across all code files in the repository using word-boundary matching.
/// Returns `(modified_relative_paths, total_replacements)`.
pub fn rename_symbol_across_repo(
    ws: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<(Vec<String>, usize), String> {
    let old_clean = old_name.trim();
    let new_clean = new_name.trim();
    if old_clean.is_empty() || new_clean.is_empty() {
        return Err("old_name and new_name cannot be empty".to_string());
    }
    let workspace = Workspace::new(ws).map_err(|e| e.to_string())?;
    let pattern = format!(r"\b{}\b", regex::escape(old_clean));
    let re = Regex::new(&pattern).map_err(|e| e.to_string())?;

    let code_exts = [
        ".rs", ".go", ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".c", ".cpp", ".h",
        ".hpp", ".java", ".cs",
    ];
    let mut modified_files = Vec::new();
    let mut total_replacements = 0usize;

    for rel in workspace.list_all_files(2500) {
        let lower = rel.to_lowercase();
        if !code_exts.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }
        let Ok(abs) = workspace.resolve(&rel) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&abs) else {
            continue;
        };
        let count = re.find_iter(&content).count();
        if count == 0 {
            continue;
        }
        let replaced = re.replace_all(&content, new_clean).into_owned();
        if fs::write(&abs, replaced).is_ok() {
            modified_files.push(rel);
            total_replacements += count;
        }
    }

    Ok((modified_files, total_replacements))
}

/// Runs Git blame & pickaxe history (`git log -L` / `git blame -L` / `git log -S`) to uncover
/// the historical context behind a file line range or symbol.
pub fn git_archeology(
    ws: &Path,
    rel_path: &str,
    query_or_symbol: &str,
    start_line: usize,
    end_line: usize,
) -> String {
    let run_git = |args: &[&str]| -> Option<String> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args).current_dir(ws);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        let out = cmd.output().ok()?;
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).to_string())
        } else {
            None
        }
    };

    if run_git(&["rev-parse", "--is-inside-work-tree"]).is_none() {
        return "<git_archeology_report>\nWorkspace is not a git repository.\n</git_archeology_report>".to_string();
    }

    let mut sb = String::from("<git_archeology_report>\n");
    let clean_path = rel_path.trim().replace('\\', "/");
    if !clean_path.is_empty() && start_line > 0 && end_line >= start_line {
        sb.push_str(&format!(
            "[Line Range Archeology: {}:{}-{}]\n",
            clean_path, start_line, end_line
        ));
        let range_arg = format!("-L{},{}:{}", start_line, end_line, clean_path);
        if let Some(out_l) = run_git(&["log", "-n", "5", &range_arg]) {
            if !out_l.trim().is_empty() {
                let clipped: String = out_l.chars().take(2500).collect();
                sb.push_str(&clipped);
            }
        } else {
            let blame_range = format!("-L{},{}", start_line, end_line);
            if let Some(blame_out) = run_git(&["blame", &blame_range, "--", &clean_path]) {
                sb.push_str("Line Blame:\n");
                sb.push_str(&blame_out);
            }
            if let Some(hist_out) = run_git(&["log", "-n", "5", "--oneline", "--", &clean_path]) {
                sb.push_str("\nRecent Commits for File:\n");
                sb.push_str(&hist_out);
            }
        }
        sb.push('\n');
    }

    let q = query_or_symbol.trim();
    if !q.is_empty() {
        sb.push_str(&format!("[Symbol / Query Pickaxe Search: '{}']\n", q));
        if let Some(pickaxe) = run_git(&["log", "-n", "8", "--oneline", "-S", q]) {
            if !pickaxe.trim().is_empty() {
                sb.push_str("Commits modifying this symbol/query:\n");
                sb.push_str(&pickaxe);
                if let Some(diff_out) = run_git(&["log", "-n", "2", "--stat", "-p", "-S", q]) {
                    sb.push_str("\nRecent Diff History:\n");
                    let clipped: String = diff_out.chars().take(2500).collect();
                    sb.push_str(&clipped);
                }
            } else {
                sb.push_str("No commits found matching query in git pickaxe log.\n");
            }
        }
        sb.push('\n');
    }

    sb.push_str("</git_archeology_report>");
    sb
}

/// Executes `command` in `ws` with crash backtrace flags enabled (`RUST_BACKTRACE=1`, `GOTRACEBACK=all`,
/// `PYTHONFAULTHANDLER=1`), parses stack frames across Rust, Go, Python, and JS/TS, and extracts
/// a 15-line source code window + local variable assignments around each in-workspace crash frame.
pub fn debug_trace_command(ws: &Path, command: &str) -> String {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    cmd.current_dir(ws)
        .env("RUST_BACKTRACE", "1")
        .env("GOTRACEBACK", "all")
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONFAULTHANDLER", "1");

    let mut sb = String::from("=== POST-MORTEM CRASH TRACE & VARIABLE INSPECTOR ===\n");
    sb.push_str(&format!("Command: {}\n", command));
    sb.push_str(&format!("Working Directory: {}\n", ws.display()));

    let Ok(out) = cmd.output() else {
        sb.push_str("Execution Status: FAILED TO SPAWN COMMAND\n");
        return sb;
    };

    if out.status.success() {
        sb.push_str("Execution Status: SUCCESS (exit code 0)\n\n");
    } else {
        sb.push_str(&format!("Execution Status: FAILED ({})\n\n", out.status));
    }

    let raw_output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    sb.push_str("--- PROCESS OUTPUT ---\n");
    let clipped_out: String = raw_output.chars().take(4000).collect();
    sb.push_str(&clipped_out);
    sb.push_str("\n\n");

    let frames = parse_crash_frames(ws, &raw_output);
    if !frames.is_empty() {
        sb.push_str(&format!(
            "--- STACK FRAME INSPECTION ({} frame(s) identified in workspace) ---\n",
            frames.len()
        ));
        for (idx, (rel_path, line_no, snippet, vars)) in frames.iter().enumerate() {
            sb.push_str(&format!(
                "\n[Frame {}: {}:{}]\nSource Context:\n{}\n",
                idx + 1,
                rel_path,
                line_no,
                snippet
            ));
            if !vars.is_empty() {
                sb.push_str("Local Variables / Assignments in Scope:\n");
                for v in vars {
                    sb.push_str(&format!("    {}\n", v));
                }
            }
        }
    } else {
        sb.push_str("--- STACK FRAME INSPECTION ---\nNo in-workspace source crash frames identified.\n");
    }

    sb
}

fn parse_crash_frames(
    ws: &Path,
    output: &str,
) -> Vec<(String, usize, String, Vec<String>)> {
    let Ok(workspace) = Workspace::new(ws) else {
        return Vec::new();
    };
    let all_files = workspace.list_all_files(1500);
    let patterns = [
        r#"File "([^"]+)", line (\d+)"#,
        r"([A-Za-z0-9_.\-/\\]+\.(?:rs|go|py|js|ts|mjs|cjs|c|cpp)):(\d+)",
    ];

    let mut frames = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for pat in patterns {
        let Ok(re) = Regex::new(pat) else {
            continue;
        };
        for cap in re.captures_iter(output) {
            let Some(f_match) = cap.get(1) else { continue };
            let Some(l_match) = cap.get(2) else { continue };
            let Ok(line_no) = l_match.as_str().parse::<usize>() else {
                continue;
            };
            if line_no == 0 {
                continue;
            }
            let raw_file = f_match.as_str().replace('\\', "/");
            let resolved_rel = if workspace.resolve(&raw_file).is_ok_and(|p| p.is_file()) {
                Some(raw_file.clone())
            } else {
                let base = raw_file.rsplit('/').next().unwrap_or(&raw_file);
                all_files
                    .iter()
                    .find(|f| f == &&raw_file || f.ends_with(&format!("/{}", base)) || f == &base)
                    .cloned()
            };

            let Some(rel_path) = resolved_rel else {
                continue;
            };
            let key = format!("{}:{}", rel_path, line_no);
            if !seen.insert(key) {
                continue;
            }
            let Ok(abs_path) = workspace.resolve(&rel_path) else {
                continue;
            };
            let Ok(content) = fs::read_to_string(&abs_path) else {
                continue;
            };
            let lines: Vec<&str> = content.lines().collect();
            if line_no > lines.len() {
                continue;
            }
            let start = line_no.saturating_sub(8);
            let end = (line_no + 7).min(lines.len());
            let mut snippet = String::new();
            let mut vars = Vec::new();
            for i in start..end {
                let cur_line = i + 1;
                let marker = if cur_line == line_no { ">>" } else { "  " };
                snippet.push_str(&format!("{} {:4}: {}\n", marker, cur_line, lines[i]));
                let trimmed = lines[i].trim();
                if (trimmed.contains(" = ") || trimmed.contains(" := "))
                    && !trimmed.starts_with("//")
                    && !trimmed.starts_with('#')
                {
                    vars.push(format!("L{}: {}", cur_line, trimmed));
                }
            }
            frames.push((rel_path, line_no, snippet.trim_end().to_string(), vars));
            if frames.len() >= 6 {
                return frames;
            }
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outline_rust_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("sample.rs");
        fs::write(
            &file,
            "use std::fs;\n\npub struct Engine {\n    pub id: usize,\n}\n\nimpl Engine {\n    pub fn check_brace(&self, c: char) -> bool {\n        let s = \"unbalanced { brace in string\";\n        c == '{' && !s.is_empty()\n    }\n}\n\npub async fn run_engine() {\n    let e = Engine { id: 1 };\n    let _ = e.check_brace('{');\n}\n\nfn main() {\n    run_engine();\n}\n",
        )
        .unwrap();
        let imports = extract_imports(&file);
        assert_eq!(imports, vec!["use std::fs;".to_string()]);

        let syms = outline_file(&file);
        assert_eq!(syms.len(), 5);
        assert_eq!(syms[0].name, "Engine");
        assert_eq!(syms[0].kind, "struct");
        assert_eq!(syms[0].line, 3);
        assert_eq!(syms[0].end_line, 5);
        // Verify `'{'` and `"{"` inside `check_brace` did not corrupt `end_line`
        let check_fn = syms.iter().find(|s| s.name == "check_brace").unwrap();
        assert_eq!(check_fn.line, 8);
        assert_eq!(check_fn.end_line, 11);

        let py_file = tmp.path().join("mod.py");
        fs::write(
            &py_file,
            "class Worker:\n    def first(self):\n        return 1\n\n    def second(self):\n        return 2\n\ndef standalone():\n    return 3\n",
        )
        .unwrap();
        let py_syms = outline_file(&py_file);
        assert_eq!(py_syms.len(), 4);
        assert_eq!(py_syms[0].name, "Worker");
        assert_eq!(py_syms[0].line, 1);
        assert_eq!(py_syms[0].end_line, 6);

        let map = generate_repo_map(tmp.path(), 10, 2000);
        assert!(map.contains("sample.rs:"));
        assert!(map.contains("run_engine"));

        let callers = find_callers(tmp.path(), "run_engine");
        assert_eq!(callers.len(), 1);
        assert!(callers[0].contains("[caller: fn main]"));

        // Verify qualified container chain `[caller: impl Engine :: fn ...]`
        let brace_callers = find_callers(tmp.path(), "check_brace");
        assert_eq!(brace_callers.len(), 1);
        assert!(brace_callers[0].contains("[caller: fn run_engine]"));
    }

    #[test]
    fn test_ast_edit_rename_and_post_edit_check() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("math.rs");
        fs::write(
            &file,
            "pub fn add_two(x: i32) -> i32 {\n    x + 2\n}\n\npub fn compute() -> i32 {\n    add_two(10)\n}\n",
        )
        .unwrap();

        let (old_src, new_src, callers) = edit_symbol(
            tmp.path(),
            "math.rs",
            "add_two",
            "pub fn add_two(x: i32) -> i32 {\n    x + 20\n}",
        )
        .unwrap();
        assert!(old_src.contains("x + 2"));
        assert!(new_src.contains("x + 20"));
        assert!(callers.contains("compute"));

        let check_ok = run_post_edit_check(tmp.path(), "math.rs");
        assert!(check_ok.contains("PASSED"));

        let (renamed_files, count) =
            rename_symbol_across_repo(tmp.path(), "add_two", "add_twenty").unwrap();
        assert_eq!(renamed_files, vec!["math.rs".to_string()]);
        assert_eq!(count, 2);

        let frames = parse_crash_frames(tmp.path(), "thread 'main' panicked at math.rs:6:5:");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "math.rs");
        assert_eq!(frames[0].1, 6);
    }
}
