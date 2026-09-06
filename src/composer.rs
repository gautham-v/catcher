//! Taking part of a note out into one of its own, and folding a whole note
//! into another.
//!
//! Both commands are text work before they are anything else — which lines
//! are taken, what stands in their place, where the arriving body lands and
//! what its front matter has to carry with it — so the shaping lives here as
//! plain functions over strings, and app.rs is left with the prompt, the
//! picker and the files.

use crate::fold;
use crate::md::Block;
use crate::notes;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// How long a title made from a line of prose may be. Past this it stops
/// reading as a name and starts reading as the paragraph it was cut from.
const TITLE_MAX: usize = 60;

/// What an extract leaves in the source where the taken lines were.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Leave {
    /// `[[Title]]`: the note is somewhere else now, and this is the way to it.
    #[default]
    Link,
    /// `![[Title]]`: the note is somewhere else and still reads here.
    Embed,
    /// Nothing at all: the lines simply go.
    Nothing,
}

/// The choices in the order the prompt lists them, with the word each is
/// drawn as.
pub const LEAVES: [(Leave, &str); 3] = [
    (Leave::Link, "[[link]]"),
    (Leave::Embed, "![[embed]]"),
    (Leave::Nothing, "nothing"),
];

impl Leave {
    /// Tab steps to the next choice, and round again.
    pub fn next(self) -> Leave {
        match self {
            Leave::Link => Leave::Embed,
            Leave::Embed => Leave::Nothing,
            Leave::Nothing => Leave::Link,
        }
    }

    /// The line that stands where the taken lines were, or `None` for the
    /// choice that leaves nothing.
    pub fn line(self, name: &str) -> Option<String> {
        match self {
            Leave::Link => Some(format!("[[{name}]]")),
            Leave::Embed => Some(format!("![[{name}]]")),
            Leave::Nothing => None,
        }
    }
}

/// The lines a heading owns — itself, down to the end of its section — or
/// `None` when `row` is not a heading. What an extract takes when there is
/// no selection to take instead.
pub fn section_range(lines: &[String], blocks: &[Block], row: usize) -> Option<(usize, usize)> {
    fold::heading_at(lines, blocks, row)?;
    Some((row, fold::section_end(lines, blocks, row)))
}

/// What the prompt offers as the new note's name: the first heading among
/// the taken lines, or else their first line of prose, cut to a length that
/// still reads as a title.
pub fn title_prefill(taken: &[String]) -> String {
    if let Some(heading) = taken.iter().find_map(|l| crate::md::heading_text(l)) {
        if !heading.is_empty() {
            return heading.to_string();
        }
    }
    let first = taken
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    first
        .chars()
        .take(TITLE_MAX)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The taken lines as the new note's body: verbatim, heading levels and all,
/// the way Obsidian's own extract leaves them.
fn taken(lines: &[String], (from, to): (usize, usize)) -> String {
    let mut body = lines[from..=to.min(lines.len() - 1)].join("\n");
    body.push('\n');
    body
}

/// `range` taken out of `lines` into a note of its own in `dir`: where the
/// note landed, and the lines that stand in its place.
///
/// The file is named for the title as it was typed rather than for its slug,
/// because the link left behind names the file (see [`notes::create_named`]);
/// a title already on disk takes a number, and the link follows the name the
/// file actually got.
pub fn extract(
    dir: &Path,
    lines: &[String],
    range: (usize, usize),
    title: &str,
    leave: Leave,
) -> Result<(PathBuf, Vec<String>)> {
    let note = notes::create_named(dir, title, taken(lines, range))?;
    let name = note
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| title.to_string());
    Ok((note.path, leave.line(&name).into_iter().collect()))
}

/// The heading level a merged note is filed under: one deeper than the
/// target's top-most heading, and `##` when it has none or opens at `#`.
/// Deeper than six is not a heading any more, so it stops there.
pub fn merge_level(target: &str) -> usize {
    let top = notes::prose_lines(target)
        .filter_map(|(_, l)| fold::heading_level(l))
        .min();
    match top {
        Some(level) if level >= 2 => (level + 1).min(6),
        _ => 2,
    }
}

