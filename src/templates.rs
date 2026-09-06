//! The templates folder: Obsidian's core Templates plugin, as two commands.
//! `list` is what the picker shows — every `.md` under `templates_dir`, by
//! name — and `render` fills the placeholders in. The daily note goes through
//! the same renderer, so a file reads the same whichever command fills it in.

use crate::dates::{self, Now};
use crate::search;
use std::path::{Path, PathBuf};

/// The default `templates_dir`, under the notes dir.
pub const DEFAULT_DIR: &str = "templates";

/// One template file: where it is, and what the picker calls it.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub path: PathBuf,
    pub name: String,
}

/// What a template is called: its filename without `.md`. The folder it sits
/// in is not part of it — the name is what someone typed when they saved it.
pub fn name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every `.md` under `dir`, subfolders included, in name order. Dotted
/// folders are stepped over, the way the vault walk skips `.obsidian`. A
/// folder that is not there is simply an empty list: the setting names where
/// templates would go, not somewhere that has to exist.
pub fn list(dir: &Path) -> Vec<Template> {
    fn walk(dir: &Path, out: &mut Vec<Template>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "md") {
                out.push(Template {
                    name: name(&path),
                    path,
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// The templates whose name answers `query`, best first. An empty query
/// scores every name the same, so the list stays in the order `list` gave it.
pub fn filter<'a>(all: &'a [Template], query: &str) -> Vec<&'a Template> {
    let mut scored: Vec<(i64, &Template)> = all
        .iter()
        .filter_map(|t| search::fuzzy(query, &t.name).map(|s| (s, t)))
        .collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().map(|(_, t)| t).collect()
}

/// The template with its variables filled in for `now`. `{{title}}` is
/// `title` — the note the text is going into; `{{date}}`, `{{yesterday}}`
/// and `{{tomorrow}}` are in `format` so they link to the notes those days
/// get; `{{time}}` is `HH:mm`; `{{date:FMT}}` and `{{time:FMT}}` take any
/// format. An unknown variable is left as it was typed.
pub fn render(template: &str, title: &str, format: &str, now: Now) -> String {
    let ((y, m, d), time) = now;
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let (name, fmt) = match after[..end].split_once(':') {
            Some((n, f)) => (n.trim(), Some(f)),
            None => (after[..end].trim(), None),
        };
        let value = match (name, fmt) {
            ("title", None) => Some(title.to_string()),
            ("date", fmt) => Some(dates::format(fmt.unwrap_or(format), now)),
            ("time", fmt) => Some(dates::format(fmt.unwrap_or("HH:mm"), now)),
            ("yesterday", None) => Some(dates::format(format, (dates::shift(y, m, d, -1), time))),
            ("tomorrow", None) => Some(dates::format(format, (dates::shift(y, m, d, 1), time))),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{tmpdir, write};

    #[test]
    fn the_picker_lists_every_md_under_the_folder_by_name() {
        let dir = tmpdir("templates", "list");
        write(&dir, "meeting.md", "# {{title}}\n");
        write(&dir, "work/standup.md", "- \n");
        write(&dir, "notes.txt", "not a template\n");
        write(&dir, ".hidden/secret.md", "# Secret\n");
        let all = list(&dir);
        let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["meeting", "standup"]);
        assert_eq!(all[1].path, dir.join("work/standup.md"));
        // a folder that is not there lists nothing rather than failing
        assert!(list(&dir.join("nowhere")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_query_narrows_the_list_by_name() {
        let all = vec![
            Template {
                path: PathBuf::from("/t/meeting notes.md"),
                name: "meeting notes".to_string(),
            },
            Template {
                path: PathBuf::from("/t/standup.md"),
                name: "standup".to_string(),
            },
        ];
        // nothing typed leaves the order alone
        assert_eq!(filter(&all, "").len(), 2);
        assert_eq!(filter(&all, "")[0].name, "meeting notes");
        // fuzzy on the name, not on the path
        assert_eq!(filter(&all, "mtg n")[0].name, "meeting notes");
        assert_eq!(filter(&all, "stand").len(), 1);
        assert!(filter(&all, "zzz").is_empty());
        assert!(filter(&all, "/t/").is_empty());
    }

    #[test]
    fn every_template_variable_is_filled_in() {
        let now = ((2026, 9, 1), (14, 5, 0));
        let t = "# {{title}}\n{{date}} ← {{yesterday}} → {{tomorrow}} {{time}}\n";
        assert_eq!(
            render(t, "Standup", "YYYY-MM-DD", now),
            "# Standup\n2026-09-01 ← 2026-08-31 → 2026-09-02 14:05\n"
        );
        // a custom format, and a variable nobody knows left as it was typed
        assert_eq!(
            render(
                "{{date:dddd Do MMMM}} {{who}} {{date",
                "x",
                "YYYY-MM-DD",
                now
            ),
            "Tuesday 1st September {{who}} {{date"
        );
        // a template with no variables is copied as it is
        assert_eq!(render("plain\n", "x", "YYYY-MM-DD", now), "plain\n");
    }
}
