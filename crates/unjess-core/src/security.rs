//! Autonomous Red Team vs. White Team Security Duel Scanner (`security.rs`).
//!
//! Performs static taint & vulnerability auditing across the active workspace
//! and synthesizes actionable White Team remediation recommendations.

use crate::workspace::Workspace;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

static COMPILED_SEC_RULES: OnceLock<Vec<SecRule>> = OnceLock::new();

/// A single vulnerability finding discovered during Red Team scanning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityFinding {
    pub severity: String, // "CRITICAL" | "HIGH" | "MEDIUM" | "LOW"
    pub title: String,
    pub category: String,
    pub file: String,
    pub line: usize,
    pub snippet: String,
    pub remediation: String,
}

/// Complete Red Team / White Team Security Duel report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityDuelReport {
    pub red_team_findings: Vec<SecurityFinding>,
    pub white_team_patches: Vec<String>,
    pub score: u8,
    pub summary: String,
}

struct SecRule {
    severity: &'static str,
    category: &'static str,
    re: Regex,
    remediation: &'static str,
}

fn severity_rank(sev: &str) -> u8 {
    match sev {
        "CRITICAL" => 0,
        "HIGH" => 1,
        "MEDIUM" => 2,
        _ => 3,
    }
}

/// Scans all source files in `ws_path` for common security vulnerabilities and secret leaks.
pub fn scan_workspace(ws_path: &Path) -> SecurityDuelReport {
    let Ok(workspace) = Workspace::new(ws_path) else {
        return SecurityDuelReport {
            red_team_findings: Vec::new(),
            white_team_patches: Vec::new(),
            score: 100,
            summary: "Workspace could not be opened for scanning.".to_string(),
        };
    };

    let rules = COMPILED_SEC_RULES.get_or_init(build_rules);
    let mut findings = Vec::new();
    let skip_exts = [
        ".lock", ".sum", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".exe", ".dll",
        ".pdb", ".so", ".dylib", ".woff", ".woff2", ".ttf", ".pdf", ".zip", ".min.js",
    ];

    for rel in workspace.list_all_files(1500) {
        if findings.len() >= 100 {
            break;
        }
        let lower_rel = rel.to_lowercase();
        if skip_exts.iter().any(|ext| lower_rel.ends_with(ext))
            || lower_rel.ends_with("security.rs")
        {
            continue;
        }
        let Ok(abs) = workspace.resolve(&rel) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&abs) else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            for rule in rules {
                if rule.re.is_match(trimmed) {
                    // Skip unsafe Rust block if documented with `// SAFETY:` on same or previous line
                    if rule.category == "Unsafe Memory Block" {
                        let prev_line = if idx > 0 { lines[idx - 1].trim() } else { "" };
                        if trimmed.contains("SAFETY:") || prev_line.contains("SAFETY:") {
                            continue;
                        }
                    }

                    let snippet: String = trimmed.chars().take(120).collect();
                    findings.push(SecurityFinding {
                        severity: rule.severity.to_string(),
                        title: rule.category.to_string(),
                        category: rule.category.to_string(),
                        file: rel.clone(),
                        line: idx + 1,
                        snippet,
                        remediation: rule.remediation.to_string(),
                    });
                }
            }
        }
    }

    // Sort findings by severity so CRITICAL and HIGH vulnerabilities always appear first
    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });

    let mut penalty: usize = 0;
    for f in &findings {
        penalty += match f.severity.as_str() {
            "CRITICAL" => 20,
            "HIGH" => 10,
            "MEDIUM" => 4,
            _ => 2,
        };
    }
    let score = 100u8.saturating_sub(penalty.min(95) as u8);

    let mut white_team_patches: Vec<String> = Vec::new();
    for f in &findings {
        if white_team_patches.len() >= 25 {
            break;
        }
        let patch = format!(
            "[WHITE TEAM PATCH] [{}] {}:{} ({}) `{}` -> {}",
            f.severity, f.file, f.line, f.category, f.snippet, f.remediation
        );
        if !white_team_patches.contains(&patch) {
            white_team_patches.push(patch);
        }
    }

    let summary = if findings.is_empty() {
        "Zero vulnerabilities detected. Workspace passed Red Team / White Team Security Duel with 100/100 score.".to_string()
    } else {
        format!(
            "Red Team discovered {} potential issue(s); White Team synthesized {} defensive remediation patch(es). Security Score: {}/100.",
            findings.len(),
            white_team_patches.len(),
            score
        )
    };

    SecurityDuelReport {
        red_team_findings: findings,
        white_team_patches,
        score,
        summary,
    }
}

