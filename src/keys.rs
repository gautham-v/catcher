//! User-settable key bindings.
//!
//! Every global action catcher has is named here once, with the key it
//! answers to. The settings file overrides any of them by name, the palette
//! shows each command's current key beside it, and the help card is generated
//! from the same table — so a rebound key is right everywhere, and an action
//! that isn't in this list has no key and isn't discoverable.
//!
//! Editor motions (⌥←, ⌘⌫, and the rest) are deliberately not here: they are
//! the platform's text-editing conventions, not catcher's opinions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Something a key can do. The order is the order the help card lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Palette,
    QuickOpen,
    NewNote,
    Settings,
    TogglePreview,
    Save,
    Help,
    Quit,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    DeleteNote,
    RenameFile,
    FollowLink,
    NavBack,
    NavForward,
    Peek,
    SearchAll,
    DailyNote,
}

/// Every action: its settings key, its default binding, and what it does.
/// `None` for a default means the action ships unbound and is reached through
/// the palette until someone gives it a key.
const ACTIONS: &[(Action, &str, Option<&str>, &str)] = &[
    (
        Action::Palette,
        "key_palette",
        Some("^K"),
        "command palette — run a command",
    ),
    (
        Action::QuickOpen,
        "key_open",
        Some("^O"),
        "open a note — every folder, recent first",
    ),
    (Action::NewNote, "key_new", Some("^N"), "new note"),
    (
        Action::Settings,
        "key_settings",
        Some("^,"),
        "settings, as a note you can edit",
    ),
    (
        Action::TogglePreview,
        "key_preview",
        Some("^P"),
        "toggle the reading view",
    ),
    (
        Action::Save,
        "key_save",
        Some("^S"),
        "save now (notes autosave anyway)",
    ),
    // ^/ on a legacy terminal arrives as ^_ (0x1F), and on some as ^7; those
    // are folded into ^/ in `matches`. F1 is there for terminals that send
    // none of them.
    (Action::Help, "key_help", Some("^/ f1"), "this card"),
    (Action::Quit, "key_quit", Some("^Q"), "save and quit"),
    (Action::Copy, "key_copy", Some("^C"), "copy selection"),
    (Action::Cut, "key_cut", Some("^X"), "cut selection"),
    (
        Action::Paste,
        "key_paste",
        Some("^V"),
        "paste — an image becomes an attachment",
    ),
    (Action::Undo, "key_undo", Some("^Z"), "undo"),
    (Action::Redo, "key_redo", Some("^Y"), "redo  (⇧^Z too)"),
    (
        Action::DeleteNote,
        "key_delete",
        None,
        "delete the note on screen",
    ),
    (
        Action::RenameFile,
        "key_rename",
        None,
        "rename the file on disk",
    ),
    // alt+enter and not plain enter, because enter has to keep inserting a
    // newline; and not ctrl+enter, because plenty of terminals do not report
    // that as anything distinct from enter. `key_follow:` overrides it.
    (
        Action::FollowLink,
        "key_follow",
        Some("alt+enter"),
        "open the [[wikilink]] or #tag under the cursor, or make the note a link names",
    ),
    // every modifier + arrow already moves the cursor (or, with ^⌥, the
    // window), so the browser keys are letters: back and forward.
    (
        Action::NavBack,
        "key_back",
        Some("^B"),
        "back to the note you came from",
    ),
    (
        Action::NavForward,
        "key_forward",
        Some("^F"),
        "forward again",
    ),
    (
        Action::Peek,
        "key_peek",
        // capital, because the label writes it as ⌥P and the settings
        // document is read back through `parse`, which keeps the letter's case
        Some("alt+P"),
        "peek at the [[wikilink]] under the cursor",
    ),
    // ⇧^F, so it sits beside ^F and ^O. A terminal without the kitty
    // protocol sends ⇧^F as plain ^F, which is forward; there the palette
    // is the way in.
    (
        Action::SearchAll,
        "key_search",
        // capital for the same reason as ⌥P: the label writes it as ⇧F
        Some("ctrl+shift+F"),
        "search in all files",
    ),
    // alt, like peek: every ctrl letter near it is spoken for. Capital for
    // the same round-trip reason as ⌥P.
    (
        Action::DailyNote,
        "key_daily",
        Some("alt+D"),
        "today's note, made from the template if new",
    ),
];

