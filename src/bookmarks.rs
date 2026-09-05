//! Two small lists kept beside the settings: the notes you have bookmarked,
//! and the vaults you have opened.
//!
//! Bookmarks live in `~/.config/catcher/bookmarks`, one path per line
//! relative to the vault root, the way Obsidian's own `bookmarks.json` keeps
//! them — which is where the list is seeded from the first time it is asked
//! for and does not exist yet. Recent vaults live in
//! `~/.config/catcher/vaults`, most recent first.

use std::fs;
use std::path::{Path, PathBuf};

/// How many vaults the recent list remembers.
pub const MAX_VAULTS: usize = 10;

fn bookmarks_path() -> Option<PathBuf> {
    crate::config::config_dir()
        .ok()
        .map(|d| d.join("bookmarks"))
}

fn vaults_path() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|d| d.join("vaults"))
}

/// The bookmarked notes, as root-relative paths, in file order. With no file
/// yet, the vault's Obsidian bookmarks are adopted and written back, so a
/// vault that had bookmarks starts with them.
pub fn load(root: &Path) -> Vec<String> {
    let Some(file) = bookmarks_path() else {
        return Vec::new();
    };
    match fs::read_to_string(&file) {
        Ok(text) => lines_of(&text),
        Err(_) => {
            let seeded = fs::read_to_string(root.join(".obsidian/bookmarks.json"))
                .map(|json| from_obsidian(&json))
                .unwrap_or_default();
            if !seeded.is_empty() {
                store(&file, &seeded);
            }
            seeded
        }
    }
}

/// Add `path` (under `root`) to the bookmarks, or take it out when it is
/// there already. `true` when it was added.
pub fn toggle(root: &Path, path: &Path) -> bool {
    let rel = relative(root, path);
    let mut list = load(root);
    let added = if let Some(i) = list.iter().position(|p| *p == rel) {
        list.remove(i);
        false
    } else {
        list.push(rel);
        true
    };
    if let Some(file) = bookmarks_path() {
        store(&file, &list);
    }
    added
}

/// `path` relative to `root`, or the whole path when it lives elsewhere.
fn relative(root: &Path, path: &Path) -> String {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (root, path) = (canon(root), canon(path));
    path.strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned()
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn store(file: &Path, list: &[String]) {
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let body: String = list.iter().map(|p| format!("{p}\n")).collect();
    let _ = fs::write(file, body);
}

/// The `path` of every `"type": "file"` item in an Obsidian `bookmarks.json`,
/// in order, without parsing the JSON: each item is a small object and its
/// two keys are found by name inside it. Groups nest their items in an
/// `items` array, which this walks through by reading every object.
pub fn from_obsidian(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find("\"type\"") {
        let after = &rest[i + "\"type\"".len()..];
        let Some(kind) = string_after_colon(after) else {
            break;
        };
        // the item's own keys come before the next item's `"type"`; a
        // group's `items` key opens another object, whose `"type"` is next
        let end = after.find("\"type\"").unwrap_or(after.len());
        let item = &after[..end];
        if kind == "file" {
            if let Some(path) = item
                .find("\"path\"")
                .and_then(|p| string_after_colon(&item[p + "\"path\"".len()..]))
            {
                if !path.is_empty() && !out.contains(&path) {
                    out.push(path);
                }
            }
        }
        rest = after;
    }
    out
}

/// The JSON string that follows `: ` at the start of `s`, with the common
/// escapes undone. `None` when what follows is not a string.
fn string_after_colon(s: &str) -> Option<String> {
    let s = s.trim_start().strip_prefix(':')?.trim_start();
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                other => out.push(other),
            },
            _ => out.push(c),
        }
    }
    None
}

/// The vaults opened before, most recent first. Folders that no longer exist
/// are dropped as they are read.
pub fn vaults() -> Vec<PathBuf> {
    let Some(file) = vaults_path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    lines_of(&text)
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .take(MAX_VAULTS)
        .collect()
}

/// Put `root` at the front of the recent vaults and write the file back.
pub fn push_vault(root: &Path) {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut list = vaults();
    list.retain(|p| *p != root);
    list.insert(0, root);
    list.truncate(MAX_VAULTS);
    if let Some(file) = vaults_path() {
        let body: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
        store(&file, &body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_bookmarks_yield_their_file_paths_in_order() {
        let json = r#"{
  "items": [
    { "type": "file", "ctime": 1, "path": "notes/spec.md", "title": "Spec" },
    { "type": "group", "title": "Work", "items": [
        { "type": "file", "path": "work/plan.md" },
        { "type": "folder", "path": "work" },
        { "type": "search", "query": "tag:#x" }
    ] },
    { "type": "file", "path": "with \"quote\".md" },
    { "type": "file", "path": "notes/spec.md" }
  ]
}"#;
        assert_eq!(
            from_obsidian(json),
            vec!["notes/spec.md", "work/plan.md", "with \"quote\".md"]
        );
        assert!(from_obsidian("{}").is_empty());
        assert!(from_obsidian("not json").is_empty());
    }

    #[test]
    fn a_path_under_the_root_is_kept_relative() {
        let dir = crate::testutil::tmpdir("bookmarks", "relative");
        let note = crate::testutil::write(&dir, "a/b.md", "# B\n");
        assert_eq!(relative(&dir, &note), "a/b.md");
        let _ = fs::remove_dir_all(&dir);
    }
}
