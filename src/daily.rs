//! The daily note: one file per day, `journal/2026-09-01.md`, made from a
//! template the first time it is asked for and never written by catcher
//! again. The ISO stem sorts in `ls` and is not a slug of the title, so the
//! rename-to-follow-title machinery in `notes::save` leaves it alone.

use crate::dates;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Where today's note lives, whether or not it exists.
pub fn path(daily_dir: &Path, (y, m, d): (i32, u32, u32)) -> PathBuf {
    daily_dir.join(format!("{}.md", dates::iso(y, m, d)))
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

/// The template with its variables filled in for `day`. `{{title}}` is the
/// long date, the rest are ISO.
pub fn render(template: &str, day: (i32, u32, u32)) -> String {
    let (y, m, d) = day;
    let (py, pm, pd) = dates::shift(y, m, d, -1);
    let (ny, nm, nd) = dates::shift(y, m, d, 1);
    template
        .replace("{{date}}", &dates::iso(y, m, d))
        .replace("{{title}}", &dates::long(y, m, d))
        .replace("{{yesterday}}", &dates::iso(py, pm, pd))
        .replace("{{tomorrow}}", &dates::iso(ny, nm, nd))
}

/// What a fresh note holds when there is no template file: a heading.
pub fn fallback_template() -> &'static str {
    "# {{title}}\n\n"
}

/// Make sure the note for `day` exists and return its path. An existing file
/// is not touched — not even to refresh a template variable — because it is
/// the user's journal by then.
pub fn ensure(daily_dir: &Path, template: &Path, day: (i32, u32, u32)) -> Result<PathBuf> {
    let path = path(daily_dir, day);
    if path.exists() {
        return Ok(path);
    }
    fs::create_dir_all(daily_dir).with_context(|| format!("creating {}", daily_dir.display()))?;
    let template = fs::read_to_string(template).unwrap_or_else(|_| fallback_template().to_string());
    fs::write(&path, render(&template, day))
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
            path(Path::new("/n/journal"), (2026, 9, 1)),
            PathBuf::from("/n/journal/2026-09-01.md")
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
        let t = "# {{title}}\n{{date}} ← {{yesterday}} → {{tomorrow}}\n{{date}}\n";
        assert_eq!(
            render(t, (2026, 9, 1)),
            "# Tuesday 1 September 2026\n2026-09-01 ← 2026-08-31 → 2026-09-02\n2026-09-01\n"
        );
        // a template with no variables is copied as it is
        assert_eq!(render("plain\n", (2026, 9, 1)), "plain\n");
    }

    #[test]
    fn a_missing_template_gives_a_heading_and_an_existing_note_is_left_alone() {
        let dir = scratch("once");
        let template = dir.join("template.md");
        let day = (2026, 9, 1);
        let p = ensure(&dir, &template, day).unwrap();
        assert_eq!(p, dir.join("2026-09-01.md"));
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "# Tuesday 1 September 2026\n\n"
        );
        // the user has written in it; a second open must not rewrite it
        fs::write(&p, "# mine\n").unwrap();
        fs::write(&template, "# {{date}}\n").unwrap();
        ensure(&dir, &template, day).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "# mine\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_template_file_is_rendered_into_a_new_note() {
        let dir = scratch("template");
        let template = dir.join("template.md");
        fs::write(&template, "# {{title}}\n\n[[{{yesterday}}]]\n").unwrap();
        let p = ensure(&dir.join("journal"), &template, (2026, 1, 1)).unwrap();
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "# Thursday 1 January 2026\n\n[[2025-12-31]]\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
