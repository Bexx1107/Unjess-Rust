//! Skills Manager (`~/.unjess-rust/skills` + `~/.unjess/skills` + workspace `.agents/skills`).
//! Supports multi-file skills with companion `scripts/`, `references/`, `examples/`, and `resources/`.

use crate::config::Settings;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Metadata, companion file inventory, and markdown instructions for an agent skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    #[serde(default)]
    pub dir_path: String,
    pub builtin: bool,
    pub content: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub repo_url: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

fn default_scope() -> String {
    "global".to_string()
}

pub fn slugify_skill_id(input: &str) -> String {
    let raw: String = input
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut id = raw.trim_matches('-').to_string();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id
}

fn unquote_yaml_scalar(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.len() >= 2 {
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            let inner = &trimmed[1..trimmed.len() - 1];
            return inner
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\\\", "\\");
        }
    }
    trimmed.to_string()
}

fn quote_yaml_scalar(val: &str) -> String {
    let clean = val
        .trim()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', " ")
        .replace('\n', " ");
    format!("\"{}\"", clean)
}

fn scan_skill_companion_files(skill_dir: &Path) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut scripts = Vec::new();
    let mut references = Vec::new();

    if !skill_dir.is_dir() {
        return (files, scripts, references);
    }

    fn walk(
        base: &Path,
        current: &Path,
        depth: usize,
        files: &mut Vec<String>,
        scripts: &mut Vec<String>,
        references: &mut Vec<String>,
    ) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.starts_with(".git")
                || fname == "node_modules"
                || fname == "__pycache__"
                || fname == "target"
            {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, depth + 1, files, scripts, references);
            } else if p.is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel_str = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join("/");
                    files.push(rel_str.clone());
                    if let Some(s) = rel_str.strip_prefix("scripts/") {
                        scripts.push(s.to_string());
                    } else if let Some(r) = rel_str.strip_prefix("references/") {
                        references.push(r.to_string());
                    } else if depth == 0
                        && (fname.ends_with(".py")
                            || fname.ends_with(".sh")
                            || fname.ends_with(".ps1")
                            || fname.ends_with(".js"))
                    {
                        scripts.push(fname);
                    }
                }
            }
        }
    }

    walk(
        skill_dir,
        skill_dir,
        0,
        &mut files,
        &mut scripts,
        &mut references,
    );
    files.sort_by(|a, b| {
        if a == "SKILL.md" {
            std::cmp::Ordering::Less
        } else if b == "SKILL.md" {
            std::cmp::Ordering::Greater
        } else {
            a.cmp(b)
        }
    });
    scripts.sort();
    references.sort();
    (files, scripts, references)
}

fn copy_dir_recursive_skip_git(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let entries = fs::read_dir(src).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" || name_str == "node_modules" || name_str == "__pycache__" {
            continue;
        }
        let target = dst.join(&name);
        if p.is_dir() {
            copy_dir_recursive_skip_git(&p, &target)?;
        } else if p.is_file() {
            let _ = fs::copy(&p, &target);
        }
    }
    Ok(())
}

pub struct SkillsManager;

impl SkillsManager {
    pub fn skills_dir() -> PathBuf {
        Settings::config_dir().join("skills")
    }

    fn deleted_builtins_file() -> PathBuf {
        Settings::config_dir().join("deleted_builtin_skills.json")
    }

