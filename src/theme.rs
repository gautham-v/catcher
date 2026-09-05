//! The one place colours live, so preview and live preview agree.
//!
//! A neutral grey chassis with a single accent. Hue is never decoration: it
//! appears in exactly three places — the top-level heading, a checked task,
//! and the status bar when catcher is talking about itself. Everything else
//! is a step on the grey ramp, which is why the ramp never reaches pure black
//! or pure white at either end: text that hits #ffffff on someone's custom
//! background looks like a bug, not emphasis.
//!
//! The two palettes are the same structure at both polarities, not an
//! inversion — the code background goes *darker* than the page in light mode,
//! because "raised" means more contrast with the ground, not lighter.

use ratatui::style::{Color, Modifier, Style};
use std::sync::RwLock;

/// Which polarity the terminal is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

/// Every colour catcher can draw with, at one polarity. Each field is
/// settable by name from the settings file, so a user who wants their own
/// hue for links or headings sets that one field and inherits the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The one hue: h1, a checked mark, status-bar state.
    pub accent: Color,
    /// The brightest step — keys, group headings, anything that must lead.
    pub bright: Color,
    /// Structure that should recede: struck tasks, the status-bar path.
    pub grey: Color,
    /// `##` headings. The complement of the accent, so the two top
    /// levels can never be mistaken for one another.
    pub heading: Color,
    /// Markers, rules, quotes: present but never read first.
    pub dim: Color,
    /// Links, which lean on the underline rather than the colour.
    pub link: Color,
    /// Inline code: a tint of the accent and no box, so a file name in a
    /// sentence reads as a name rather than a patch on the page.
    pub code: Color,
    /// Behind a fenced code block.
    pub code_bg: Color,
    /// The text on that background. Code is the one construct that paints
    /// its own ground, so it is also the one that cannot leave its
    /// foreground to the terminal: prose inherits whatever colour the
    /// terminal is already using and looks right either way, but a dark
    /// chip under a light terminal's ink is black on black. Both halves
    /// are set together or neither is legible.
    pub code_fg: Color,
    /// Panel borders at rest.
    pub border: Color,
    /// Destructive confirmation, and a `[[link]]` that names no note —
    /// the two things the eye must not slide past. Recolour it and both
    /// move together.
    pub danger: Color,
    /// Good news: a tip or success callout.
    pub success: Color,
    /// The ground a highlight or an inverted heading sits its text on.
    pub ground: Color,
}

/// One settable colour: its settings-file name, the field it sets, and the
/// one-line hint the settings document prints beside it.
pub struct ColorKey {
    pub name: &'static str,
    field: fn(&mut Palette) -> &mut Color,
    pub hint: &'static str,
}

/// Every colour the settings file accepts, in the order the settings document
/// lists them. The single source of truth: a name that isn't here can't be
/// set and isn't documented.
pub const COLORS: [ColorKey; 13] = [
    color(
        "accent",
        |p| &mut p.accent,
        "h1, ticked boxes, the status bar",
    ),
    color("bright", |p| &mut p.bright, "h3, and the step that leads"),
    color("grey", |p| &mut p.grey, "structure that recedes"),
    color("heading", |p| &mut p.heading, "h2"),
    color("dim", |p| &mut p.dim, "markers, rules, quotes"),
    color("link", |p| &mut p.link, "links, which also underline"),
    color("code", |p| &mut p.code, "inline code, no box"),
    color("code_bg", |p| &mut p.code_bg, "behind a code block"),
    color("code_fg", |p| &mut p.code_fg, "and the code on it"),
    color("border", |p| &mut p.border, "panel borders"),
    color(
        "danger",
        |p| &mut p.danger,
        "the delete prompt, and a broken [[link]]",
    ),
    color("success", |p| &mut p.success, "tip and success callouts"),
    color("ground", |p| &mut p.ground, "under a highlight"),
];

const fn color(
    name: &'static str,
    field: fn(&mut Palette) -> &mut Color,
    hint: &'static str,
) -> ColorKey {
    ColorKey { name, field, hint }
}

/// Just the names, derived from `COLORS`.
pub const COLOR_KEYS: [&str; 13] = {
    let mut keys = [""; 13];
    let mut i = 0;
    while i < keys.len() {
        keys[i] = COLORS[i].name;
        i += 1;
    }
    keys
};

