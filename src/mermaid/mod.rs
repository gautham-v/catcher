//! Mermaid fences, drawn with box-drawing characters.
//!
//! A ```mermaid fence is a diagram someone typed, not a picture someone
//! attached: it is text in the file, and catcher draws it the way it draws a
//! table, a rule or a callout card — itself, in the terminal, with no image
//! protocol, no network and no dependency. What cannot be drawn is left as the
//! code it already was, so a diagram kind we have never heard of degrades to
//! exactly what a fence looked like yesterday.
//!
//! The output is deliberately the smallest shape both views can consume. The
//! reading view ([`crate::render`]) wants runs of styled characters it can turn
//! into `PCell`s; the live-preview editor ([`crate::md`]) wants the same runs
//! turned into `Cell`s that all map back to the fence's own source line. So a
//! diagram is [`Rendered`]: rows of [`Run`]s, each a string and a [`Role`], and
//! each caller maps the four roles onto the palette itself. No `Style` crosses
//! this boundary — the theme stays in one place, at the top of `md.rs`.

pub mod canvas;
pub mod flow;
pub mod sequence;

/// What a run of characters in a diagram *is*, so the caller can style it
/// without the diagram knowing anything about colour.
///
/// Four roles, because the palette is monochrome and a diagram is chrome the
/// note draws around its own words: the accent belongs to headings, and a
/// diagram that reached for it would compete with them.
///
/// The mapping each caller is expected to make:
/// `Node` → `theme::PLAIN`, `Line` → `theme::marker()`,
/// `Label` → `theme::grey()`, `Bright` → `theme::bright()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Box edges, connecting lines, arrowheads: the scaffolding.
    Line,
    /// The text inside a node — the words the author actually wrote.
    Node,
    /// An edge label: text on a connection rather than in a box.
    Label,
    /// A name that leads: a subgraph title, a sequence participant.
    Bright,
}

/// One run of characters that share a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub role: Role,
}

impl Run {
    pub fn new(text: impl Into<String>, role: Role) -> Run {
        Run {
            text: text.into(),
            role,
        }
    }
}

/// One drawn row of a diagram.
pub type Row = Vec<Run>;

/// A drawn diagram: rows of styled runs, and the widest of them.
///
/// Rows are already trimmed on the right, and a row may be wider than the page
/// it was asked for — the reading view pans a wide diagram the way it pans a
/// wide table, and the editor cuts it. Fitting is a preference, not a promise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub rows: Vec<Row>,
    /// Widest row, in display columns.
    pub width: usize,
}

impl Rendered {
    /// Collect drawn rows into a diagram, measuring as it goes.
    pub fn new(rows: Vec<Row>) -> Rendered {
        let width = rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|run| crate::md::str_width(&run.text))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        Rendered { rows, width }
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// The plain text of every row, for tests.
    #[cfg(test)]
    pub fn text(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }
}

/// Which way a flowchart runs.
///
/// `TB` is mermaid's own spelling of `TD` and means the same thing; `BT` and
/// anything else unrecognised is tolerated and laid out top-down, since a
/// direction we cannot honour is no reason to refuse the diagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Left to right: layers are columns.
    Lr,
    /// Right to left: layers are columns, filled from the right.
    Rl,
    /// Top down: layers are rows.
    Td,
}

/// The diagram kinds catcher can draw. Anything else is not a `Kind`, which is
/// how the caller learns to fall back to a labelled fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Flow(Dir),
    Sequence,
}

/// Whether a fence's info string opens a mermaid diagram. The info string is
/// whatever followed the backticks, so `mermaid`, `Mermaid` and
/// `mermaid {theme: dark}` all count — the attributes are somebody else's
/// renderer's business, and a fence that names mermaid is a diagram either way.
pub fn is_mermaid(info: &str) -> bool {
    info.trim()
        .split(|c: char| c.is_whitespace() || c == '{')
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("mermaid"))
}

/// The first word of the first line that says anything — the word mermaid
/// itself uses to pick a renderer. `%%` comments and blank lines above it are
/// skipped, so a note that starts its diagram with a comment still names its
/// kind. This is what a fallback label has to show: telling someone their
/// `gantt` is not drawn is useful; telling them "mermaid" is not.
pub fn kind_word(src: &str) -> Option<String> {
    header(src).map(|(word, _)| word)
}

