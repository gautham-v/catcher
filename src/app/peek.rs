//! The peek: a floating glimpse of the note a hovered or cursor-held link
//! points at, and the card offered when it points at no note yet.

use super::*;

/// A floating glimpse of another note, Obsidian-style.
#[derive(Clone, Debug)]
pub struct Peek {
    /// The link target it was opened for, so a hover over the same link is a
    /// no-op rather than a re-read.
    pub target: String,
    /// The note the link resolved to, which a click or Enter opens.
    pub path: PathBuf,
    /// False when the link names no note yet: the popup is then a card
    /// offering to make it, and opening it makes it.
    pub exists: bool,
    /// The file's name, which titles the popup.
    pub name: String,
    /// The note's markdown with any front matter already cut off.
    pub body: String,
    /// The screen band of the link it belongs to, which the popup sits beside.
    pub anchor: Rect,
    /// The whole note rendered at the popup's inner width, cached by the
    /// first draw and reused until the width changes.
    pub rows: Vec<ratatui::text::Line<'static>>,
    /// The width `rows` were rendered at; zero until the first draw.
    pub rows_width: usize,
    /// The first row on show.
    pub scroll: usize,
    /// How many rows the last draw had room for, which bounds the scroll.
    pub view_rows: usize,
    /// Where the last draw put the popup, for pointer hit-testing.
    pub rect: Rect,
}

impl Peek {
    /// The wikilink target the popup was opened for, without its `wikilink:`
    /// dress — what `create_from_link` wants.
    pub(super) fn target_name(&self) -> String {
        match md::LinkTarget::parse(&self.target) {
            md::LinkTarget::Wiki(t) => t,
            _ => self.target.clone(),
        }
    }

    /// Render the body for `width`, unless the cache already is.
    pub fn ensure_rendered(&mut self, width: usize, tables: crate::config::TableStyle) {
        if self.rows_width == width {
            return;
        }
        let first = self.rows_width == 0;
        let rendered = crate::render::render_page_at(&self.body, 0, width, tables);
        // wrapped at draw width the way the reading view is, so a paragraph
        // that overruns the popup folds instead of falling off its right edge;
        // a wide row (a table too broad to fold) stays one row, as it does there
        let mut goto = None;
        let anchor_line = self.anchor_line();
        self.rows.clear();
        for l in &rendered.lines {
            // the first row of the heading the link named
            if goto.is_none() && anchor_line.is_some_and(|a| l.src_line.is_some_and(|s| s >= a)) {
                goto = Some(self.rows.len());
            }
            if l.wide {
                self.rows.push(crate::render::to_line(&l.cells));
            } else {
                self.rows.extend(
                    crate::render::wrap_pline(l, width)
                        .iter()
                        .map(|cells| crate::render::to_line(cells)),
                );
            }
        }
        self.rows_width = width;
        // a `[[note#Heading]]` opens on that heading, once; a width change
        // later on keeps whatever the reader has scrolled to
        if let (true, Some(row)) = (first, goto) {
            self.scroll = row;
        }
        self.clamp();
    }

    /// The body line the peeked link's `#fragment` names, if it named one
    /// and the note has it.
    pub(super) fn anchor_line(&self) -> Option<usize> {
        let target = match md::LinkTarget::parse(&self.target) {
            md::LinkTarget::Wiki(t) => t,
            _ => return None,
        };
        let fragment = md::split_fragment(&target).1?;
        let lines: Vec<String> = self.body.lines().map(str::to_string).collect();
        crate::links::find_anchor(&lines, fragment)
    }

    /// The furthest `scroll` may go: the last row lands on the last line.
    pub fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.view_rows.max(1))
    }

    pub fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll() as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
    }

    /// Whether the pointer is inside the popup as last drawn.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.rect.contains(ratatui::layout::Position { x, y })
    }
}

/// The one sentence for a link to a note that is not there yet, shared by
/// the status bar and the peek card. `key` is whatever follow-link is bound
/// to, so a rebound key is named right.
pub(super) fn missing_link_hint(name: &str, key: &str) -> String {
    format!("no note called \u{201c}{name}\u{201d} \u{b7} {key} creates it")
}

/// How long the pointer rests on a link before it is taken as a request to
/// peek, rather than as a path across the page.
pub(super) const PEEK_DWELL: Duration = Duration::from_millis(300);

impl App {
    /// Open the peek for a hover that has lasted long enough.
    pub(super) fn maybe_peek(&mut self) -> bool {
        let Some((url, rect, since)) = self.hover.clone() else {
            return false;
        };
        if since.elapsed() < PEEK_DWELL || self.peek.as_ref().is_some_and(|p| p.target == url) {
            return false;
        }
        if let Some(peek) = self.load_peek(&url, rect) {
            self.peek = Some(peek);
        } else {
            // nothing to show: forget the hover so this is not retried every
            // tick for as long as the pointer sits there
            self.hover = None;
        }
        true
    }

    /// ⌥P: peek at the \[\[wikilink\]\] under the editor cursor, which is the
    /// only cursor the app has — the reading view is pointer-driven.
    pub(super) fn peek_at_cursor(&mut self) {
        let pos = self.editor.cursor;
        let target = self
            .editor
            .lines()
            .get(pos.0)
            .and_then(|l| md::link_at(l, pos.1));
        let url = match target {
            Some(t @ (md::LinkTarget::Wiki(_) | md::LinkTarget::Note(_))) => t.href(),
            _ => {
                self.flash("no wikilink here".to_string());
                return;
            }
        };
        // beside the cursor's row when it is on screen, else the top of the page
        let anchor = self
            .edit_rows
            .iter()
            .find(|r| r.line == pos.0)
            .map(|r| r.rect)
            .unwrap_or(Rect::new(
                self.editor_area.x,
                self.editor_area.y,
                self.editor_area.width,
                1,
            ));
        match self.load_peek(&url, anchor) {
            Some(p) => self.peek = Some(p),
            None => self.flash("no such note".to_string()),
        }
    }

