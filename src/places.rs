//! Where you were in each note, so coming back to one lands where you left
//! it rather than at the top.
//!
//! Session only, and keyed by path: ⌥[ back to a note, a `[[link]]` to it, or
//! picking it from ^O all return to the same place. Nothing is stored beside
//! the file. A note may have been edited, by hand or by another program,
//! between leaving and returning, so every position is clamped against the
//! text about to be shown rather than trusted.

use crate::editor::Pos;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// The editor's cursor.
    pub cursor: Pos,
    /// The editor's first visible source line.
    pub scroll: usize,
    /// The source line at the top of the reading view, when the note was
    /// left from the reading view. `None` when it was left from the editor:
    /// the page then opens where the editor is, as toggling to it does.
    pub top: Option<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct Places {
    by_path: HashMap<PathBuf, Place>,
}

impl Places {
    /// Record where `path` was left.
    pub fn remember(&mut self, path: &Path, place: Place) {
        self.by_path.insert(path.to_path_buf(), place);
    }

    /// Where `path` was left, fitted to a note of `lines` lines: a note that
    /// shrank since gives back a place inside it, not past its end. The
    /// column is left to the editor, which clamps it to the line.
    pub fn recall(&self, path: &Path, lines: usize) -> Option<Place> {
        let place = self.by_path.get(path)?;
        let last = lines.saturating_sub(1);
        Some(Place {
            cursor: (place.cursor.0.min(last), place.cursor.1),
            scroll: place.scroll.min(last),
            top: place.top.map(|t| t.min(last)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_note_never_left_has_no_place() {
        let places = Places::default();
        assert_eq!(places.recall(&p("a"), 10), None);
    }

    #[test]
    fn a_place_comes_back_as_it_was_left() {
        let mut places = Places::default();
        let left = Place {
            cursor: (7, 3),
            scroll: 5,
            top: Some(6),
        };
        places.remember(&p("a"), left);
        assert_eq!(places.recall(&p("a"), 20), Some(left));
        // and each note keeps its own
        places.remember(
            &p("b"),
            Place {
                cursor: (1, 0),
                scroll: 0,
                top: None,
            },
        );
        assert_eq!(places.recall(&p("a"), 20), Some(left));
    }

    #[test]
    fn leaving_again_replaces_the_place() {
        let mut places = Places::default();
        let first = Place {
            cursor: (7, 3),
            scroll: 5,
            top: Some(6),
        };
        let second = Place {
            cursor: (2, 0),
            scroll: 0,
            top: None,
        };
        places.remember(&p("a"), first);
        places.remember(&p("a"), second);
        assert_eq!(places.recall(&p("a"), 20), Some(second));
    }

    #[test]
    fn a_note_that_shrank_gives_a_place_inside_it() {
        let mut places = Places::default();
        places.remember(
            &p("a"),
            Place {
                cursor: (40, 2),
                scroll: 30,
                top: Some(35),
            },
        );
        assert_eq!(
            places.recall(&p("a"), 10),
            Some(Place {
                cursor: (9, 2),
                scroll: 9,
                top: Some(9),
            })
        );
        // an emptied note still has its one line
        assert_eq!(
            places.recall(&p("a"), 0),
            Some(Place {
                cursor: (0, 2),
                scroll: 0,
                top: Some(0),
            })
        );
    }
}