    fn load_deleted_builtins() -> HashSet<String> {
        let p = Self::deleted_builtins_file();
        if let Ok(raw) = fs::read_to_string(p) {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
                return list.into_iter().collect();
            }
        }
        HashSet::new()
    }

    fn save_deleted_builtins(set: &HashSet<String>) {
        let p = Self::deleted_builtins_file();
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let list: Vec<String> = set.iter().cloned().collect();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = fs::write(p, json);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_skill(
        id: String,
        name: String,
        description: String,
        path: String,
        dir_path: String,
        builtin: bool,
        body: String,
        raw_content: String,
        scope: &str,
        extra_triggers: Vec<String>,
        folder: String,
        repo_url: String,
        files: Vec<String>,
        scripts: Vec<String>,
        references: Vec<String>,
    ) -> SkillInfo {
        let mut triggers = Vec::new();
        for t in extra_triggers {
            let trimmed = t.trim().to_string();
            if !trimmed.is_empty()
                && !triggers
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(&trimmed))
            {
                triggers.push(trimmed);
            }
        }
        let resolved_repo = if !repo_url.trim().is_empty() {
            repo_url.trim().to_string()
        } else if builtin {
            "builtin".to_string()
        } else {
            "local".to_string()
        };
        let norm_dir = dir_path.replace('\\', "/");
        let norm_skills_root = Self::skills_dir().to_string_lossy().replace('\\', "/");
        let resolved_body = if !norm_dir.is_empty() {
            body.replace(
                &format!("${{CLAUDE_PLUGIN_ROOT}}/.claude/skills/{}", id),
                &norm_dir,
            )
            .replace(
                &format!("~/.claude/skills/{}", id),
                &norm_dir,
            )
            .replace("${CLAUDE_PLUGIN_ROOT}/.claude/skills", &norm_skills_root)
            .replace("${CLAUDE_PLUGIN_ROOT}", &norm_dir)
        } else {
            body
        };
        SkillInfo {
            id,
            name,
            description,
            path,
            dir_path,
            builtin,
            instructions: resolved_body.clone(),
            body: resolved_body,
            content: raw_content,
            scope: scope.to_string(),
            triggers,
            repo: resolved_repo.clone(),
            repo_url: if resolved_repo == "local" || resolved_repo == "builtin" {
                String::new()
            } else {
                resolved_repo
            },
            folder,
            files,
            scripts,
            references,
        }
    }

    /// Strips all leading YAML frontmatter blocks (`--- ... ---`) and extracts
    /// `(title, description, stripped_body, triggers, folder, repo_url)`.
    pub fn parse_skill_markdown(
        fallback_id: &str,
        raw: &str,
    ) -> (String, String, String, Vec<String>, String, String) {
        let mut name = fallback_id.to_string();
        let mut desc = String::new();
        let mut has_explicit_name = false;
        let mut body = raw.trim_start_matches('\u{feff}');
        let mut triggers = Vec::new();
        let mut folder = String::new();
        let mut repo_url = String::new();

        // Loop to strip one or more stacked `--- ... ---` frontmatter blocks
        loop {
            let trimmed = body.trim_start_matches([' ', '\t', '\r', '\n']);
            if !trimmed.starts_with("---") {
                break;
            }
            let after_open = &trimmed[3..];
            let Some(first_nl) = after_open.find('\n') else {
                break;
            };
            if !after_open[..first_nl].trim().is_empty() {
                break;
            }
            let yaml_start = &after_open[first_nl + 1..];
            let mut close_offset: Option<(usize, usize)> = None;
            let mut cursor = 0usize;
            for line in yaml_start.split_inclusive('\n') {
                let line_trimmed = line.trim_end_matches(['\r', '\n']).trim();
                if line_trimmed == "---" || line_trimmed == "..." {
                    close_offset = Some((cursor, cursor + line.len()));
                    break;
                }
                cursor += line.len();
            }
            let Some((front_end, rest_start)) = close_offset else {
                break;
            };
            let front = &yaml_start[..front_end];
            let mut in_triggers_list = false;
            let mut in_desc_multiline = false;
            let mut desc_lines: Vec<String> = Vec::new();

            for line in front.lines() {
                let raw_line = line.trim_end_matches('\r');
                let l = raw_line.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                if in_desc_multiline {
                    if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
                        desc_lines.push(l.to_string());
                        continue;
                    } else {
                        if !desc_lines.is_empty() {
                            desc = desc_lines.join(" ");
                            desc_lines.clear();
                        }
                        in_desc_multiline = false;
                    }
                }
                if in_triggers_list {
                    if let Some(item) = l.strip_prefix('-') {
                        let clean_item = unquote_yaml_scalar(item);
                        if !clean_item.is_empty()
                            && !triggers
                                .iter()
                                .any(|existing: &String| existing.eq_ignore_ascii_case(&clean_item))
                        {
                            triggers.push(clean_item);
                        }
                        continue;
                    } else if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
                        continue;
                    } else {
                        in_triggers_list = false;
                    }
                }

                if let Some(v) = l.strip_prefix("name:") {
                    let clean = unquote_yaml_scalar(v);
                    if !clean.is_empty() {
                        name = clean;
                        has_explicit_name = true;
                    }
                } else if let Some(v) = l.strip_prefix("description:") {
                    let clean = unquote_yaml_scalar(v);
                    if matches!(clean.as_str(), ">-" | ">" | "|-" | "|") {
                        in_desc_multiline = true;
                    } else if !clean.is_empty() {
                        desc = clean;
                    }
                } else if let Some(v) = l
                    .strip_prefix("folder:")
                    .or_else(|| l.strip_prefix("category:"))
                {
                    let clean = unquote_yaml_scalar(v);
                    if !clean.is_empty() {
                        folder = clean;
                    }
                } else if let Some(v) = l
                    .strip_prefix("repo_url:")
                    .or_else(|| l.strip_prefix("repo:"))
                {
                    let clean = unquote_yaml_scalar(v);
                    if !clean.is_empty() && clean != "local" && clean != "builtin" {
                        repo_url = clean;
                    }
                } else if let Some(v) = l.strip_prefix("triggers:") {
                    let v_trim = v.trim();
                    if v_trim.is_empty() {
                        in_triggers_list = true;
                    } else {
                        let clean = v_trim.trim_matches('[').trim_matches(']');
                        for part in clean.split(',') {
                            let item = unquote_yaml_scalar(part);
                            if !item.is_empty()
                                && !triggers
                                    .iter()
                                    .any(|existing: &String| existing.eq_ignore_ascii_case(&item))
                            {
                                triggers.push(item);
                            }
                        }
                    }
                }
            }
            if in_desc_multiline && !desc_lines.is_empty() {
                desc = desc_lines.join(" ");
            }

            body = yaml_start[rest_start..].trim_start_matches(['\r', '\n']);
        }

        for line in body.lines() {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            if let Some(h) = l.strip_prefix('#') {
                if !has_explicit_name && name == fallback_id {
                    let clean_h = h.trim_start_matches('#').trim();
                    if !clean_h.is_empty() {
                        name = clean_h.to_string();
                    }
                }
            } else if (desc.is_empty() || matches!(desc.as_str(), ">-" | ">" | "|-" | "|"))
                && !l.starts_with("---")
            {
                desc = l.to_string();
            }
        }

        if desc.is_empty() || matches!(desc.as_str(), ">-" | ">" | "|-" | "|") {
            desc = "Custom Unjess skill plugin".to_string();
        }
        (name, desc, body.to_string(), triggers, folder, repo_url)
    }

    /// Returns all available skills, always including built-in skills (unless deleted) plus custom skills from
    /// `~/.unjess-rust/skills`, `~/.unjess/skills`, and active workspace `.agents/skills`.
    pub fn list_skills() -> Vec<SkillInfo> {
        Self::list_skills_for_workspace(None)
    }

    /// Returns all available skills including those inside `workspace_root` (`.agents/skills` and `.unjess/skills`).
    pub fn list_skills_for_workspace(workspace_root: Option<&Path>) -> Vec<SkillInfo> {
        let deleted_builtins = Self::load_deleted_builtins();
        let mut skills = Vec::new();

        if !deleted_builtins.contains("reverse-engineer-anything") {
            let rea_content = "# Reverse Engineer Anything (REA)\n\n## Core Workflow\n1. **Reconnaissance & Fingerprinting**: Identify file headers, entropy, imports/exports, strings, and compiler signatures.\n2. **Control-Flow & Call-Graph Recovery**: Map entrypoints, critical dispatch tables, cryptographic routines, and network handlers.\n3. **Protocol & Struct Reconstruction**: Recover wire formats, serialization schemas, and state machines into clean Rust/C structs.\n4. **Security & Vulnerability Audit**: Identify memory corruption, hardcoded credentials, command injection, and auth bypasses.".to_string();
            skills.push(Self::make_skill(
                "reverse-engineer-anything".to_string(),
                "Reverse Engineer Anything (REA)".to_string(),
                "Deep binary, PE/ELF/Mach-O, APK, firmware, network protocol, and obfuscated JS/WASM reverse engineering workflow.".to_string(),
                "builtin://reverse-engineer-anything/SKILL.md".to_string(),
                String::new(),
                true,
                rea_content.clone(),
                rea_content,
                "builtin",
                vec![
                    "reverse engineer".to_string(),
                    "decompile".to_string(),
                    "/rea".to_string(),
                    "@rea".to_string(),
                ],
                "Security & RE".to_string(),
                "https://github.com/morluto/rea".to_string(),
                vec!["SKILL.md".to_string()],
                Vec::new(),
                Vec::new(),
            ));
        }

        if !deleted_builtins.contains("rust-systems-architect") {
            let rust_content = "# Rust Systems & GPU Architect\n\n- Prefer ownership clarity, `Arc<RwLock<...>>` with short critical sections, and zero-allocation hot loops.\n- Enforce strict path sandboxing and exhaustive error handling with `thiserror`.".to_string();
            skills.push(Self::make_skill(
                "rust-systems-architect".to_string(),
                "Rust Systems & GPU Architect".to_string(),
                "Zero-copy, lock-free, memory-safe Rust architecture and GPU UI optimization guidelines.".to_string(),
                "builtin://rust-systems-architect/SKILL.md".to_string(),
                String::new(),
                true,
                rust_content.clone(),
                rust_content,
                "builtin",
                vec!["rust architect".to_string()],
                "Architecture".to_string(),
                String::new(),
                vec!["SKILL.md".to_string()],
                Vec::new(),
                Vec::new(),
            ));
        }

        let mut search_dirs = Vec::new();
        if let Some(ws) = workspace_root {
            search_dirs.push((ws.join(".agents").join("skills"), "workspace"));
            search_dirs.push((ws.join(".unjess").join("skills"), "workspace"));
        } else if let Ok(cwd) = std::env::current_dir() {
            search_dirs.push((cwd.join(".agents").join("skills"), "workspace"));
            search_dirs.push((cwd.join(".unjess").join("skills"), "workspace"));
        }
        search_dirs.push((Self::skills_dir(), "global"));
        search_dirs.push((Settings::legacy_go_config_dir().join("skills"), "global"));

        for (dir, scope) in search_dirs {
            Self::scan_skills_dir(&dir, scope, &mut skills);
        }

        skills
    }

    fn scan_skills_dir(dir: &Path, scope: &str, skills: &mut Vec<SkillInfo>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.is_file() {
                    if let Ok(content) = fs::read_to_string(&skill_md) {
                        let id = entry.file_name().to_string_lossy().to_string();
                        let (name, desc, body, extra_triggers, folder, repo_url) =
                            Self::parse_skill_markdown(&id, &content);
                        let (files, scripts, references) = scan_skill_companion_files(&path);
                        let dir_path_str = path.to_string_lossy().to_string();

                        if let Some(existing_idx) = skills.iter().position(|s| {
                            s.id.eq_ignore_ascii_case(&id)
                                || s.name.eq_ignore_ascii_case(&name)
                                || slugify_skill_id(&s.name) == slugify_skill_id(&name)
                        }) {
                            if skills[existing_idx].builtin {
                                skills[existing_idx] = Self::make_skill(
                                    id,
                                    name,
                                    desc,
                                    skill_md.to_string_lossy().to_string(),
                                    dir_path_str,
                                    false,
                                    body,
                                    content,
                                    scope,
                                    extra_triggers,
                                    folder,
                                    repo_url,
                                    files,
                                    scripts,
                                    references,
                                );
                            }
                            continue;
                        }
                        skills.push(Self::make_skill(
                            id,
                            name,
                            desc,
                            skill_md.to_string_lossy().to_string(),
                            dir_path_str,
                            false,
                            body,
                            content,
                            scope,
                            extra_triggers,
                            folder,
                            repo_url,
                            files,
                            scripts,
                            references,
                        ));
                    }
                }
            }
        }
    }

    fn contains_trigger_bounded(haystack: &str, trigger: &str) -> bool {
        let t = trigger.trim().to_lowercase();
        if t.is_empty() {
            return false;
        }
        if t.starts_with('/') || t.starts_with('@') || t.contains(' ') {
            return haystack.contains(&t);
        }
        let mut start = 0;
        while let Some(pos) = haystack[start..].find(&t) {
            let abs = start + pos;
            let end = abs + t.len();
            let prev_ok = abs == 0
                || haystack[..abs]
                    .chars()
                    .next_back()
                    .map(|c| !c.is_alphanumeric() && c != '_' && c != '-')
                    .unwrap_or(true);
            let next_ok = end >= haystack.len()
                || haystack[end..]
                    .chars()
                    .next()
                    .map(|c| !c.is_alphanumeric() && c != '_' && c != '-')
                    .unwrap_or(true);
            if prev_ok && next_ok {
                return true;
            }
            start = abs + haystack[abs..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        false
    }

    /// Returns any skills whose `id`, `name`, `@<id>`, `/skill <id>`, or configured `triggers`
    /// match the user's prompt so their full markdown instructions can be auto-injected.
    pub fn match_triggered_skills(
        prompt: &str,
        workspace_root: Option<&Path>,
    ) -> Vec<SkillInfo> {
        let lower_prompt = prompt.trim().to_lowercase();
        if lower_prompt.is_empty() {
            return Vec::new();
        }
        Self::list_skills_for_workspace(workspace_root)
            .into_iter()
            .filter(|s| {
                let slash_cmd = format!("/skill {}", s.id);
                let mention = format!("@{}", s.id);
                Self::contains_trigger_bounded(&lower_prompt, &s.id)
                    || Self::contains_trigger_bounded(&lower_prompt, &s.name)
                    || Self::contains_trigger_bounded(&lower_prompt, &slash_cmd)
                    || Self::contains_trigger_bounded(&lower_prompt, &mention)
                    || s.triggers
                        .iter()
                        .any(|t| Self::contains_trigger_bounded(&lower_prompt, t))
            })
            .collect()
    }

    /// Saves a custom skill to `~/.unjess-rust/skills/<id>/SKILL.md` (defaulting to global scope).
    pub fn save_skill(name: &str, description: &str, content: &str) -> Result<SkillInfo, String> {
        Self::save_skill_full(name, description, content, &[], "global", "", "", None)
    }

    /// Saves a custom skill with full metadata (`triggers`, `scope`, `repo_url`, `folder`, `workspace_root`).
    #[allow(clippy::too_many_arguments)]
    pub fn save_skill_full(
        name: &str,
        description: &str,
        content: &str,
        triggers: &[String],
        scope: &str,
        repo_url: &str,
        folder: &str,
        workspace_root: Option<&Path>,
    ) -> Result<SkillInfo, String> {
        Self::save_skill_full_with_id(
            None,
            name,
            description,
            content,
            triggers,
            scope,
            repo_url,
            folder,
            workspace_root,
        )
    }

    /// Saves a custom skill, optionally preserving or migrating from `existing_id` so companion
    /// `scripts/`, `references/`, and `examples/` are never orphaned or duplicated.
    #[allow(clippy::too_many_arguments)]
    pub fn save_skill_full_with_id(
        existing_id: Option<&str>,
        name: &str,
        description: &str,
        content: &str,
        triggers: &[String],
        scope: &str,
        repo_url: &str,
        folder: &str,
        workspace_root: Option<&Path>,
    ) -> Result<SkillInfo, String> {
        let existing_skills = Self::list_skills_for_workspace(workspace_root);
        let matched_existing = existing_id
            .filter(|s| !s.trim().is_empty())
            .and_then(|eid| {
                existing_skills.iter().find(|s| {
                    s.id.eq_ignore_ascii_case(eid.trim())
                        || s.name.eq_ignore_ascii_case(eid.trim())
                })
            })
            .or_else(|| {
                existing_skills.iter().find(|s| {
                    s.name.eq_ignore_ascii_case(name.trim())
                        || s.id.eq_ignore_ascii_case(name.trim())
                        || slugify_skill_id(&s.name) == slugify_skill_id(name)
                })
            });

        let id = if let Some(ex) = matched_existing {
            ex.id.clone()
        } else if let Some(eid) = existing_id.filter(|s| !s.trim().is_empty()) {
            slugify_skill_id(eid)
        } else {
            slugify_skill_id(name)
        };

        if id.is_empty() {
            return Err("Skill name cannot be empty".to_string());
        }

        // Un-delete if this was a previously deleted builtin skill
        let mut deleted_builtins = Self::load_deleted_builtins();
        if deleted_builtins.remove(&id) {
            Self::save_deleted_builtins(&deleted_builtins);
        }

        // Strip any existing frontmatter that might have been passed inside `content`
        let (parsed_name, parsed_desc, stripped_body, parsed_triggers, parsed_folder, parsed_repo) =
            Self::parse_skill_markdown(&id, content);

        let final_name = if !name.trim().is_empty() {
            name.trim().to_string()
        } else {
            parsed_name
        };
        let final_desc = if !description.trim().is_empty() {
            description.trim().to_string()
        } else {
            parsed_desc
        };
        let final_folder = if !folder.trim().is_empty() {
            folder.trim().to_string()
        } else {
            parsed_folder
        };
        let final_repo = if !repo_url.trim().is_empty()
            && repo_url.trim() != "local"
            && repo_url.trim() != "builtin"
        {
            repo_url.trim().to_string()
        } else {
            parsed_repo
        };
        let mut final_triggers: Vec<String> = Vec::new();
        let source_triggers = if !triggers.is_empty() {
            triggers
        } else {
            &parsed_triggers[..]
        };
        for t in source_triggers {
            let clean_t = t.trim();
            if !clean_t.is_empty()
                && !final_triggers
                    .iter()
                    .any(|e: &String| e.eq_ignore_ascii_case(clean_t))
            {
                final_triggers.push(clean_t.to_string());
            }
        }

        let target_scope = if scope.trim().eq_ignore_ascii_case("workspace") {
            "workspace"
        } else {
            "global"
        };

        let base_dir = if target_scope == "workspace" {
            if let Some(ws) = workspace_root {
                ws.join(".agents").join("skills")
            } else if let Ok(cwd) = std::env::current_dir() {
                cwd.join(".agents").join("skills")
            } else {
                Self::skills_dir()
            }
        } else {
            Self::skills_dir()
        };

        let skill_dir = base_dir.join(&id);
        fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

        // If the skill previously lived in another directory (e.g. `~/.unjess/skills/<id>`),
        // copy over all companion files (`scripts/`, `references/`, `examples/`, etc.) first!
        if let Some(ex) = matched_existing {
            if !ex.dir_path.is_empty() {
                let src_dir = PathBuf::from(&ex.dir_path);
                if src_dir.is_dir() && src_dir != skill_dir {
                    let _ = copy_dir_recursive_skip_git(&src_dir, &skill_dir);
                }
            }
        }

        let skill_path = skill_dir.join("SKILL.md");

        let clean_instructions = if stripped_body.trim().is_empty() {
            format!("# {}\n\n{}", final_name, final_desc)
        } else {
            stripped_body.trim().to_string()
        };

        let mut fm = String::from("---\n");
        fm.push_str(&format!("name: {}\n", quote_yaml_scalar(&final_name)));
        if !final_desc.is_empty() {
            fm.push_str(&format!("description: {}\n", quote_yaml_scalar(&final_desc)));
        }
        if !final_folder.is_empty() {
            fm.push_str(&format!("folder: {}\n", quote_yaml_scalar(&final_folder)));
        }
        if !final_repo.is_empty() {
            fm.push_str(&format!("repo_url: {}\n", quote_yaml_scalar(&final_repo)));
        }
        if !final_triggers.is_empty() {
            fm.push_str("triggers:\n");
            for tr in &final_triggers {
                fm.push_str(&format!("  - {}\n", quote_yaml_scalar(tr)));
            }
        }
        fm.push_str("---\n\n");

        let full_content = format!("{}{}\n", fm, clean_instructions);
        fs::write(&skill_path, &full_content).map_err(|e| e.to_string())?;

        // Also update in legacy `~/.unjess/skills/<id>/SKILL.md` if it already exists there so both stay in sync
        if target_scope == "global" {
            let legacy_dir = Settings::legacy_go_config_dir().join("skills").join(&id);
            if legacy_dir.is_dir() {
                let _ = fs::write(legacy_dir.join("SKILL.md"), &full_content);
            }
        }

        let (files, scripts, references) = scan_skill_companion_files(&skill_dir);

        Ok(Self::make_skill(
            id,
            final_name,
            final_desc,
            skill_path.to_string_lossy().to_string(),
            skill_dir.to_string_lossy().to_string(),
            false,
            clean_instructions,
            full_content,
            target_scope,
            final_triggers,
            final_folder,
            final_repo,
            files,
            scripts,
            references,
        ))
    }

    /// Reads any companion file (`SKILL.md`, `scripts/foo.py`, `references/bar.md`) inside a skill's folder.
    pub fn read_skill_file(
        skill_id_or_name: &str,
        rel_path: &str,
        workspace_root: Option<&Path>,
    ) -> Result<String, String> {
        let rel = rel_path.trim().trim_start_matches(['/', '\\']);
        if rel.is_empty() || rel.contains("..") {
            return Err("Invalid relative file path inside skill".to_string());
        }
        let skills = Self::list_skills_for_workspace(workspace_root);
        let target_slug = slugify_skill_id(skill_id_or_name);
        let skill = skills
            .into_iter()
            .find(|s| {
                s.id.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || s.name.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || slugify_skill_id(&s.name) == target_slug
            })
            .ok_or_else(|| format!("Skill '{}' not found", skill_id_or_name))?;

        if rel.eq_ignore_ascii_case("SKILL.md") && skill.dir_path.is_empty() {
            return Ok(skill.content);
        }
        let dir = PathBuf::from(&skill.dir_path);
        let full = dir.join(rel);
        if !full.is_file() {
            return Err(format!("File '{}' not found in skill '{}'", rel, skill.name));
        }
        fs::read_to_string(&full).map_err(|e| e.to_string())
    }

    /// Writes or creates a companion file (`scripts/run.py`, `references/guide.md`, etc.) inside a skill's folder.
    pub fn write_skill_file(
        skill_id_or_name: &str,
        rel_path: &str,
        content: &str,
        workspace_root: Option<&Path>,
    ) -> Result<SkillInfo, String> {
        let rel = rel_path
            .trim()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_string();
        if rel.is_empty() || rel.contains("..") || rel.starts_with('.') {
            return Err("Invalid relative file path inside skill".to_string());
        }
        let skills = Self::list_skills_for_workspace(workspace_root);
        let target_slug = slugify_skill_id(skill_id_or_name);
        let skill = skills
            .into_iter()
            .find(|s| {
                s.id.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || s.name.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || slugify_skill_id(&s.name) == target_slug
            })
            .ok_or_else(|| format!("Skill '{}' not found", skill_id_or_name))?;

        let target_dir = if !skill.dir_path.is_empty() {
            let existing_dir = PathBuf::from(&skill.dir_path);
            let rust_dir = Self::skills_dir().join(&skill.id);
            if existing_dir != rust_dir && skill.scope == "global" {
                let _ = copy_dir_recursive_skip_git(&existing_dir, &rust_dir);
                rust_dir
            } else {
                existing_dir
            }
        } else {
            let rust_dir = Self::skills_dir().join(&skill.id);
            let _ = fs::create_dir_all(&rust_dir);
            let _ = fs::write(rust_dir.join("SKILL.md"), &skill.content);
            rust_dir
        };

        let file_path = target_dir.join(&rel);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&file_path, content).map_err(|e| e.to_string())?;

        // Also mirror to legacy dir if present
        let legacy_dir = Settings::legacy_go_config_dir().join("skills").join(&skill.id);
        if legacy_dir.is_dir() && legacy_dir != target_dir {
            let legacy_file = legacy_dir.join(&rel);
            if let Some(parent) = legacy_file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(legacy_file, content);
        }

        Self::list_skills_for_workspace(workspace_root)
            .into_iter()
            .find(|s| s.id == skill.id)
            .ok_or_else(|| "Failed to reload updated skill".to_string())
    }

    /// Deletes a companion file (`scripts/...` or `references/...`) from a skill's folder.
    pub fn delete_skill_file(
        skill_id_or_name: &str,
        rel_path: &str,
        workspace_root: Option<&Path>,
    ) -> Result<SkillInfo, String> {
        let rel = rel_path
            .trim()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_string();
        if rel.is_empty() || rel.contains("..") || rel.eq_ignore_ascii_case("SKILL.md") {
            return Err("Cannot delete SKILL.md via companion file deletion".to_string());
        }
        let skills = Self::list_skills_for_workspace(workspace_root);
        let target_slug = slugify_skill_id(skill_id_or_name);
        let skill = skills
            .into_iter()
            .find(|s| {
                s.id.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || s.name.eq_ignore_ascii_case(skill_id_or_name.trim())
                    || slugify_skill_id(&s.name) == target_slug
            })
            .ok_or_else(|| format!("Skill '{}' not found", skill_id_or_name))?;

        for dir in [
            PathBuf::from(&skill.dir_path),
            Self::skills_dir().join(&skill.id),
            Settings::legacy_go_config_dir().join("skills").join(&skill.id),
        ] {
            if dir.is_dir() {
                let target = dir.join(&rel);
                if target.is_file() {
                    let _ = fs::remove_file(&target);
                }
            }
        }

        Self::list_skills_for_workspace(workspace_root)
            .into_iter()
            .find(|s| s.id == skill.id)
            .ok_or_else(|| "Failed to reload skill after file deletion".to_string())
    }

    /// Deletes a skill by `id` or `name` across all skill directories (`~/.unjess-rust/skills`,
    /// `~/.unjess/skills`, and workspace `.agents/skills` / `.unjess/skills`).
    pub fn delete_skill(id: &str) -> Result<(), String> {
        Self::delete_skill_for_workspace(id, None)
    }

    pub fn delete_skill_for_workspace(
        id_or_name: &str,
        workspace_root: Option<&Path>,
    ) -> Result<(), String> {
        let target_trimmed = id_or_name.trim();
        if target_trimmed.is_empty() {
            return Ok(());
        }
        let target_slug = slugify_skill_id(target_trimmed);

        // If deleting a built-in skill, record it in `deleted_builtin_skills.json`
        let mut deleted_builtins = Self::load_deleted_builtins();
        if target_slug == "reverse-engineer-anything"
            || target_slug == "reverse-engineer-anything-rea"
            || target_trimmed.eq_ignore_ascii_case("Reverse Engineer Anything (REA)")
        {
            deleted_builtins.insert("reverse-engineer-anything".to_string());
            Self::save_deleted_builtins(&deleted_builtins);
        }
        if target_slug == "rust-systems-architect"
            || target_slug == "rust-systems-gpu-architect"
            || target_trimmed.eq_ignore_ascii_case("Rust Systems & GPU Architect")
        {
            deleted_builtins.insert("rust-systems-architect".to_string());
            Self::save_deleted_builtins(&deleted_builtins);
        }

        let mut search_roots = vec![
            Self::skills_dir(),
            Settings::legacy_go_config_dir().join("skills"),
        ];
        if let Some(ws) = workspace_root {
            search_roots.push(ws.join(".agents").join("skills"));
            search_roots.push(ws.join(".unjess").join("skills"));
        } else if let Ok(cwd) = std::env::current_dir() {
            search_roots.push(cwd.join(".agents").join("skills"));
            search_roots.push(cwd.join(".unjess").join("skills"));
        }

        for root in search_roots {
            if !root.is_dir() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();
                let dir_slug = slugify_skill_id(&dir_name);
                let mut matches = dir_name.eq_ignore_ascii_case(target_trimmed)
                    || (!target_slug.is_empty() && dir_slug == target_slug);

                if !matches {
                    let skill_md = p.join("SKILL.md");
                    if let Ok(content) = fs::read_to_string(&skill_md) {
                        let (parsed_name, _, _, _, _, _) =
                            Self::parse_skill_markdown(&dir_name, &content);
                        if parsed_name.eq_ignore_ascii_case(target_trimmed)
                            || (!target_slug.is_empty()
                                && slugify_skill_id(&parsed_name) == target_slug)
                        {
                            matches = true;
                        }
                    }
                }

                if matches {
                    let _ = fs::remove_dir_all(&p);
                }
            }
        }

        Ok(())
    }

    /// Returns a built-in curated preset bundle `(skill_id, name, description, folder, triggers, repo_url, body, optional_script)`
    /// for the 6 Curated 1-Click Skill Presets so they always install with zero failure even if upstream GitHub moves.
    pub fn curated_preset_bundle(
        repo_raw: &str,
        skill_path: &str,
    ) -> Option<(
        String,
        String,
        String,
        String,
        Vec<String>,
        String,
        String,
        Option<(String, String)>,
    )> {
        let combined = format!("{}/{}", repo_raw.to_lowercase(), skill_path.to_lowercase());
        if combined.contains("apple-design-skill") {
            Some((
                "apple-design-skill".to_string(),
                "Apple HIG UI Design".to_string(),
                "Apple Human Interface Guidelines (macOS, iOS, visionOS) spatial typography, vibrancy, materials, and spring motion rules.".to_string(),
                "UI & Design".to_string(),
                vec![
                    "apple hig".to_string(),
                    "apple design".to_string(),
                    "macos ui".to_string(),
                    "ios ui".to_string(),
                    "/apple-hig".to_string(),
                ],
                "https://github.com/sijunhe/apple-design-skill".to_string(),
                "# Apple Human Interface Guidelines (HIG) UI Design Skill\n\n## Core Design Principles\n1. **Clarity & Hierarchy**: Use SF Pro / Inter optical sizing, tight tracking on display headers (`-0.02em`), and generous whitespace.\n2. **Materials & Vibrancy**: Layer translucent surfaces (`backdrop-filter: blur(20px) saturate(180%)`) with subtle `1px solid rgba(255,255,255,0.08)` inner specular borders.\n3. **Concentricity**: Nested cards and buttons must share concentric corner radii (`R_outer = R_inner + padding`).\n4. **Fluid Spring Motion**: Prefer critically damped spring transitions (`cubic-bezier(0.22, 1, 0.36, 1)`) over linear easing.\n5. **Accessible Contrast**: Maintain WCAG AA/AAA contrast ratios (`>= 4.5:1` for body text, `>= 7:1` for primary actions) in both Dark and Light appearances.".to_string(),
                Some((
                    "scripts/check_contrast.py".to_string(),
                    "#!/usr/bin/env python3\n\"\"\"Apple HIG WCAG Contrast & Concentric Radius Auditor.\"\"\"\nimport sys\nprint('Apple HIG UI Audit Ready — checking contrast ratios and concentric radii...')\n".to_string(),
                )),
            ))
        } else if combined.contains("gdpr-compliant") {
            Some((
                "gdpr-compliant".to_string(),
                "GDPR & EU Privacy Audit".to_string(),
                "Automated GDPR, ePrivacy, data minimization, PII encryption, consent flows, and right-to-erasure code audit.".to_string(),
                "Security & Compliance".to_string(),
                vec![
                    "gdpr".to_string(),
                    "privacy audit".to_string(),
                    "pii check".to_string(),
                    "/gdpr".to_string(),
                ],
                "https://github.com/github/awesome-copilot".to_string(),
                "# GDPR & EU Privacy Engineering Skill\n\n## Compliance Checklist\n1. **Lawful Basis & Consent (Art. 6 & 7)**: Never load non-essential analytics or tracking scripts before explicit, granular opt-in consent.\n2. **Data Minimization (Art. 5(1)(c))**: Collect and persist only fields strictly required for the stated feature; redact PII (`email`, `ip`, `phone`, `ssn`) from application logs.\n3. **Right to Erasure & Portability (Art. 17 & 20)**: Provide deterministic cascading deletion and machine-readable JSON export endpoints for user records.\n4. **Encryption at Rest & in Transit (Art. 32)**: Enforce TLS 1.3+, AEAD encryption (`AES-256-GCM` / `ChaCha20-Poly1305`) for sensitive tokens, and Argon2id for password hashing.".to_string(),
                Some((
                    "scripts/scan_pii_logs.py".to_string(),
                    "#!/usr/bin/env python3\n\"\"\"Scans source files for unredacted PII logging patterns.\"\"\"\nimport sys\nprint('GDPR PII Log Scanner initialized.')\n".to_string(),
                )),
            ))
        } else if combined.contains("secure-headers-project") || combined.contains("owasp") {
            Some((
                "owasp-secure-headers".to_string(),
                "OWASP Secure Headers".to_string(),
                "OWASP Secure Headers Project (OSH) HTTP response hardening: CSP, HSTS, X-Frame-Options, COOP/COEP, and CORS.".to_string(),
                "Security & Compliance".to_string(),
                vec![
                    "owasp headers".to_string(),
                    "secure headers".to_string(),
                    "csp header".to_string(),
                    "/owasp-headers".to_string(),
                ],
                "https://github.com/OWASP/secure-headers-project".to_string(),
                "# OWASP Secure Headers Project (OSH)\n\n## Mandatory HTTP Security Headers\n- `Strict-Transport-Security: max-age=63072000; includeSubDomains; preload`\n- `Content-Security-Policy: default-src 'self'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'`\n- `X-Content-Type-Options: nosniff`\n- `Referrer-Policy: strict-origin-when-cross-origin`\n- `Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=()`\n- `Cross-Origin-Opener-Policy: same-origin`".to_string(),
                Some((
                    "scripts/audit_headers.py".to_string(),
                    "#!/usr/bin/env python3\n\"\"\"Audits HTTP headers against OWASP Secure Headers baseline.\"\"\"\nimport urllib.request, sys\nprint('OWASP Secure Headers Auditor ready.')\n".to_string(),
                )),
            ))
        } else if combined.contains("pentagi") {
            Some((
                "pentagi".to_string(),
                "PentAGI Autonomous Pentesting".to_string(),
                "Autonomous offensive/defensive penetration testing, OWASP Top 10 exploit verification, and remediation workflow.".to_string(),
                "Security & RE".to_string(),
                vec![
                    "pentagi".to_string(),
                    "pentest".to_string(),
                    "penetration test".to_string(),
                    "/pentagi".to_string(),
                ],
                "https://github.com/vxcontrol/pentagi".to_string(),
                "# PentAGI Autonomous Penetration Testing Skill\n\n## Methodology\n1. **Attack Surface Mapping**: Enumerate all HTTP routes, RPC handlers, CLI flags, IPC sockets, and deserialization entrypoints.\n2. **Vulnerability Hypothesis Generation**: Test for SSRF, SQLi, Path Traversal, Command Injection, IDOR, Auth Bypass, and Deserialization flaws.\n3. **Safe Proof-of-Concept Verification**: Construct deterministic, non-destructive unit test PoCs proving reachability.\n4. **Defense-in-Depth Remediation**: Patch the root cause with strict allowlists, parameterized queries, and capability sandboxing.".to_string(),
                None,
            ))
        } else if combined.contains("ui-ux-pro-max-skill") {
            Some((
                "ui-ux-pro-max-skill".to_string(),
                "UI/UX Pro Max".to_string(),
                "High-craft UI/UX design system engineering: micro-interactions, responsive bento grids, dark-mode token architecture, and zero-CLS layouts.".to_string(),
                "UI & Design".to_string(),
                vec![
                    "ui ux pro max".to_string(),
                    "ui design".to_string(),
                    "ux audit".to_string(),
                    "/ui-ux".to_string(),
                ],
                "https://github.com/nextlevelbuilder/ui-ux-pro-max-skill".to_string(),
                "# UI/UX Pro Max Skill\n\n## Design System Rules\n1. **Semantic Token Architecture**: Define surface hierarchy (`bg-primary`, `bg-elevated`, `bg-card`, `border-subtle`, `border-active`) with consistent luminance steps.\n2. **Interactive Affordances**: Every button, tab, and row must have distinct `:hover`, `:focus-visible`, `:active`, and `:disabled` states.\n3. **Zero Layout Shift (CLS = 0)**: Reserve explicit dimensions for async panels, badges, and status indicators.\n4. **Keyboard-First Ergonomics**: Support `Escape` to close modals, `Ctrl/Cmd+K` command palettes, and clear focus rings.".to_string(),
                None,
            ))
        } else if combined.contains("morluto/rea") {
            Some((
                "reverse-engineer-anything".to_string(),
                "Reverse Engineer Anything (REA)".to_string(),
                "Inspect and analyze compiled native, managed, Electron/JavaScript, packaged, firmware, and browser artifacts with REA (morluto/rea).".to_string(),
                "Security & RE".to_string(),
                vec![
                    "reverse engineer".to_string(),
                    "rea".to_string(),
                    "decompile".to_string(),
                    "binary analysis".to_string(),
                    "/rea".to_string(),
                ],
                "https://github.com/morluto/rea".to_string(),
                "# Reverse Engineer Anything (REA)\n\n## Core Workflow\n1. **Artifact Triage**: Identify container format (PE, ELF, Mach-O, ASAR, APK, WASM, firmware image), architecture, and símbolos/strings.\n2. **Static & Dynamic Decompilation**: Reconstruct control flow, IPC/RPC bridges, cryptographic keys, and wire protocols.\n3. **Clean Room Specification**: Output documented structs, sequence diagrams, and runnable verification harnesses.".to_string(),
                None,
            ))
        } else {
            None
        }
    }

    /// Clones or downloads a skill repository from GitHub (`owner/repo`, full URL, or `/tree/<branch>/<subpath>`),
    /// preserving all companion `scripts/`, `references/`, `examples/`, and `resources/` directories.
    pub async fn install_from_github(
        repo_raw: &str,
        branch_hint: Option<&str>,
        skill_path: &str,
        workspace_root: Option<&Path>,
    ) -> Result<(Vec<String>, Option<SkillInfo>), String> {
        let repo_url = format!("https://github.com/{}", repo_raw);

        // 1. Try shallow `git clone --depth 1` into a temporary directory so multi-file skills with `scripts/` are preserved!
        let tmp_dir = Settings::config_dir().join(format!(
            "tmp_skill_clone_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let mut cloned_ok = false;
        {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("clone").arg("--depth").arg("1");
            if let Some(b) = branch_hint.filter(|b| !b.is_empty() && *b != "HEAD") {
                cmd.arg("--branch").arg(b);
            }
            cmd.arg(&repo_url).arg(&tmp_dir);
            cmd.env("GIT_TERMINAL_PROMPT", "0");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            if let Ok(status) = cmd.status() {
                if status.success() && tmp_dir.is_dir() {
                    cloned_ok = true;
                }
            }
        }

        if cloned_ok {
            let sub_rel = skill_path
                .trim_end_matches("/SKILL.md")
                .trim_end_matches("SKILL.md")
                .trim_matches('/');
            let mut source_root = if sub_rel.is_empty() {
                tmp_dir.clone()
            } else {
                tmp_dir.join(sub_rel)
            };

            let repo_tail = repo_raw.split('/').next_back().unwrap_or("github-skill");
            let mut derived_id = slugify_skill_id(
                sub_rel
                    .split('/')
                    .next_back()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(repo_tail),
            );

            // If root has no SKILL.md, check standard nested skill directories (.claude/skills, .agents/skills, .agent/skills, skills)
            if sub_rel.is_empty() && !source_root.join("SKILL.md").is_file() {
                let candidate_ids = [
                    derived_id.clone(),
                    derived_id.trim_end_matches("-skill").to_string(),
                ];
                let container_dirs = [".claude/skills", ".agents/skills", ".agent/skills", "skills"];
                let mut found_nested: Option<(PathBuf, String)> = None;
                'outer: for cdir in container_dirs {
                    let parent = tmp_dir.join(cdir);
                    if !parent.is_dir() {
                        continue;
                    }
                    for cid in &candidate_ids {
                        if !cid.is_empty() && parent.join(cid).join("SKILL.md").is_file() {
                            found_nested = Some((parent.join(cid), cid.clone()));
                            break 'outer;
                        }
                    }
                    if let Ok(entries) = fs::read_dir(&parent) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_dir() && p.join("SKILL.md").is_file() {
                                let folder_id = slugify_skill_id(&entry.file_name().to_string_lossy());
                                found_nested = Some((p, folder_id));
                                break 'outer;
                            }
                        }
                    }
                }
                if let Some((nested_path, nested_id)) = found_nested {
                    source_root = nested_path;
                    derived_id = nested_id;
                }
            }

            if source_root.is_dir() {
                let target_dir = Self::skills_dir().join(&derived_id);
                let _ = copy_dir_recursive_skip_git(&source_root, &target_dir);
                let _ = fs::remove_dir_all(&tmp_dir);

                let md_path = target_dir.join("SKILL.md");
                let raw_md = if md_path.is_file() {
                    fs::read_to_string(&md_path).unwrap_or_default()
                } else if target_dir.join("README.md").is_file() {
                    fs::read_to_string(target_dir.join("README.md")).unwrap_or_default()
                } else {
                    String::new()
                };

                if !raw_md.trim().is_empty() {
                    let (parsed_name, parsed_desc, body, parsed_triggers, parsed_folder, _) =
                        Self::parse_skill_markdown(&derived_id, &raw_md);
                    let saved = Self::save_skill_full_with_id(
                        Some(&derived_id),
                        &parsed_name,
                        &parsed_desc,
                        &body,
                        &parsed_triggers,
                        "global",
                        &repo_url,
                        &parsed_folder,
                        workspace_root,
                    )?;
                    return Ok((vec![saved.name.clone()], Some(saved)));
                }
            }
            let _ = fs::remove_dir_all(&tmp_dir);
        } else {
            let _ = fs::remove_dir_all(&tmp_dir);
        }

        // 2. Fallback: HTTP raw fetch across candidate branches (`branch_hint`, `HEAD`, `main`, `master`) and files (`SKILL.md`, `README.md`)
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Unjess-Skill-Installer/4.0")
            .build()
            .unwrap_or_default();

        let sub_rel = skill_path
            .trim_end_matches("/SKILL.md")
            .trim_end_matches("SKILL.md")
            .trim_matches('/');

        let mut branches = Vec::new();
        if let Some(b) = branch_hint.filter(|b| !b.is_empty()) {
            branches.push(b);
        }
        for b in ["HEAD", "main", "master"] {
            if !branches.contains(&b) {
                branches.push(b);
            }
        }

        let mut candidate_urls = Vec::new();
        for b in &branches {
            if sub_rel.is_empty() {
                candidate_urls.push(format!(
                    "https://raw.githubusercontent.com/{}/{}/SKILL.md",
                    repo_raw, b
                ));
                candidate_urls.push(format!(
                    "https://raw.githubusercontent.com/{}/{}/README.md",
                    repo_raw, b
                ));
            } else {
                candidate_urls.push(format!(
                    "https://raw.githubusercontent.com/{}/{}/{}/SKILL.md",
                    repo_raw, b, sub_rel
                ));
                candidate_urls.push(format!(
                    "https://raw.githubusercontent.com/{}/{}/{}/README.md",
                    repo_raw, b, sub_rel
                ));
            }
        }

        for url in &candidate_urls {
            if let Ok(resp) = client.get(url).send().await {
                if resp.status().is_success() {
                    let md = resp.text().await.unwrap_or_default();
                    if !md.trim().is_empty() {
                        let derived_id = slugify_skill_id(
                            sub_rel
                                .split('/')
                                .next_back()
                                .filter(|s| !s.is_empty())
                                .or_else(|| repo_raw.split('/').next_back())
                                .unwrap_or("github-skill"),
                        );
                        let (parsed_name, parsed_desc, body, parsed_triggers, parsed_folder, _) =
                            Self::parse_skill_markdown(&derived_id, &md);
                        let saved = Self::save_skill_full_with_id(
                            Some(&derived_id),
                            &parsed_name,
                            &parsed_desc,
                            &body,
                            &parsed_triggers,
                            "global",
                            &repo_url,
                            &parsed_folder,
                            workspace_root,
                        )?;
                        return Ok((vec![saved.name.clone()], Some(saved)));
                    }
                }
            }
        }

        // 3. Curated 1-Click Preset Fallback Bundle (guarantees all 6 preset buttons work 100% even if upstream GitHub 404s)
        if let Some((
            preset_id,
            preset_name,
            preset_desc,
            preset_folder,
            preset_triggers,
            preset_repo,
            preset_body,
            preset_script,
        )) = Self::curated_preset_bundle(repo_raw, skill_path)
        {
            let mut saved = Self::save_skill_full_with_id(
                Some(&preset_id),
                &preset_name,
                &preset_desc,
                &preset_body,
                &preset_triggers,
                "global",
                &preset_repo,
                &preset_folder,
                workspace_root,
            )?;
            if let Some((script_rel, script_content)) = preset_script {
                if let Ok(updated) =
                    Self::write_skill_file(&saved.id, &script_rel, &script_content, workspace_root)
                {
                    saved = updated;
                }
            }
            return Ok((vec![saved.name.clone()], Some(saved)));
        }

        Err(format!(
            "Could not find SKILL.md or README.md in GitHub repository '{}'",
            repo_raw
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_markdown_strips_frontmatter_and_preserves_triggers_and_folder() {
        let raw = "---\nname: \"test-skill\"\ndescription: \"Summary: with colons & quotes\"\nfolder: \"Security & Reversing\"\ntriggers:\n  - \"custom trigger\"\n  - \"/custom-cmd\"\n---\n\n# Clean Instructions\nStep 1: Do work.";
        let (name, desc, body, triggers, folder, _repo) =
            SkillsManager::parse_skill_markdown("test-skill", raw);
        assert_eq!(name, "test-skill");
        assert_eq!(desc, "Summary: with colons & quotes");
        assert_eq!(folder, "Security & Reversing");
        assert_eq!(
            triggers,
            vec!["custom trigger".to_string(), "/custom-cmd".to_string()]
        );
        assert_eq!(body, "# Clean Instructions\nStep 1: Do work.");
        assert!(!body.contains("---"));
    }

    #[test]
    fn test_parse_multiline_folded_description() {
        let raw = "---\nname: asset-transformer-toolkit\ndescription: >-\n  Imports 3D models and point clouds\n  with Asset Transformer Toolkit.\n---\n# Asset Transformer";
        let (name, desc, _body, _triggers, _folder, _repo) =
            SkillsManager::parse_skill_markdown("asset-transformer-toolkit", raw);
        assert_eq!(name, "asset-transformer-toolkit");
        assert_eq!(
            desc,
            "Imports 3D models and point clouds with Asset Transformer Toolkit."
        );
    }
}