    /// Read the note a link names, for a peek. A wikilink that resolves to
    /// nothing gets a card saying so instead of nothing at all — the hover
    /// that asked is the moment you want to know. Deliberately no vault
    /// re-walk here, which is a cost a hover must not pay.
    pub(super) fn load_peek(&self, url: &str, anchor: Rect) -> Option<Peek> {
        let path = match md::LinkTarget::parse(url) {
            md::LinkTarget::Note(p) => PathBuf::from(p),
            // `[[#Heading]]` peeks at the note on screen, at that heading
            md::LinkTarget::Wiki(t) if md::split_fragment(&t).0.is_empty() => {
                self.active_note().path.clone()
            }
            md::LinkTarget::Wiki(t) => match self.resolve_link(&t) {
                Some(p) => p,
                None => match best_title_match(&self.notes, md::split_fragment(&t).0) {
                    Some(i) => self.notes[i].path.clone(),
                    None => return Some(self.missing_peek(url, &t, anchor)),
                },
            },
            md::LinkTarget::Url(_) | md::LinkTarget::Tag(_) | md::LinkTarget::File(_) => {
                return None
            }
        };
        // an open note may have edits the disk has not seen yet
        let content = match self.notes.iter().find(|n| n.path == path) {
            Some(n) => n.content.clone(),
            None => std::fs::read_to_string(&path).ok()?,
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| url.to_string());
        Some(Peek {
            target: url.to_string(),
            path,
            exists: true,
            name,
            body: notes::body_after_front_matter(&content).to_string(),
            anchor,
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: Rect::default(),
        })
    }

    /// The card a peek at a link to nowhere shows: the same words the status
    /// bar uses, so the two never disagree about what the key does.
    pub(super) fn missing_peek(&self, url: &str, target: &str, anchor: Rect) -> Peek {
        let name = Self::link_note_path(target)
            .map(|(_, n)| n)
            .unwrap_or_else(|| target.to_string());
        Peek {
            target: url.to_string(),
            path: PathBuf::new(),
            exists: false,
            name: name.clone(),
            body: missing_link_hint(&name, &self.config.keys.label(Action::FollowLink)),
            anchor,
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: Rect::default(),
        }
    }

    /// Open the peeked note for real, putting the popup away. A peek at a
    /// note that is not there yet makes it, as following the link would.
    pub(super) fn open_peek(&mut self) {
        if let Some(peek) = self.peek.take() {
            self.hover = None;
            if peek.exists {
                self.open_path(&peek.path);
            } else {
                self.create_from_link(&peek.target_name());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_wraps_long_paragraphs_at_width() {
        let mut p = super::Peek {
            path: PathBuf::new(),
            exists: true,
            target: String::new(),
            name: String::new(),
            body: "# A heading that is longer than thirty columns\n\nAs Lead PM for a connected vehicle platform, I nudged a long paragraph across many rows.".into(),
            anchor: super::Rect::default(),
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 5,
            rect: super::Rect::default(),
        };
        p.ensure_rendered(30, crate::config::TableStyle::default());
        assert!(
            p.rows.len() > 2,
            "expected wrapped rows, got {}",
            p.rows.len()
        );
        for row in &p.rows {
            assert!(row.width() <= 30, "row wider than 30: {:?}", row);
        }
    }

    #[test]
    fn peek_scroll_clamps_to_content() {
        let mut p = super::Peek {
            path: PathBuf::new(),
            exists: true,
            target: String::new(),
            name: String::new(),
            body: String::new(),
            anchor: super::Rect::default(),
            rows: (0..20)
                .map(|i| ratatui::text::Line::from(i.to_string()))
                .collect(),
            rows_width: 10,
            scroll: 0,
            view_rows: 5,
            rect: super::Rect::default(),
        };
        assert_eq!(p.max_scroll(), 15);
        p.scroll_by(-3);
        assert_eq!(p.scroll, 0);
        p.scroll_by(7);
        assert_eq!(p.scroll, 7);
        p.scroll_by(100);
        assert_eq!(p.scroll, 15);
        // a shorter window than the content, and content shorter than the window
        p.view_rows = 30;
        p.clamp();
        assert_eq!(p.scroll, 0);
        assert_eq!(p.max_scroll(), 0);
    }

    #[test]
    fn the_missing_link_hint_names_the_note_and_the_key() {
        assert_eq!(
            super::missing_link_hint("plan", "⌥⏎"),
            "no note called \u{201c}plan\u{201d} \u{b7} ⌥⏎ creates it"
        );
    }

    #[test]
    fn a_peek_at_a_link_to_nowhere_is_a_card_that_offers_to_create() {
        let p = super::Peek {
            path: PathBuf::new(),
            exists: false,
            target: "wikilink:plan".into(),
            name: "plan".into(),
            body: super::missing_link_hint("plan", "⌥⏎"),
            anchor: super::Rect::default(),
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: super::Rect::default(),
        };
        assert_eq!(p.target_name(), "plan");
        assert!(p.body.contains("creates it"));
    }
}