/// A settings key that used to go by another name: the old spelling is still
/// read, the new one is what gets written.
const ALIASES: &[(&str, &str)] = &[("key_help", "key_shortcuts")];

/// Defaults that a settings file may still carry from an earlier version. The
/// settings note is written out with every key filled in, so a default that
/// changes would otherwise stay pinned to the old key on every machine that
/// ever ran the old build. A value equal to a superseded default is treated
/// as "never set" and follows the current default instead.
const SUPERSEDED: &[(&str, &[&str])] = &[
    ("key_help", &["^G"]),
    ("key_back", &["⌥←", "alt+left", "ctrl+⌥←", "ctrl+alt+left"]),
    (
        "key_forward",
        &["⌥→", "alt+right", "ctrl+⌥→", "ctrl+alt+right"],
    ),
];

fn superseded(key: &str, spec: &str) -> bool {
    let spec = spec.trim();
    SUPERSEDED
        .iter()
        .find(|(k, _)| *k == key)
        .is_some_and(|(_, olds)| olds.iter().any(|o| o.eq_ignore_ascii_case(spec)))
}

/// One key, as the settings file spells it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Binding {
    code: KeyCode,
    /// `^X` is happy with either ctrl or cmd — a Mac hand reaches for ⌘ and a
    /// Linux one for ctrl, and neither is wrong. `cmd+x` and `ctrl+x` written
    /// out are exact.
    ctrl_or_cmd: bool,
    ctrl: bool,
    cmd: bool,
    alt: bool,
    shift: bool,
}

impl Binding {
    /// Parse a spec of one or more keys — `^/ f1`, `^K, cmd+k` — into every
    /// key it names. Empty for `none` and for anything unreadable.
    pub fn parse_all(text: &str) -> Vec<Binding> {
        // split on whitespace only: `^,` is itself a key, so a comma can
        // separate keys only when it trails one — `^K, f5`
        text.split_whitespace()
            .filter_map(|t| {
                Binding::parse(t).or_else(|| t.strip_suffix(',').and_then(Binding::parse))
            })
            .collect()
    }

    /// Parse `^K`, `ctrl+k`, `cmd+,`, `alt+p`, `f5`, `⌘k`. `None` for anything
    /// unreadable, and for the word `none`, which unbinds.
    pub fn parse(text: &str) -> Option<Binding> {
        let t = text.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("off") {
            return None;
        }
        let mut b = Binding {
            code: KeyCode::Null,
            ctrl_or_cmd: false,
            ctrl: false,
            cmd: false,
            alt: false,
            shift: false,
        };
        // the caret spelling: the one everybody already uses for ctrl keys
        if let Some(rest) = t.strip_prefix('^') {
            b.ctrl_or_cmd = true;
            b.code = key_code(rest)?;
            return Some(b);
        }
        // strip modifier prefixes off the front, in any order, until what is
        // left is the key itself
        const CTRL: &[&str] = &["ctrl+", "control+"];
        const CMD: &[&str] = &["cmd+", "super+", "meta+", "⌘"];
        const ALT: &[&str] = &["alt+", "opt+", "option+", "⌥"];
        const SHIFT: &[&str] = &["shift+", "⇧"];
        let mut rest = t;
        loop {
            let lower = rest.to_ascii_lowercase();
            let hit =
                [(CTRL, 0), (CMD, 1), (ALT, 2), (SHIFT, 3)]
                    .iter()
                    .find_map(|(prefixes, which)| {
                        let p = prefixes.iter().find(|p| lower.starts_with(**p))?;
                        Some((p.len(), *which))
                    });
            let Some((len, which)) = hit else { break };
            match which {
                0 => b.ctrl = true,
                1 => b.cmd = true,
                2 => b.alt = true,
                _ => b.shift = true,
            }
            rest = &rest[len..];
        }
        b.code = key_code(rest)?;
        // a bare letter is not a binding: it is what you type into a note.
        // A function key is, since it types nothing.
        let bare_ok = matches!(b.code, KeyCode::F(_));
        (bare_ok || b.ctrl || b.cmd || b.alt).then_some(b)
    }

