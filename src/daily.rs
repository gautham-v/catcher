//! The daily note: one file per day, `journal/2026-09-01.md`, made from a
//! template the first time it is asked for and never written by catcher
//! again. The stem is `daily_format` — ISO by default, which sorts in `ls`
//! and is not a slug of the title, so the rename-to-follow-title machinery
//! in `notes::save` leaves it alone. A slash in the format is a subfolder.

use crate::dates::{self, Now};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The default `daily_format`.
pub const DEFAULT_FORMAT: &str = "YYYY-MM-DD";

/// Where the note for `now` lives, whether or not it exists: the format
/// filled in, `.md` on the end, under the daily dir.
pub fn path(daily_dir: &Path, format: &str, now: Now) -> PathBuf {
    daily_dir.join(format!("{}.md", dates::format(format, now)))
}

/// A setting that is a folder or file under the notes dir unless it is
/// spelled absolute.
pub fn resolve(notes_dir: &Path, setting: &Path) -> PathBuf {
    if setting.is_absolute() {
        setting.to_path_buf()
    } else {
        notes_dir.join(setting)
    }
}

/// The template with its variables filled in for `now`. The pass itself is
/// [`crate::templates::render`], which *Insert template* uses too; a daily
/// note only decides what `{{title}}` is — the long date, since that is what
/// the day's note is called.
pub fn render(template: &str, format: &str, now: Now) -> String {
    let ((y, m, d), _) = now;
    crate::templates::render(template, &dates::long(y, m, d), format, now)
}

/// What a fresh note holds when there is no template file: a heading.
pub fn fallback_template() -> &'static str {
    "# {{title}}\n\n"
}

/// Make sure the note for `now` exists and return its path. An existing file
/// is not touched — not even to refresh a template variable — because it is
/// the user's journal by then.
pub fn ensure(daily_dir: &Path, format: &str, template: &Path, now: Now) -> Result<PathBuf> {
    let path = path(daily_dir, format, now);
    if path.exists() {
        return Ok(path);
    }
    let parent = path.parent().unwrap_or(daily_dir);
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let template = fs::read_to_string(template).unwrap_or_else(|_| fallback_template().to_string());
    fs::write(&path, render(&template, format, now))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("catcher-daily-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_file_is_the_iso_date_under_the_daily_dir() {
        assert_eq!(
            path(
                Path::new("/n/journal"),
                DEFAULT_FORMAT,
                ((2026, 9, 1), (0, 0, 0))
            ),
            PathBuf::from("/n/journal/2026-09-01.md")
        );
        // a slash in the format is a subfolder
        assert_eq!(
            path(
                Path::new("/n/journal"),
                "YYYY/MM/DD-MM-YYYY",
                ((2026, 9, 1), (0, 0, 0))
            ),
            PathBuf::from("/n/journal/2026/09/01-09-2026.md")
        );
    }

    #[test]
    fn a_relative_setting_sits_under_the_notes_dir_and_an_absolute_one_stands() {
        let n = Path::new("/n");
        assert_eq!(
            resolve(n, Path::new("journal")),
            PathBuf::from("/n/journal")
        );
        assert_eq!(
            resolve(n, Path::new("journal/template.md")),
            PathBuf::from("/n/journal/template.md")
        );
        assert_eq!(
            resolve(n, Path::new("/vault/daily")),
            PathBuf::from("/vault/daily")
        );
    }

    #[test]
    fn every_template_variable_is_filled_in() {
        let now = ((2026, 9, 1), (14, 5, 0));
        let t = "# {{title}}\n{{date}} ← {{yesterday}} → {{tomorrow}}\n{{date}}\n";
        assert_eq!(
            render(t, DEFAULT_FORMAT, now),
            "# Tuesday 1 September 2026\n2026-09-01 ← 2026-08-31 → 2026-09-02\n2026-09-01\n"
        );
        // a template with no variables is copied as it is
        assert_eq!(render("plain\n", DEFAULT_FORMAT, now), "plain\n");
        // the links follow daily_format; time and custom formats fill in
        assert_eq!(
            render(
                "[[{{yesterday}}]] {{time}} {{date:dddd Do MMMM}} {{time:h:mm a}}",
                "DD-MM-YYYY",
                now
            ),
            "[[31-08-2026]] 14:05 Tuesday 1st September 2:05 pm"
        );
        // an unknown variable and an unclosed one are left alone
        assert_eq!(
            render("{{who}} {{date", DEFAULT_FORMAT, now),
            "{{who}} {{date"
        );
    }

    #[test]
    fn a_missing_template_gives_a_heading_and_an_existing_note_is_left_alone() {
        let dir = scratch("once");
        let template = dir.join("template.md");
        let day = (2026, 9, 1);
        let p = ensure(&dir, DEFAULT_FORMAT, &template, (day, (0, 0, 0))).unwrap();
        assert_eq!(p, dir.join("2026-09-01.md"));
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "# Tuesday 1 September 2026\n\n"
        );
        // the user has written in it; a second open must not rewrite it
        fs::write(&p, "# mine\n").unwrap();
        fs::write(&template, "# {{date}}\n").unwrap();
        ensure(&dir, DEFAULT_FORMAT, &template, (day, (0, 0, 0))).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "# mine\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_template_file_is_rendered_into_a_new_note() {
        let dir = scratch("template");
        let template = dir.join("template.md");
        fs::write(&template, "# {{title}}\n\n[[{{yesterday}}]]\n").unwrap();
        let p = ensure(
            &dir.join("journal"),
            DEFAULT_FORMAT,
            &template,
            ((2026, 1, 1), (0, 0, 0)),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "# Thursday 1 January 2026\n\n[[2025-12-31]]\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nested_format_makes_the_subfolders() {
        let dir = scratch("nested");
        let template = dir.join("template.md");
        fs::write(&template, "{{date}} after {{yesterday}}\n").unwrap();
        let p = ensure(
            &dir.join("journal"),
            "YYYY/MM/DD-MM-YYYY",
            &template,
            ((2026, 9, 1), (0, 0, 0)),
        )
        .unwrap();
        assert_eq!(p, dir.join("journal/2026/09/01-09-2026.md"));
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "2026/09/01-09-2026 after 2026/08/31-08-2026\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