/// The diagram kind, or `None` for one catcher does not draw.
pub fn kind_of(src: &str) -> Option<Kind> {
    let (word, rest) = header(src)?;
    match word.to_ascii_lowercase().as_str() {
        // `graph` is the old spelling of `flowchart` and still the commonest
        "flowchart" | "graph" => Some(Kind::Flow(direction(&rest))),
        "sequencediagram" => Some(Kind::Sequence),
        _ => None,
    }
}

/// Draw `src` for a page `width` columns wide, or `None` when this is not a
/// diagram we draw — an unknown kind, or one whose body says nothing we could
/// make a picture of. Both answers mean the same thing to the caller: leave
/// the fence as the code it was, with a label saying what it is.
pub fn render(src: &str, width: usize) -> Option<Rendered> {
    match kind_of(src)? {
        Kind::Flow(dir) => flow::render(src, dir, width),
        Kind::Sequence => sequence::render(src, width),
    }
}

/// The first meaningful line, split into its first word and the rest. A
/// trailing `;` is mermaid's optional statement terminator and is not part of
/// the word.
fn header(src: &str) -> Option<(String, String)> {
    let line = src
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"))?;
    let line = line.trim_end_matches(';').trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let word = parts.next()?.trim_end_matches(';').to_string();
    if word.is_empty() {
        return None;
    }
    Some((word, parts.next().unwrap_or("").trim().to_string()))
}

/// The direction word after `flowchart`/`graph`, if it named one we honour.
fn direction(rest: &str) -> Dir {
    let word = rest
        .split(|c: char| c.is_whitespace() || c == ';')
        .find(|w| !w.is_empty())
        .unwrap_or("");
    match word.to_ascii_uppercase().as_str() {
        "LR" => Dir::Lr,
        "RL" => Dir::Rl,
        _ => Dir::Td,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fence_that_names_mermaid_is_a_diagram() {
        assert!(is_mermaid("mermaid"));
        assert!(is_mermaid("Mermaid"));
        assert!(is_mermaid("mermaid {init: {}}"));
        assert!(!is_mermaid("rust"));
        assert!(!is_mermaid("mermaidjs"));
        assert!(!is_mermaid(""));
    }

    #[test]
    fn a_flowchart_header_names_its_direction() {
        assert_eq!(kind_of("flowchart LR\nA --> B"), Some(Kind::Flow(Dir::Lr)));
        assert_eq!(kind_of("graph RL;\nA --> B"), Some(Kind::Flow(Dir::Rl)));
        assert_eq!(kind_of("flowchart TB\nA --> B"), Some(Kind::Flow(Dir::Td)));
        assert_eq!(kind_of("graph TD\nA --> B"), Some(Kind::Flow(Dir::Td)));
        // no direction at all, and one we don't honour, both fall to top-down
        assert_eq!(kind_of("flowchart\nA --> B"), Some(Kind::Flow(Dir::Td)));
        assert_eq!(kind_of("flowchart BT\nA --> B"), Some(Kind::Flow(Dir::Td)));
    }

    #[test]
    fn comments_and_blank_lines_above_the_header_are_skipped() {
        let src = "\n%% drawn by hand\nsequenceDiagram\n  A->>B: hi";
        assert_eq!(kind_of(src), Some(Kind::Sequence));
        assert_eq!(kind_word(src).as_deref(), Some("sequenceDiagram"));
    }

    #[test]
    fn an_unsupported_diagram_kind_is_not_drawn() {
        let src = "classDiagram\n  Animal <|-- Duck";
        assert_eq!(kind_of(src), None);
        assert_eq!(render(src, 80), None);
        // but the fallback still knows what to call it
        assert_eq!(kind_word(src).as_deref(), Some("classDiagram"));
        assert_eq!(kind_word("   \n\n"), None);
    }

    #[test]
    fn a_diagram_measures_its_widest_row() {
        let r = Rendered::new(vec![
            vec![Run::new("──", Role::Line)],
            vec![Run::new("│ ", Role::Line), Run::new("hi", Role::Node)],
        ]);
        assert_eq!(r.width, 4);
        assert_eq!(r.height(), 2);
        assert_eq!(r.text(), vec!["──".to_string(), "│ hi".to_string()]);
    }
}