    /// Does this binding answer to `key`?
    pub fn matches(&self, key: &KeyEvent) -> bool {
        let m = key.modifiers;
        let ctrl = m.contains(KeyModifiers::CONTROL);
        let cmd = m.contains(KeyModifiers::SUPER);
        let alt = m.contains(KeyModifiers::ALT);
        if !same_key(self.code, normalize(key.code, ctrl)) {
            return false;
        }
        // a binding that asks for shift needs it; one that does not is
        // indifferent, because a capital letter arrives with SHIFT set on
        // some terminals and not on others
        if self.shift && !m.contains(KeyModifiers::SHIFT) {
            return false;
        }
        if self.ctrl_or_cmd {
            // shift is not checked: ⇧^Z reaching redo is a feature, and a
            // capital letter arrives with SHIFT set on some terminals anyway
            return (ctrl || cmd) && !alt;
        }
        ctrl == self.ctrl && cmd == self.cmd && alt == self.alt
    }

    /// How this binding is written — in the palette, the card, the settings.
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.ctrl_or_cmd {
            s.push('^');
        }
        if self.ctrl {
            s.push_str("ctrl+");
        }
        if self.cmd {
            s.push('⌘');
        }
        if self.alt {
            s.push('⌥');
        }
        if self.shift {
            s.push('⇧');
        }
        match self.code {
            KeyCode::Char(c) => s.push(c.to_ascii_uppercase()),
            KeyCode::F(n) => s.push_str(&format!("F{n}")),
            KeyCode::Esc => s.push_str("esc"),
            KeyCode::Enter => s.push('⏎'),
            KeyCode::Tab => s.push_str("tab"),
            KeyCode::Left => s.push('←'),
            KeyCode::Right => s.push('→'),
            KeyCode::Up => s.push('↑'),
            KeyCode::Down => s.push('↓'),
            _ => s.push('?'),
        }
        s
    }
}

/// What a terminal reports for a ctrl chord is not always the key that was
/// pressed: ^/ is byte 0x1F, which crossterm hands back as ctrl+`_`, and a
/// few terminals send it as ctrl+`7`. Fold those into ^/ so one binding
/// answers to all of them.
fn normalize(code: KeyCode, ctrl: bool) -> KeyCode {
    match code {
        KeyCode::Char('_') | KeyCode::Char('7') if ctrl => KeyCode::Char('/'),
        _ => code,
    }
}

/// Case-insensitive for letters: `^K` and `^k` are the same key.
fn same_key(a: KeyCode, b: KeyCode) -> bool {
    match (a, b) {
        (KeyCode::Char(x), KeyCode::Char(y)) => x.eq_ignore_ascii_case(&y),
        _ => a == b,
    }
}

fn key_code(text: &str) -> Option<KeyCode> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f') {
        if let Ok(n) = n.parse::<u8>() {
            if (1..=12).contains(&n) {
                return Some(KeyCode::F(n));
            }
        }
    }
    Some(match lower.as_str() {
        "esc" | "escape" => KeyCode::Esc,
        // the glyphs are here so `label` round-trips back through `parse`:
        // the settings document is generated from the labels, and a key that
        // could be written but not read would be silently lost on the rewrite
        "enter" | "return" | "⏎" | "↵" => KeyCode::Enter,
        "tab" | "⇥" => KeyCode::Tab,
        "left" | "←" => KeyCode::Left,
        "right" | "→" => KeyCode::Right,
        "up" | "↑" => KeyCode::Up,
        "down" | "↓" => KeyCode::Down,
        "space" => KeyCode::Char(' '),
        _ => {
            let mut chars = t.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(KeyCode::Char(c))?
        }
    })
}

/// Every action's current binding.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    bound: Vec<(Action, Vec<Binding>)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            bound: ACTIONS
                .iter()
                .map(|(a, _, default, _)| (*a, default.map(Binding::parse_all).unwrap_or_default()))
                .collect(),
        }
    }
}

