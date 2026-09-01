//! Browser-style back/forward between notes.
//!
//! Every note the app lands on is pushed here, by path. Going back then
//! opening something new drops the forward entries, the way a browser does;
//! the same note twice in a row is recorded once; and a note that has since
//! been deleted is skipped over rather than opened as an error.

use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct History {
    entries: Vec<PathBuf>,
    /// Index into `entries` of the note on screen. `None` only before the
    /// first push.
    at: Option<usize>,
}

impl History {
    /// Record `path` as the note now on screen. Drops anything forward of
    /// the current position, and does nothing if `path` is already current.
    pub fn push(&mut self, path: &Path) {
        if let Some(i) = self.at {
            if self.entries[i] == path {
                return;
            }
            self.entries.truncate(i + 1);
        }
        self.entries.push(path.to_path_buf());
        self.at = Some(self.entries.len() - 1);
    }

    /// The nearest earlier note that `exists`, made current. Entries that
    /// fail the check are removed on the way past.
    pub fn back(&mut self, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        self.step(-1, exists)
    }

    /// The nearest later note that `exists`, made current.
    pub fn forward(&mut self, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        self.step(1, exists)
    }

    #[cfg(test)]
    pub fn current(&self) -> Option<&Path> {
        self.at.map(|i| self.entries[i].as_path())
    }

    #[cfg(test)]
    pub fn can_back(&self) -> bool {
        self.at.is_some_and(|i| i > 0)
    }

    #[cfg(test)]
    pub fn can_forward(&self) -> bool {
        self.at.is_some_and(|i| i + 1 < self.entries.len())
    }

    fn step(&mut self, dir: isize, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        let mut i = self.at?;
        loop {
            let next = i.checked_add_signed(dir)?;
            if next >= self.entries.len() {
                return None;
            }
            if exists(&self.entries[next]) {
                self.at = Some(next);
                return Some(self.entries[next].clone());
            }
            // gone from disk: forget it, and look one further along. Removing
            // an entry before the cursor shifts the cursor with it.
            self.entries.remove(next);
            if dir < 0 {
                i -= 1;
                self.at = Some(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn all(_: &Path) -> bool {
        true
    }

    #[test]
    fn back_and_forward_walk_the_stack() {
        let mut h = History::default();
        h.push(&p("a"));
        h.push(&p("b"));
        h.push(&p("c"));
        assert_eq!(h.back(all), Some(p("b")));
        assert_eq!(h.back(all), Some(p("a")));
        assert_eq!(h.back(all), None);
        assert_eq!(h.forward(all), Some(p("b")));
        assert_eq!(h.forward(all), Some(p("c")));
        assert_eq!(h.forward(all), None);
    }

    #[test]
    fn opening_after_going_back_drops_the_forward_entries() {
        let mut h = History::default();
        h.push(&p("a"));
        h.push(&p("b"));
        h.push(&p("c"));
        h.back(all);
        h.back(all);
        h.push(&p("d"));
        assert!(!h.can_forward());
        assert_eq!(h.back(all), Some(p("a")));
        assert_eq!(h.forward(all), Some(p("d")));
        assert_eq!(h.forward(all), None);
    }

    #[test]
    fn the_same_note_twice_in_a_row_is_recorded_once() {
        let mut h = History::default();
        h.push(&p("a"));
        h.push(&p("a"));
        assert!(!h.can_back());
        h.push(&p("b"));
        h.push(&p("b"));
        assert_eq!(h.back(all), Some(p("a")));
        assert_eq!(h.back(all), None);
    }

    #[test]
    fn going_back_to_the_current_note_keeps_forward_history() {
        let mut h = History::default();
        h.push(&p("a"));
        h.push(&p("b"));
        h.back(all);
        // re-landing on the current entry is not a new visit
        h.push(&p("a"));
        assert!(h.can_forward());
        assert_eq!(h.forward(all), Some(p("b")));
    }

    #[test]
    fn a_deleted_note_is_skipped_and_forgotten() {
        let mut h = History::default();
        h.push(&p("a"));
        h.push(&p("gone"));
        h.push(&p("c"));
        let exists = |x: &Path| x != Path::new("gone");
        assert_eq!(h.back(exists), Some(p("a")));
        assert_eq!(h.current(), Some(Path::new("a")));
        assert_eq!(h.forward(exists), Some(p("c")));
        assert_eq!(h.back(exists), Some(p("a")));
        assert_eq!(h.forward(exists), Some(p("c")));
    }

    #[test]
    fn nothing_left_when_every_earlier_note_is_gone() {
        let mut h = History::default();
        h.push(&p("x"));
        h.push(&p("y"));
        h.push(&p("z"));
        assert_eq!(h.back(|x| x == Path::new("z")), None);
        assert_eq!(h.current(), Some(Path::new("z")));
        assert!(!h.can_back());
    }

    #[test]
    fn empty_history_goes_nowhere() {
        let mut h = History::default();
        assert_eq!(h.back(all), None);
        assert_eq!(h.forward(all), None);
        assert_eq!(h.current(), None);
    }
}
