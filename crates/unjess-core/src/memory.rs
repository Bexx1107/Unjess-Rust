//! Persistent Cross-Session Memory Store (`~/.unjess-rust/memory.json`).

use crate::config::Settings;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// A single cross-session memory fact or architectural note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub category: String,
    pub key: String,
    pub content: String,
    pub created_at: String,
}

pub struct MemoryStore;

impl MemoryStore {
    fn memory_path() -> PathBuf {
        Settings::config_dir().join("memory.json")
    }

    /// Loads all saved memory entries from `~/.unjess-rust/memory.json`.
    pub fn load() -> Vec<MemoryEntry> {
        let path = Self::memory_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(entries) = serde_json::from_str::<Vec<MemoryEntry>>(&raw) {
                return entries;
            }
        }
        Vec::new()
    }

    fn save_all(entries: &[MemoryEntry]) {
        let path = Self::memory_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(entries) {
            let tmp_path = path.with_extension("json.tmp");
            if fs::write(&tmp_path, &json).is_ok() {
                if fs::rename(&tmp_path, &path).is_err() {
                    let _ = fs::write(&path, &json);
                    let _ = fs::remove_file(&tmp_path);
                }
            } else {
                let _ = fs::write(&path, &json);
            }
        }
    }

    /// Creates or updates a memory entry by `(category, key)`, moving it to the end (most recent)
    /// and persisting to disk.
    pub fn save_entry(category: &str, key: &str, content: &str) -> MemoryEntry {
        let mut entries = Self::load();
        let now = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

        if let Some(pos) = entries
            .iter()
            .position(|e| e.category.eq_ignore_ascii_case(category) && e.key.eq_ignore_ascii_case(key))
        {
            let mut existing = entries.remove(pos);
            existing.content = content.to_string();
            existing.created_at = now;
            entries.push(existing.clone());
            Self::save_all(&entries);
            return existing;
        }

        let entry = MemoryEntry {
            id: Uuid::new_v4().to_string(),
            category: category.to_string(),
            key: key.to_string(),
            content: content.to_string(),
            created_at: now,
        };
        entries.push(entry.clone());
        if entries.len() > 500 {
            let excess = entries.len() - 500;
            entries.drain(0..excess);
        }
        Self::save_all(&entries);
        entry
    }

    fn has_word_boundary_match(haystack: &str, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        let mut start = 0;
        while let Some(pos) = haystack[start..].find(needle) {
            let abs = start + pos;
            let end = abs + needle.len();
            let prev_ok = abs == 0
                || haystack[..abs]
                    .chars()
                    .next_back()
                    .map(|c| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(true);
            let next_ok = end >= haystack.len()
                || haystack[end..]
                    .chars()
                    .next()
                    .map(|c| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(true);
            if prev_ok && next_ok {
                return true;
            }
            start = abs + needle.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        false
    }

    /// Searches memory entries using multi-term weighted relevance scoring across `key`, `category`, and `content`.
    pub fn recall(query: &str) -> Vec<MemoryEntry> {
        let q = query.trim().to_lowercase();
        let all = Self::load();
        if q.is_empty() {
            return all.into_iter().rev().collect();
        }

        let mut tokens: Vec<&str> = Vec::new();
        for tok in q
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|t| t.len() >= 2)
        {
            if !tokens.contains(&tok) {
                tokens.push(tok);
            }
        }
        if tokens.is_empty() {
            tokens.push(q.as_str());
        }

        let mut scored: Vec<(usize, usize, MemoryEntry)> = all
            .into_iter()
            .enumerate()
            .filter_map(|(idx, e)| {
                let k_low = e.key.to_lowercase();
                let c_low = e.category.to_lowercase();
                let body_low = e.content.to_lowercase();

                let mut score = 0usize;
                if k_low.contains(&q) || c_low.contains(&q) || body_low.contains(&q) {
                    score += 5;
                }
                for &tok in &tokens {
                    if k_low.contains(tok) {
                        score += 3;
                        if Self::has_word_boundary_match(&k_low, tok) {
                            score += 2;
                        }
                    }
                    if c_low.contains(tok) {
                        score += 2;
                    }
                    if body_low.contains(tok) {
                        score += 1;
                        if Self::has_word_boundary_match(&body_low, tok) {
                            score += 2;
                        }
                    }
                }
                if score > 0 {
                    Some((score, idx, e))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        scored.into_iter().map(|(_, _, e)| e).collect()
    }

    /// Deletes a memory entry by `id` or `key`, returning `true` if removed.
    pub fn delete_entry(id_or_key: &str) -> bool {
        let mut entries = Self::load();
        let before = entries.len();
        entries.retain(|e| e.id != id_or_key && !e.key.eq_ignore_ascii_case(id_or_key));
        if entries.len() != before {
            Self::save_all(&entries);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_crud() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("UNJESS_RUST_CONFIG_DIR", tmp.path());
        }
        let saved = MemoryStore::save_entry("arch", "backend", "100% Native Rust");
        let recalled = MemoryStore::recall("Native Rust");
        assert_eq!(recalled.len(), 1);
        let multi_token = MemoryStore::recall("arch Rust");
        assert_eq!(multi_token.len(), 1);
        assert!(MemoryStore::delete_entry(&saved.id));
        assert!(MemoryStore::load().is_empty());
    }
}