impl Keymap {
    /// Read the `key_*` settings over the defaults. `lookup` is handed each
    /// action's settings key and returns what the file said, if anything.
    pub fn from_settings(lookup: impl Fn(&str) -> Option<String>) -> Keymap {
        let mut map = Keymap::default();
        for (action, key, _, _) in ACTIONS {
            let old = ALIASES
                .iter()
                .find(|(new, _)| new == key)
                .map(|(_, old)| *old);
            let spec = lookup(key).or_else(|| old.and_then(&lookup));
            if let Some(spec) = spec.filter(|s| !superseded(key, s)) {
                // an unreadable spec unbinds rather than silently keeping the
                // default, so a typo is visible instead of mysterious
                map.set(*action, Binding::parse_all(&spec));
            }
        }
        map
    }

    fn set(&mut self, action: Action, binding: Vec<Binding>) {
        if let Some(slot) = self.bound.iter_mut().find(|(a, _)| *a == action) {
            slot.1 = binding;
        }
    }

    /// Which action `key` triggers, if any. First match wins, so a key bound
    /// twice by hand runs the action listed first rather than both — except
    /// that a binding spelled with shift beats one without when shift is
    /// down, or ⇧^F could never be anything but ^F.
    pub fn action(&self, key: &KeyEvent) -> Option<Action> {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let hits = self
            .bound
            .iter()
            .filter_map(|(a, bs)| bs.iter().find(|b| b.matches(key)).map(|b| (*a, b.shift)));
        let mut best: Option<(Action, bool)> = None;
        for hit in hits {
            match best {
                None => best = Some(hit),
                Some((_, exact)) if shift && hit.1 && !exact => best = Some(hit),
                _ => {}
            }
        }
        best.map(|(a, _)| a)
    }