fn build_rules() -> Vec<SecRule> {
    let raw = [
        (
            "CRITICAL",
            "Hardcoded API Key / Secret",
            r#"(?i)(?:api_key|secret_key|private_key|access_token|auth_token|client_secret)\s*[:=]\s*["'](?:sk-[A-Za-z0-9_-]{12,}|AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|[A-Za-z0-9_\-]{20,})["']"#,
            "Load secrets from environment variables or `~/.unjess-rust/keys.yaml` instead of hardcoding literals.",
        ),
        (
            "CRITICAL",
            "Hardcoded Private Key / JWT",
            r#"-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----|\beyJ[A-Za-z0-9_-]{12,}\.eyJ[A-Za-z0-9_-]{12,}\.[A-Za-z0-9_-]{12,}"#,
            "Never commit private PEM keys or live JWT tokens in source files; load from secure vault/env at runtime.",
        ),
        (
            "CRITICAL",
            "SSRF / Cloud Metadata Endpoint",
            r#"(?:169\.254\.169\.254|metadata\.google\.internal|100\.100\.100\.200|\[fd00:ec2::254\])"#,
            "Block link-local/cloud metadata endpoints (`169.254.169.254`) and validate resolved IP addresses against SSRF.",
        ),
        (
            "HIGH",
            "Command Injection / Shell Eval",
            r#"(?:\beval\s*\(|os\.system\s*\(|subprocess\.(?:Popen|call|run)\([^)]*shell\s*=\s*True|child_process\.exec\s*\(|exec\.Command\(\s*["'](?:sh|bash|cmd|powershell)["'])"#,
            "Avoid `eval` or `shell=True`; pass arguments as an explicit array to `Command::new`.",
        ),
        (
            "HIGH",
            "Formatted Shell Command Execution",
            r#"Command::new\(\s*["'](?:sh|bash|cmd|powershell)["']\s*\).*format!\("#,
            "Avoid passing interpolated `format!(...)` strings to a shell interpreter; pass untrusted values as discrete arguments.",
        ),
        (
            "HIGH",
            "Insecure Deserialization",
            r#"(?:pickle\.loads\s*\(|yaml\.load\s*\([^)]*\)|marshal\.loads\s*\()"#,
            "Use `yaml.safe_load` or typed JSON/schema deserialization instead of arbitrary object deserializers.",
        ),
        (
            "HIGH",
            "SQL Injection Risk",
            r#"(?i)(?:SELECT\s+.+\s+FROM|INSERT\s+INTO|UPDATE\s+\w+\s+SET|DELETE\s+FROM)\s+.+(?:\+\s*[a-zA-Z_]|format!\(|\$\{|f["'])"#,
            "Use parameterized queries / prepared statements (`$1`, `?`) instead of string interpolation.",
        ),
        (
            "HIGH",
            "Path Traversal Risk",
            r#"(?:\.\./|\.\.\\){2,}|(?:send_file|read_to_string|readFileSync)\s*\(\s*(?:req\.|params\.|query\.|user_)"#,
            "Canonicalize paths and enforce workspace containment (`Workspace::resolve`) before reading or serving files.",
        ),
        (
            "HIGH",
            "Cross-Site Scripting (XSS) / DOM Sink",
            r#"(?:\.innerHTML\s*=|\.outerHTML\s*=|dangerouslySetInnerHTML|document\.write\s*\(|v-html\s*=)"#,
            "Sanitize untrusted HTML or use `.textContent` to prevent DOM-based XSS.",
        ),
        (
            "MEDIUM",
            "Unsafe Memory Block",
            r#"\bunsafe\s*\{"#,
            "Document invariants with `// SAFETY:` or refactor to safe Rust abstractions.",
        ),
        (
            "MEDIUM",
            "TLS Verification Disabled",
            r#"(?:danger_accept_invalid_certs\s*\(\s*true\s*\)|InsecureSkipVerify\s*:\s*true|verify\s*=\s*False|NODE_TLS_REJECT_UNAUTHORIZED\s*=\s*['"]?0)"#,
            "Enable strict TLS certificate verification in production HTTP clients.",
        ),
        (
            "MEDIUM",
            "Weak Cryptographic Primitive",
            r#"(?i)(?:\b(?:md5|sha1)::|hashlib\.(?:md5|sha1)\s*\(|crypto\.createHash\(\s*['"](?:md5|sha1)['"]\s*\))"#,
            "Upgrade legacy MD5/SHA-1 hashes to SHA-256 (`sha2::Sha256`) or BLAKE3.",
        ),
        (
            "MEDIUM",
            "Prototype Pollution / DOM Clobbering",
            r#"(?:\.__proto__\b|\[\s*['"]__proto__['"]\s*\]|constructor\.prototype\b)"#,
            "Use `Object.create(null)` or validate object keys against `__proto__` and `constructor` before merging.",
        ),
    ];

    raw.into_iter()
        .filter_map(|(severity, category, pat, remediation)| {
            Regex::new(pat).ok().map(|re| SecRule {
                severity,
                category,
                re,
                remediation,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_duel_scanner() {
        let tmp = tempfile::tempdir().unwrap();
        let vuln_file = tmp.path().join("vuln.py");
        fs::write(
            &vuln_file,
            "os.system(user_cmd)\napi_key = \"sk-1234567890abcdefghijklmnop\"\n",
        )
        .unwrap();

        let report = scan_workspace(tmp.path());
        assert!(report.red_team_findings.len() >= 2);
        assert_eq!(report.red_team_findings[0].severity, "CRITICAL");
        assert!(report.score < 100);
        assert!(!report.white_team_patches.is_empty());

        // Verify self-scan never flags `security.rs`
        let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let self_report = scan_workspace(&crate_src);
        assert!(!self_report
            .red_team_findings
            .iter()
            .any(|f| f.file.ends_with("security.rs")));
    }
}