impl Palette {
    /// Set one field by its settings-file name. Returns false for a name
    /// that isn't a colour, so the caller can report the typo.
    pub fn set(&mut self, key: &str, color: Color) -> bool {
        match COLORS.iter().find(|c| c.name == key) {
            Some(c) => {
                *(c.field)(self) = color;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &str) -> Option<Color> {
        let mut copy = *self;
        COLORS
            .iter()
            .find(|c| c.name == key)
            .map(|c| *(c.field)(&mut copy))
    }
}

pub const DARK: Palette = Palette {
    accent: Color::Rgb(0xff, 0x9e, 0x64),
    bright: Color::Rgb(0xe1, 0xe1, 0xe1),
    grey: Color::Rgb(0x78, 0x78, 0x78),
    heading: Color::Rgb(0x8f, 0xb4, 0xd9),
    dim: Color::Rgb(0x82, 0x82, 0x82),
    link: Color::Rgb(0xb4, 0xb4, 0xb4),
    code: Color::Rgb(0xd9, 0xa2, 0x7a),
    code_bg: Color::Rgb(0x1c, 0x1c, 0x1c),
    code_fg: Color::Rgb(0xe1, 0xe1, 0xe1),
    border: Color::Rgb(0x32, 0x32, 0x37),
    danger: Color::Rgb(0xf7, 0x76, 0x8e),
    success: Color::Rgb(0x7f, 0xc8, 0x8f),
    ground: Color::Rgb(0x14, 0x14, 0x14),
};

pub const LIGHT: Palette = Palette {
    accent: Color::Rgb(0xb8, 0x5c, 0x18),
    bright: Color::Rgb(0x26, 0x26, 0x26),
    grey: Color::Rgb(0x55, 0x55, 0x55),
    heading: Color::Rgb(0x3d, 0x6a, 0x99),
    dim: Color::Rgb(0x8d, 0x8d, 0x8d),
    link: Color::Rgb(0x5a, 0x58, 0x52),
    code: Color::Rgb(0x8a, 0x4a, 0x14),
    code_bg: Color::Rgb(0xe2, 0xe2, 0xe2),
    code_fg: Color::Rgb(0x26, 0x26, 0x26),
    border: Color::Rgb(0xc8, 0xc8, 0xcd),
    danger: Color::Rgb(0xcd, 0x30, 0x48),
    success: Color::Rgb(0x2e, 0x7d, 0x4f),
    ground: Color::Rgb(0xee, 0xee, 0xee),
};

/// The palette in force. A lock rather than a `OnceLock`: settings are
/// edited inside the app now, and a saved change has to be visible on the
/// very next frame without a restart.
static PALETTE: RwLock<Palette> = RwLock::new(DARK);
static BOLD_HEADINGS: RwLock<bool> = RwLock::new(true);
/// The polarity the terminal was found to be showing at startup, for a
/// `theme: auto` setting. Dark until a probe says otherwise: the far more
/// common terminal, and the palette the app always used to assume.
static DETECTED: RwLock<Mode> = RwLock::new(Mode::Dark);

/// Record what the terminal reported about its own background.
pub fn set_detected(mode: Mode) {
    if let Ok(mut w) = DETECTED.write() {
        *w = mode;
    }
}

/// Whether the terminal's background agreed with the system appearance at
/// startup — the sign that it follows the system, and will flip with it.
static FOLLOWS_SYSTEM: RwLock<bool> = RwLock::new(false);

pub fn set_follows_system(on: bool) {
    if let Ok(mut w) = FOLLOWS_SYSTEM.write() {
        *w = on;
    }
}

pub fn follows_system() -> bool {
    FOLLOWS_SYSTEM.read().map(|b| *b).unwrap_or(false)
}

/// The operating system's own appearance, where it has one to ask about.
/// macOS only for now: `AppleInterfaceStyle` is `Dark` or unset.
pub fn system_mode() -> Option<Mode> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;
    Some(if String::from_utf8_lossy(&out.stdout).trim() == "Dark" {
        Mode::Dark
    } else {
        Mode::Light
    })
}

/// The polarity a `theme: auto` setting resolves to.
pub fn detected() -> Mode {
    DETECTED.read().map(|m| *m).unwrap_or(Mode::Dark)
}

/// Which way a background colour runs, by relative luminance. The
/// components are 8-bit; anything brighter than mid-grey is light.
pub fn mode_of_background(r: u8, g: u8, b: u8) -> Mode {
    let lum = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    if lum > 127.5 {
        Mode::Light
    } else {
        Mode::Dark
    }
}

/// The built-in palette for a polarity, before any user overrides.
pub fn base(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => DARK,
        Mode::Light => LIGHT,
    }
}

/// Install the palette for the run. Called at startup and again every time
/// the settings are saved.
pub fn set_palette(p: Palette) {
    if let Ok(mut w) = PALETTE.write() {
        *w = p;
    }
}

pub fn set_bold_headings(on: bool) {
    if let Ok(mut w) = BOLD_HEADINGS.write() {
        *w = on;
    }
}

