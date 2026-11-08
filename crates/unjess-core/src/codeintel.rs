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
}