/// `target` with `source` folded into it under a heading called `title`: the
/// front matter carried over first, then a blank line, the heading, and the
/// arriving body with its own front matter cut off.
pub fn merge(target: &str, source: &str, title: &str) -> String {
    let level = merge_level(target);
    let mut out = carry_front_matter(target, source, title);
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&format!("{} {title}\n", "#".repeat(level)));
    let body = notes::body_after_front_matter(source).trim();
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// The values a front matter key holds, under either spelling of its name and
/// however they were written: a scalar, an inline `[a, b]`, or a block list.
/// A bare `tags: a, b` is the list it was meant to be.
fn values(content: &str, keys: [&str; 2]) -> Vec<String> {
    keys.iter()
        .flat_map(|key| crate::md::front_matter_values(content, key))
        .flat_map(|v| {
            v.split(',')
                .map(|p| p.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// `have` with everything in `add` that is not already there, compared
/// without case: `#Work` and `#work` are one tag.
fn union(have: &[String], add: &[String]) -> Vec<String> {
    let mut out = have.to_vec();
    for v in add {
        if !out.iter().any(|h| h.eq_ignore_ascii_case(v)) {
            out.push(v.clone());
        }
    }
    out
}

/// One `  - item` line, quoted only when YAML would read it as something
/// other than the string it is.
fn item(value: &str) -> String {
    if value.starts_with(['#', '[', '{', '&', '*', '\'', '"']) || value.contains(": ") {
        format!("  - \"{}\"\n", value.replace('"', "'"))
    } else {
        format!("  - {value}\n")
    }
}

/// A key and its values, as a block list.
fn property(key: &str, values: &[String]) -> String {
    let mut out = format!("{key}:\n");
    for v in values {
        out.push_str(&item(v));
    }
    out
}

/// `target` with the source's `tags:` and `aliases:` folded into its front
/// matter, and the source's title added as an alias so `[[Old]]` still lands
/// on the note that swallowed it.
///
/// A key the target already has is rewritten as a block list, values and all;
/// a target with no front matter grows one only when there is something to
/// carry into it.
fn carry_front_matter(target: &str, source: &str, title: &str) -> String {
    let mut add_aliases = values(source, ["aliases", "alias"]);
    let title = title.trim();
    if !title.is_empty() {
        add_aliases.push(title.to_string());
    }
    // each key with the values arriving under it, and the spellings the
    // target may have written it as; the first spelling is the one a target
    // that lacks the key gets
    let wanted = [
        (["tags", "tag"], values(source, ["tags", "tag"])),
        (["aliases", "alias"], add_aliases),
    ];

    let Some(range) = notes::front_matter_range(target) else {
        let block: String = wanted
            .iter()
            .filter(|(_, add)| !add.is_empty())
            .map(|(names, add)| property(names[0], add))
            .collect();
        if block.is_empty() {
            return target.to_string();
        }
        return format!("---\n{block}---\n{target}");
    };

    let props = crate::md::front_matter_properties(target);
    let fence = notes::front_matter_end(target.lines()).unwrap_or(0);
    // which lines each rewritten key occupies: its own, plus whatever list
    // items sit under it before the next key or the closing fence
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (names, add) in &wanted {
        let Some(p) = props.iter().find(|p| names.contains(&p.key.as_str())) else {
            continue;
        };
        let end = props
            .iter()
            .map(|o| o.line)
            .filter(|&l| l > p.line)
            .min()
            .unwrap_or(fence);
        // the key keeps the spelling the note gave it: a vault that writes
        // `tag:` should not find `tags:` written under it as well
        spans.push((p.line, end, property(&p.key, &union(&p.values, add))));
    }

    let mut out = String::new();
    let mut skip_to = 0;
    for (i, line) in target[range.clone()].split_inclusive('\n').enumerate() {
        if i < skip_to {
            continue;
        }
        if i == fence {
            // the keys the target did not have yet, written in before the
            // closing fence
            for (names, add) in &wanted {
                if add.is_empty() || props.iter().any(|p| names.contains(&p.key.as_str())) {
                    continue;
                }
                out.push_str(&property(names[0], add));
            }
        }
        match spans.iter().find(|(start, ..)| *start == i) {
            Some((_, end, text)) => {
                out.push_str(text);
                skip_to = *end;
            }
            None => out.push_str(line),
        }
    }
    out.push_str(&target[range.end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    fn blocks(lines: &[String]) -> Vec<Block> {
        crate::app::blocks_with(lines, crate::config::FrontMatter::Dim)
    }

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::tmpdir("composer", name)
    }

    #[test]
    fn a_section_is_its_heading_down_to_the_next_one_of_the_same_level() {
        let note = lines("# Top\n\nintro\n\n## Method\n\nsteps\n\n### Detail\n\nmore\n\n## Next\n");
        let b = blocks(&note);
        assert_eq!(section_range(&note, &b, 4), Some((4, 11)));
        // a deeper heading is part of the section above it
        assert_eq!(section_range(&note, &b, 8), Some((8, 11)));
        // the top heading owns the whole note
        assert_eq!(section_range(&note, &b, 0), Some((0, 12)));
        // and a line that is not a heading has no section to take
        assert_eq!(section_range(&note, &b, 2), None);
    }

    #[test]
    fn the_prefill_is_the_first_heading_or_the_first_line_cut_short() {
        assert_eq!(
            title_prefill(&lines("\n## Method ##\n\ntext")),
            "Method".to_string()
        );
        assert_eq!(
            title_prefill(&lines("\n  a stray thought\nand more")),
            "a stray thought".to_string()
        );
        let long = "x".repeat(80);
        assert_eq!(title_prefill(&lines(&long)).chars().count(), TITLE_MAX);
        assert_eq!(title_prefill(&[]), String::new());
    }

    #[test]
    fn extracting_leaves_a_link_where_the_lines_were() {
        let dir = tmpdir("extract");
        let note = lines("# Note\n\n## Method\n\nsteps\n\n## Next\n");
        let (path, replacement) = extract(&dir, &note, (2, 5), "Method", Leave::Link).unwrap();
        assert_eq!(path, dir.join("Method.md"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "## Method\n\nsteps\n\n"
        );
        assert_eq!(replacement, vec!["[[Method]]".to_string()]);
        // the other two choices leave an embed, or nothing at all, and tab
        // goes round the three of them
        assert_eq!(Leave::Embed.line("Method").as_deref(), Some("![[Method]]"));
        assert_eq!(Leave::Nothing.line("Method"), None);
        assert_eq!(Leave::Link.next().next().next(), Leave::Link);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_title_already_on_disk_takes_a_number_and_the_link_follows_it() {
        let dir = tmpdir("unique");
        let note = lines("steps\n");
        let (first, link) = extract(&dir, &note, (0, 0), "Method", Leave::Link).unwrap();
        assert_eq!(first, dir.join("Method.md"));
        assert_eq!(link, vec!["[[Method]]".to_string()]);
        let (second, link) = extract(&dir, &note, (0, 0), "Method", Leave::Link).unwrap();
        assert_eq!(second, dir.join("Method-2.md"));
        assert_eq!(link, vec!["[[Method-2]]".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_arriving_heading_is_one_level_under_the_targets_top_one() {
        assert_eq!(merge_level("# Title\n\n## Part\n"), 2);
        assert_eq!(merge_level("## Part\n\n### Bit\n"), 3);
        assert_eq!(merge_level("prose, no headings\n"), 2);
        assert_eq!(merge_level("###### Deep\n"), 6);
        // a `#` in a fence is not a heading, and neither is one in front matter
        assert_eq!(merge_level("```sh\n# comment\n```\n"), 2);
    }

    #[test]
    fn merging_appends_the_body_under_a_heading_of_its_own() {
        // the alias the merge carries is already there, so the front matter
        // is left as it stands and only the body arrives
        let front = "---\naliases:\n  - Spec\n---\n";
        let out = merge(
            &format!("{front}# Plan\n\ntext\n"),
            "# Spec\n\ndetails\n",
            "Spec",
        );
        assert_eq!(
            out,
            format!("{front}# Plan\n\ntext\n\n## Spec\n# Spec\n\ndetails\n")
        );
        // a target whose top heading is `##` takes the arrival at `###`
        let out = merge(&format!("{front}## Plan\n"), "notes\n", "Spec");
        assert_eq!(out, format!("{front}## Plan\n\n### Spec\nnotes\n"));
    }

    #[test]
    fn the_front_matter_of_both_notes_ends_up_on_the_target() {
        let target = "---\ntags:\n  - plan\naliases: [Roadmap]\nstatus: open\n---\n# Plan\n";
        let source = "---\ntags: spec, plan\nalias: The Spec\n---\n# Spec\n";
        let out = merge(target, source, "Spec");
        assert!(out.starts_with(
            "---\ntags:\n  - plan\n  - spec\naliases:\n  - Roadmap\n  - The Spec\n  - Spec\nstatus: open\n---\n"
        ));
        // the note's own title is an alias too, so [[Spec]] keeps landing
        assert!(out.contains("\n## Spec\n"));
    }

    #[test]
    fn a_target_without_front_matter_grows_one_only_to_carry_something() {
        let out = merge("# Plan\n", "---\ntags: [spec]\n---\n# Spec\n", "Spec");
        assert!(out.starts_with("---\ntags:\n  - spec\naliases:\n  - Spec\n---\n# Plan\n"));
        // nothing to carry but the title, which is still something
        let out = merge("# Plan\n", "# Spec\n", "Spec");
        assert!(out.starts_with("---\naliases:\n  - Spec\n---\n# Plan\n"));
        // and a note with no title at all leaves the target as it was
        let out = merge("# Plan\n", "body\n", "");
        assert!(out.starts_with("# Plan\n"));
    }

    #[test]
    fn a_value_yaml_would_misread_is_written_quoted() {
        let out = merge("# Plan\n", "---\ntags: ['#work']\n---\n", "Note: a thing");
        assert!(out.contains("  - \"#work\"\n"), "{out}");
        assert!(out.contains("  - \"Note: a thing\"\n"), "{out}");
    }
}