pub fn palette() -> Palette {
    PALETTE.read().map(|p| *p).unwrap_or(DARK)
}

fn bold() -> Modifier {
    let on = BOLD_HEADINGS.read().map(|b| *b).unwrap_or(true);
    if on {
        Modifier::BOLD
    } else {
        Modifier::empty()
    }
}

/// Body text is never coloured: it inherits whatever foreground the
/// terminal is already using, so a custom Ghostty theme keeps its own
/// idea of what plain prose looks like.
pub const PLAIN: Style = Style::new();

/// Every heading level a terminal can tell apart without a change of
/// size: the accent leads, `##` takes its complement, `###` the bright
/// step, and anything deeper is weight alone.
pub fn heading(level: usize) -> Style {
    match level {
        1 => Style::new().fg(palette().accent).add_modifier(bold()),
        2 => Style::new().fg(palette().heading).add_modifier(bold()),
        3 => Style::new().fg(palette().bright).add_modifier(bold()),
        _ => Style::new().add_modifier(bold()),
    }
}

/// Quoted text reads in the normal foreground: the rail is the signal,
/// and bold or code inside the quote should look like bold or code.
pub fn quote() -> Style {
    Style::new()
}
pub fn marker() -> Style {
    Style::new().fg(palette().dim)
}
/// Secondary text that still has to be read: the status-bar path, struck
/// tasks. One step darker than `marker`, which is for chrome.
pub fn grey() -> Style {
    Style::new().fg(palette().grey)
}
/// Inline code: tinted ink, no background. The box that a fence gets
/// nearly vanished at terminal contrast inside a sentence; a hue does not.
pub fn inline_code() -> Style {
    Style::new().fg(palette().code)
}
/// A fenced block carries no hue of its own — the raised background is
/// the signal. It states its foreground anyway: see `code_fg`.
pub fn code() -> Style {
    Style::new().fg(palette().code_fg).bg(palette().code_bg)
}
/// The colour a callout of `kind` is drawn in, following Obsidian's
/// families: notes blue, tips green, warnings orange, dangers red.
pub fn callout(kind: &str) -> Style {
    let p = palette();
    let color = match kind {
        "tip" | "hint" | "important" | "success" | "check" | "done" => p.success,
        "warning" | "caution" | "attention" | "question" | "help" | "faq" => p.accent,
        "danger" | "error" | "bug" | "failure" | "fail" | "missing" => p.danger,
        "example" => p.code,
        "quote" | "cite" => p.grey,
        "summary" | "abstract" | "tldr" => p.link,
        _ => p.heading,
    };
    Style::new().fg(color)
}

/// Maths, inline or displayed: italic, as a typeset formula would be.
pub fn math() -> Style {
    Style::new().add_modifier(Modifier::ITALIC)
}
pub fn link() -> Style {
    Style::new()
        .fg(palette().link)
        .add_modifier(Modifier::UNDERLINED)
}
pub fn highlight() -> Style {
    Style::new().fg(palette().ground).bg(palette().accent)
}
pub fn done() -> Style {
    Style::new().fg(palette().accent)
}
/// A forwarded task's `➔`: the heading colour, so it stands apart from
/// both the accent of a done box and the dim of an open one.
pub fn forwarded() -> Style {
    Style::new().fg(palette().heading)
}
/// The text of a finished task: struck through, in `grey` rather than
/// `dim`. It is still content you sometimes need to read, so it sits one
/// step above hints and markers.
pub fn done_text() -> Style {
    Style::new()
        .fg(palette().grey)
        .add_modifier(Modifier::CROSSED_OUT)
}
/// An inline `#tag`: the accent, like a heading, because a tag is a
/// heading of sorts — it names what the note is about.
pub fn tag() -> Style {
    Style::new().fg(palette().accent)
}
/// Status-bar state, panel titles: catcher talking about itself.
pub fn state() -> Style {
    Style::new().fg(palette().accent)
}
pub fn border() -> Style {
    Style::new().fg(palette().border)
}
pub fn danger() -> Style {
    Style::new().fg(palette().danger)
}
pub fn bright() -> Style {
    Style::new().fg(palette().bright)
}
/// The marker on a folded heading: the accent, so a closed section reads
/// as the one thing on the page that is asking to be opened.
pub fn fold() -> Style {
    Style::new().fg(palette().accent)
}

/// The ground a selected palette row sits on. Monochrome on purpose: the
/// palette is chrome over the note, and a hue here would compete with the
/// one the note itself spends on headings. `border` is the step that is
/// visible against the page at both polarities without shouting.
pub fn row() -> Style {
    Style::new().bg(palette().border)
}