    /// Every key bound to `action`, first the one hints show.
    pub fn bindings(&self, action: Action) -> &[Binding] {
        self.bound
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, b)| b.as_slice())
            .unwrap_or(&[])
    }

    /// How this action's key is written — the first one, where it has
    /// several — or an empty string when unbound.
    pub fn label(&self, action: Action) -> String {
        self.bindings(action)
            .first()
            .map(|b| b.label())
            .unwrap_or_default()
    }

    /// Every key for `action`, space-separated, or an empty string.
    fn labels(&self, action: Action) -> String {
        self.bindings(action)
            .iter()
            .map(|b| b.label())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// (key, what it does) for every bound action, for the help card.
    pub fn card_rows(&self) -> Vec<(String, &'static str)> {
        ACTIONS
            .iter()
            .filter(|(a, _, _, _)| !self.bindings(*a).is_empty())
            .map(|(a, _, _, what)| (self.labels(*a), *what))
            .collect()
    }

    /// (settings key, current spelling, one-line hint) for the settings note.
    pub fn settings_rows(&self) -> Vec<(&'static str, String, &'static str)> {
        ACTIONS
            .iter()
            .map(|(a, key, _, what)| {
                let spec = match self.labels(*a) {
                    s if s.is_empty() => "none".to_string(),
                    s => s,
                };
                (*key, spec, *what)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn the_caret_spelling_answers_to_ctrl_or_cmd() {
        let b = Binding::parse("^K").unwrap();
        assert!(b.matches(&ev(KeyCode::Char('k'), KeyModifiers::CONTROL)));
        assert!(b.matches(&ev(KeyCode::Char('k'), KeyModifiers::SUPER)));
        // case does not matter: ⇧^K is still ^K
        assert!(b.matches(&ev(KeyCode::Char('K'), KeyModifiers::CONTROL)));
        assert!(!b.matches(&ev(KeyCode::Char('k'), KeyModifiers::NONE)));
        assert!(!b.matches(&ev(KeyCode::Char('j'), KeyModifiers::CONTROL)));
        assert_eq!(b.label(), "^K");
    }

    #[test]
    fn a_spelled_out_modifier_is_exact() {
        let cmd = Binding::parse("cmd+p").unwrap();
        assert!(cmd.matches(&ev(KeyCode::Char('p'), KeyModifiers::SUPER)));
        assert!(!cmd.matches(&ev(KeyCode::Char('p'), KeyModifiers::CONTROL)));
        let alt = Binding::parse("alt+p").unwrap();
        assert!(alt.matches(&ev(KeyCode::Char('p'), KeyModifiers::ALT)));
        assert!(!alt.matches(&ev(KeyCode::Char('p'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn punctuation_and_function_keys_bind() {
        let comma = Binding::parse("^,").unwrap();
        assert!(comma.matches(&ev(KeyCode::Char(','), KeyModifiers::CONTROL)));
        let f = Binding::parse("f5").unwrap();
        assert!(f.matches(&ev(KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!(f.label(), "F5");
    }

    #[test]
    fn a_bare_letter_is_not_a_binding_and_none_unbinds() {
        // it would swallow the letter everywhere you type it
        assert!(Binding::parse("k").is_none());
        assert!(Binding::parse("none").is_none());
        assert!(Binding::parse("").is_none());
        assert!(Binding::parse("nonsense+k").is_none());
    }

    #[test]
    fn settings_override_the_defaults_and_can_unbind() {
        let map = Keymap::from_settings(|k| match k {
            "key_palette" => Some("cmd+p".to_string()),
            "key_quit" => Some("none".to_string()),
            _ => None,
        });
        assert_eq!(
            map.action(&ev(KeyCode::Char('p'), KeyModifiers::SUPER)),
            Some(Action::Palette)
        );
        // the old key is gone, not kept alongside the new one
        assert_eq!(
            map.action(&ev(KeyCode::Char('k'), KeyModifiers::CONTROL)),
            None
        );
        // and an unbound action answers to nothing
        assert_eq!(
            map.action(&ev(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(map.label(Action::Quit), "");
    }

    #[test]
    fn the_defaults_are_the_keys_the_readme_documents() {
        let map = Keymap::default();
        assert_eq!(map.label(Action::Palette), "^K");
        assert_eq!(map.label(Action::QuickOpen), "^O");
        assert_eq!(map.label(Action::NewNote), "^N");
        assert_eq!(map.label(Action::TogglePreview), "^P");
        assert_eq!(map.label(Action::Settings), "^,");
        // delete and rename ship unbound: they are palette commands
        assert_eq!(map.label(Action::DeleteNote), "");
    }

    #[test]
    fn help_answers_to_ctrl_slash_however_the_terminal_spells_it() {
        let map = Keymap::default();
        for c in ['/', '_', '7'] {
            assert_eq!(
                map.action(&ev(KeyCode::Char(c), KeyModifiers::CONTROL)),
                Some(Action::Help),
                "ctrl+{c}"
            );
        }
        assert_eq!(
            map.action(&ev(KeyCode::F(1), KeyModifiers::NONE)),
            Some(Action::Help)
        );
        // the old key is free
        assert_eq!(
            map.action(&ev(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            None
        );
        // plain _ and 7 still type
        assert_eq!(
            map.action(&ev(KeyCode::Char('_'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('7'), KeyModifiers::NONE)),
            None
        );
        // hints show the short one, the card and settings show both
        assert_eq!(map.label(Action::Help), "^/");
        assert_eq!(map.labels(Action::Help), "^/ F1");
        let row = map
            .settings_rows()
            .into_iter()
            .find(|(k, _, _)| *k == "key_help")
            .unwrap();
        assert_eq!(row.1, "^/ F1");
        // and the spelling the settings writer emits reads back to the same keys
        assert_eq!(
            Keymap::from_settings(|k| (k == "key_help").then(|| row.1.clone())),
            map
        );
    }

    #[test]
    fn a_superseded_default_in_the_file_yields_to_the_current_one() {
        let map = Keymap::from_settings(|k| match k {
            "key_help" => Some("^G".to_string()),
            "key_back" => Some("ctrl+⌥←".to_string()),
            _ => None,
        });
        assert_eq!(
            map.action(&ev(KeyCode::F(1), KeyModifiers::NONE)),
            Some(Action::Help)
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Action::NavBack)
        );
        // a key the user chose on purpose still wins
        let map = Keymap::from_settings(|k| (k == "key_help").then(|| "^H".to_string()));
        assert_eq!(
            map.action(&ev(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            Some(Action::Help)
        );
    }

    #[test]
    fn a_spec_can_name_several_keys() {
        let bs = Binding::parse_all("^K, f5 alt+k");
        assert_eq!(bs.len(), 3);
        assert!(Binding::parse_all("none").is_empty());
        // one bad key drops only itself
        assert_eq!(Binding::parse_all("^K junk").len(), 1);
        // a comma can be a key, not just a separator
        assert_eq!(Binding::parse_all("^,").len(), 1);
        assert_eq!(Binding::parse_all("^, f1").len(), 2);
    }

    #[test]
    fn the_old_key_shortcuts_name_still_binds_help() {
        // (^G itself is a superseded default and would be ignored, so ^H)
        let map = Keymap::from_settings(|k| (k == "key_shortcuts").then(|| "^H".to_string()));
        assert_eq!(
            map.action(&ev(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            Some(Action::Help)
        );
        assert_eq!(map.action(&ev(KeyCode::F(1), KeyModifiers::NONE)), None);
        // the new name wins when both are present
        let map = Keymap::from_settings(|k| match k {
            "key_help" => Some("f2".to_string()),
            "key_shortcuts" => Some("^H".to_string()),
            _ => None,
        });
        assert_eq!(
            map.action(&ev(KeyCode::F(2), KeyModifiers::NONE)),
            Some(Action::Help)
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            None
        );
        // and the settings writer emits the new name, never the old
        assert!(map.settings_rows().iter().any(|(k, _, _)| *k == "key_help"));
        assert!(!map
            .settings_rows()
            .iter()
            .any(|(k, _, _)| *k == "key_shortcuts"));
    }

    #[test]
    fn arrow_keys_bind_and_round_trip_through_their_labels() {
        let b = Binding::parse("alt+left").unwrap();
        assert!(b.matches(&ev(KeyCode::Left, KeyModifiers::ALT)));
        assert!(!b.matches(&ev(KeyCode::Left, KeyModifiers::NONE)));
        assert_eq!(b.label(), "⌥←");
        assert_eq!(Binding::parse(&b.label()), Some(b));
        let map = Keymap::default();
        assert_eq!(
            map.action(&ev(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            Some(Action::NavBack)
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            Some(Action::NavForward)
        );
        // every modifier + arrow stays with the editor (or the window manager)
        assert_eq!(map.action(&ev(KeyCode::Left, KeyModifiers::ALT)), None);
        assert_eq!(
            map.action(&ev(
                KeyCode::Left,
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )),
            None
        );
    }

    #[test]
    fn a_shifted_binding_wins_over_the_plain_one_and_needs_its_shift() {
        let map = Keymap::default();
        let shifted = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            map.action(&ev(KeyCode::Char('f'), shifted)),
            Some(Action::SearchAll)
        );
        // the kitty protocol reports the shifted letter as a capital
        assert_eq!(
            map.action(&ev(KeyCode::Char('F'), shifted)),
            Some(Action::SearchAll)
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            Some(Action::NavForward)
        );
        // the label reads back to the same key
        let b = Binding::parse("ctrl+shift+F").unwrap();
        assert_eq!(b.label(), "ctrl+⇧F");
        assert_eq!(Binding::parse(&b.label()), Some(b));
    }

    #[test]
    fn the_daily_note_answers_to_alt_d_and_leaves_plain_d_typing() {
        let map = Keymap::default();
        assert_eq!(
            map.action(&ev(KeyCode::Char('d'), KeyModifiers::ALT)),
            Some(Action::DailyNote)
        );
        assert_eq!(
            map.action(&ev(KeyCode::Char('d'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(map.label(Action::DailyNote), "⌥D");
        assert!(map
            .settings_rows()
            .iter()
            .any(|(k, _, _)| *k == "key_daily"));
    }

    #[test]
    fn every_action_has_a_settings_row() {
        assert_eq!(Keymap::default().settings_rows().len(), ACTIONS.len());
    }
}