/// `#rrggbb`, `#rgb`, or one of the sixteen ANSI names, as written in the
/// settings file. `None` for anything else, which the settings reader
/// reports rather than silently ignoring.
pub fn parse_color(text: &str) -> Option<Color> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix('#') {
        let digits: Vec<u32> = hex
            .chars()
            .map(|c| c.to_digit(16))
            .collect::<Option<Vec<u32>>>()?;
        return match digits.len() {
            // #rgb is the shorthand every CSS-trained hand tries first
            3 => Some(Color::Rgb(
                (digits[0] * 17) as u8,
                (digits[1] * 17) as u8,
                (digits[2] * 17) as u8,
            )),
            6 => Some(Color::Rgb(
                (digits[0] * 16 + digits[1]) as u8,
                (digits[2] * 16 + digits[3]) as u8,
                (digits[4] * 16 + digits[5]) as u8,
            )),
            _ => None,
        };
    }
    Some(match t.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" | "darkgray" | "darkgrey" => Color::DarkGray,
        "brightred" => Color::LightRed,
        "brightgreen" => Color::LightGreen,
        "brightyellow" => Color::LightYellow,
        "brightblue" => Color::LightBlue,
        "brightmagenta" => Color::LightMagenta,
        "brightcyan" => Color::LightCyan,
        "brightwhite" => Color::Gray,
        "default" | "terminal" => Color::Reset,
        _ => return None,
    })
}

/// A colour written back the way the settings file spells it.
pub fn color_to_string(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => "default".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

pub const CHECKED: &str = "\u{2713}";
pub const UNCHECKED: &str = "\u{2610}";
/// The other states a task box can hold, in the Obsidian Tasks / Minimal
/// convention: `[/]` in progress, `[-]` cancelled, `[>]` forwarded, `[?]`
/// a question.
pub const IN_PROGRESS: &str = "\u{25d0}";
pub const CANCELLED: &str = "\u{2298}";
pub const FORWARDED: &str = "\u{2794}";
pub const QUESTION: &str = "?";
/// Every glyph a task box is drawn as, so a click can tell a box from text.
pub const TASK_GLYPHS: [&str; 6] = [
    CHECKED,
    UNCHECKED,
    IN_PROGRESS,
    CANCELLED,
    FORWARDED,
    QUESTION,
];
pub const BULLET: &str = "\u{2022}";
/// Second- and third-level bullets; `bullet(depth)` cycles through the three.
pub const BULLET_2: &str = "\u{25e6}";
pub const BULLET_3: &str = "\u{25aa}";

/// The bullet glyph for a list nested `depth` levels deep (1 is top level):
/// `•`, `◦`, `▪`, then round again. Depth 0 is treated as 1.
pub fn bullet(depth: usize) -> &'static str {
    match depth.max(1) % 3 {
        1 => BULLET,
        2 => BULLET_2,
        _ => BULLET_3,
    }
}
/// In front of a folded heading.
pub const FOLDED: &str = "\u{25b8} ";
/// In front of an open callout that can fold.
pub const UNFOLDED: &str = "\u{25be} ";
pub const QUOTE_BAR: &str = "\u{258c}";
/// A hard line break, in place of its trailing space or backslash.
pub const HARD_BREAK: &str = "\u{21b5}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_glyph_cycles_every_three_levels() {
        assert_eq!(bullet(1), BULLET);
        assert_eq!(bullet(2), BULLET_2);
        assert_eq!(bullet(3), BULLET_3);
        assert_eq!(bullet(4), BULLET);
        assert_eq!(bullet(6), BULLET_3);
        // depth 0 never happens, but reads as top level if it does
        assert_eq!(bullet(0), BULLET);
    }

    #[test]
    fn each_of_the_first_three_heading_levels_takes_its_own_colour() {
        for p in [DARK, LIGHT] {
            set_palette(p);
            let fg: Vec<_> = (1..=4).map(|l| heading(l).fg).collect();
            assert_eq!(fg[..3], [Some(p.accent), Some(p.heading), Some(p.bright)]);
            assert_eq!(fg[3], None);
            assert_ne!(p.accent, p.heading);
            assert_ne!(p.heading, p.bright);
        }
        set_palette(DARK);
    }

    #[test]
    fn code_states_both_halves_so_a_light_terminal_is_not_black_on_black() {
        // prose leaves its foreground to the terminal on purpose; code paints
        // its own ground and so cannot
        let c = code();
        assert!(c.bg.is_some());
        assert!(
            c.fg.is_some(),
            "code without a foreground is unreadable on a terminal whose ink matches code_bg"
        );
    }
}
