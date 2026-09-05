mod app;
mod cli;
mod clipboard;
mod commands;
mod config;
mod contents;
mod daily;
mod dates;
mod editor;
mod find;
mod fold;
mod history;
mod images;
mod index;
mod keys;
mod links;
mod md;
mod mentions;
mod mermaid;
mod notes;
mod outline;
mod render;
mod search;
mod table;
mod terminal;
#[cfg(test)]
mod testutil;
mod theme;
mod tree;
mod ui;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use std::time::Duration;

/// The kitty keyboard protocol, when the terminal speaks it. Ghostty does, and
/// with it Cmd arrives as `KeyModifiers::SUPER` and Option/Shift survive on the
/// arrow keys — which is what the macOS editing shortcuts are built on. Without
/// it the Home/End/PageUp/PageDown and alt-arrow fallbacks still work.
fn keyboard_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
}

/// Whether the enhanced key encoding is currently pushed. The panic hook has no
/// other way to know, and it changes when the TUI is suspended for $EDITOR.
static ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask for the enhanced keys; returns whether they were pushed (and so must be
/// popped again).
fn push_keyboard() -> bool {
    if !matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) {
        return false;
    }
    let ok = crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(keyboard_flags())
    )
    .is_ok();
    ENHANCED.store(ok, std::sync::atomic::Ordering::Relaxed);
    ok
}

fn pop_keyboard() {
    if ENHANCED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("catcher: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match cli::parse(&args, probe) {
        cli::Cli::Tui(l) => l,
        cli::Cli::Help => {
            print!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Cli::Version => {
            println!("catcher {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Cli::PrintPath => {
            println!("{}", config::Config::load()?.notes_dir.display());
            return Ok(());
        }
        cli::Cli::Add(text) => return add(text),
        cli::Cli::Keys => return keys(),
        cli::Cli::Error(msg) => {
            eprintln!("catcher: {msg}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    tui(launch)
}

/// What a bare argument is on disk, for the parser.
fn probe(s: &str) -> cli::PathKind {
    let path = std::path::Path::new(s);
    if path.is_dir() {
        cli::PathKind::Dir
    } else if path.is_file() {
        cli::PathKind::File
    } else {
        cli::PathKind::Missing
    }
}

/// `catcher add` — capture without ever starting the TUI. The first line
/// becomes the title and so the filename; the path is printed for scripts.
fn add(text: Option<String>) -> Result<()> {
    let text = match text {
        Some(t) => t,
        None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf
        }
    };
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("nothing to add");
    }
    let config = config::Config::load()?;
    config.ensure_dirs()?;
    let note = notes::create_with(&config.notes_dir, format!("{text}\n"))?;
    println!("{}", note.path.display());
    Ok(())
}

/// `catcher --keys` — ground truth about a terminal.
///
/// Raw mode, the keyboard enhancement pushed exactly as the TUI pushes it, and
/// then every key event printed as it arrives, until Esc. What a terminal
/// actually sends for ⌘← is not something to guess at: many of them rewrite the
/// Mac editing keys into legacy control bytes with their own keybinds, which the
/// kitty protocol never gets a chance to encode.
fn keys() -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let supported = crossterm::terminal::supports_keyboard_enhancement();
    let pushed = push_keyboard();
    println!("keyboard enhancement: supported={supported:?} pushed={pushed}\r");
    println!("press keys — esc to quit\r");
    loop {
        match event::read()? {
            Event::Key(k) => {
                println!(
                    "code={:?}  modifiers={:?}  kind={:?}  state={:?}\r",
                    k.code, k.modifiers, k.kind, k.state
                );
                if k.code == event::KeyCode::Esc && k.kind != event::KeyEventKind::Release {
                    break;
                }
            }
            other => println!("{other:?}\r"),
        }
    }
    pop_keyboard();
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

/// Save the terminal's own title (xterm title stack; Ghostty, iTerm2, kitty
/// and WezTerm honour it) so the shell's comes back on exit.
fn push_title() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[22;0t");
    let _ = out.flush();
}

/// Put the shell's title back; on a terminal without the stack, a blank
/// title is the harmless fallback.
fn pop_title() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[23;0t");
    let _ = crossterm::execute!(out, crossterm::terminal::SetTitle(""));
}

/// Ask the terminal which way its background runs, for `theme: auto`.
///
/// OSC 11 (`]11;?`) answers with the background colour on every terminal
/// that matters — Ghostty, iTerm2, kitty, WezTerm, Terminal.app, foot,
/// Alacritty. The reply is read back through crossterm's event queue (the
/// same one the TUI uses, so nothing else is left holding stdin): crossterm
/// has no idea what an OSC reply is and hands it over as a run of key
/// events, which are turned back into bytes here. When no reply arrives in
/// time, `COLORFGBG` (rxvt, Konsole and a few others set it) is the
/// fallback, and dark is the fallback for that.
fn detect_background() -> theme::Mode {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || std::env::var_os("TMUX").is_some() {
        return from_colorfgbg().unwrap_or_default();
    }
    if crossterm::terminal::enable_raw_mode().is_err() {
        return from_colorfgbg().unwrap_or_default();
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b]11;?\x1b\\");
    let _ = out.flush();

    // read through crossterm's event queue with a deadline, so nothing keeps
    // reading stdin after we leave and a silent terminal costs only the wait;
    // the DSR makes that the rare case
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    let mut reply = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() || !crossterm::event::poll(left).unwrap_or(false) {
            break;
        }
        match crossterm::event::read() {
            Ok(Event::Key(k)) => push_key_bytes(&mut reply, &k),
            Ok(_) => {}
            Err(_) => break,
        }
        let done = reply.windows(4).any(|w| w == b"]11;")
            && (reply.ends_with(b"\x1b\\") || reply.ends_with(b"\x07"));
        if done || reply.len() > 256 {
            break;
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
    parse_osc11(&reply)
        .or_else(from_colorfgbg)
        .unwrap_or_default()
}

/// The bytes a key event stands for, undoing crossterm's reading of an OSC
/// reply: `ESC x` comes back as Alt+x, a control byte as Ctrl+letter, and
/// the rest as plain characters.
fn push_key_bytes(buf: &mut Vec<u8>, k: &event::KeyEvent) {
    use event::{KeyCode, KeyModifiers};
    if k.modifiers.contains(KeyModifiers::ALT) {
        buf.push(0x1b);
    }
    match k.code {
        KeyCode::Char(c)
            if k.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_lowercase() =>
        {
            buf.push(c as u8 - b'a' + 1);
        }
        KeyCode::Char(c) => {
            let mut tmp = [0u8; 4];
            buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
        }
        KeyCode::Esc => buf.push(0x1b),
        _ => {}
    }
}

/// The polarity in an OSC 11 reply: `]11;rgb:RRRR/GGGG/BBBB` with 1–4 hex
/// digits per channel, terminated by ST or BEL.
fn parse_osc11(reply: &[u8]) -> Option<theme::Mode> {
    let text = String::from_utf8_lossy(reply);
    let rest = text.split("]11;").nth(1)?;
    let rest = rest.strip_prefix("rgb:")?;
    let mut chan = rest.split(['\x1b', '\x07']).next()?.split('/').map(|h| {
        // scale any width to 8 bits from its leading digits
        let h = h.trim();
        let n = u32::from_str_radix(h, 16).ok()?;
        let bits = 4 * h.len() as u32;
        match bits {
            4 => Some((n * 17) as u8),
            8 | 12 | 16 => Some((n >> (bits - 8)) as u8),
            _ => None,
        }
    });
    let (r, g, b) = (chan.next()??, chan.next()??, chan.next()??);
    Some(theme::mode_of_background(r, g, b))
}

/// `COLORFGBG=fg;bg`, with the background as an ANSI index: 0–6 and 8 are
/// dark, 7 and 9–15 are light.
fn from_colorfgbg() -> Option<theme::Mode> {
    use theme::Mode;
    let v = std::env::var("COLORFGBG").ok()?;
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(match bg {
        7 | 9..=15 => Mode::Light,
        _ => Mode::Dark,
    })
}

fn tui(launch: cli::Launch) -> Result<()> {
    // before the settings load: `theme: auto` resolves against this
    let detected = detect_background();
    theme::set_detected(detected);
    // a terminal whose background matches the system appearance is taken to
    // follow it, and the palette then follows too — see `follow_system_theme`
    theme::set_follows_system(theme::system_mode() == Some(detected));
    // shells out to `date` once, so it runs before raw mode
    dates::init();
    // a name that matches nothing fails here, before the terminal is touched
    let mut app = app::App::launch(launch)?;
    push_title();
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    push_keyboard();
    // ratatui's panic hook puts back raw mode and the alternate screen, but
    // knows nothing about mouse reporting or the keyboard protocol; without
    // this a panic leaves the shell spitting escape sequences at every click
    // and the terminal's key encoding pushed.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        pop_keyboard();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        pop_title();
        hook(info);
    }));

    // ask the terminal about graphics support once, in raw mode
    app.images.probe();

    let result = run(&mut terminal, &mut app);

    pop_keyboard();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    pop_title();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    let result = run_loop(terminal, app);
    // saved whether the loop ended by quitting or by an error, so a terminal
    // hiccup never costs the note being edited
    app.save_now();
    result
}

fn run_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
    // the first frame is unconditional; after that the screen is painted
    // only when an event came in (a resize included) or the tick changed
    // something, rather than ten times a second while idle
    let mut changed = true;
    while !app.quit {
        if changed {
            // a frame goes to the terminal as one piece: ratatui clears the
            // screen on a resize and paints it again, and without this the
            // terminal shows the blank in between, which is the flicker a
            // window drag had
            let _ = crossterm::execute!(std::io::stdout(), BeginSynchronizedUpdate);
            let drawn = terminal.draw(|f| ui::draw(f, app));
            let _ = crossterm::execute!(std::io::stdout(), EndSynchronizedUpdate);
            drawn?;
        }
        let mut handled = false;
        if event::poll(Duration::from_millis(100))? {
            handle(app, event::read()?);
            handled = true;
            // a wheel flick or a window drag arrives as a burst; the whole
            // burst is taken before the next frame, so the screen is painted
            // once for where things ended up rather than once per step
            while event::poll(Duration::ZERO)? {
                handle(app, event::read()?);
            }
        }
        let ticked = app.tick();
        changed = handled || ticked;
    }
    Ok(())
}

fn handle(app: &mut app::App, ev: Event) {
    match ev {
        Event::Key(k) if k.kind != event::KeyEventKind::Release => app.on_key(k),
        Event::Mouse(m) => app.on_mouse(m),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::Mode;

    #[test]
    fn osc11_reply_tells_light_from_dark() {
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:eeee/eeee/eeee\x1b\\\x1b[0n"),
            Some(Mode::Light)
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:1414/1414/1414\x07\x1b[0n"),
            Some(Mode::Dark)
        );
        // 8-bit channels too
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:ff/ff/ff\x1b\\"),
            Some(Mode::Light)
        );
        // no colour reply, only the status report
        assert_eq!(parse_osc11(b"\x1b[0n"), None);
        assert_eq!(parse_osc11(b""), None);
    }

    #[test]
    fn key_events_turn_back_into_the_osc_reply_bytes() {
        use event::{KeyCode, KeyEvent, KeyModifiers};
        let mut buf = Vec::new();
        push_key_bytes(
            &mut buf,
            &KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT),
        );
        for c in "11;rgb:ff/ff/ff".chars() {
            push_key_bytes(
                &mut buf,
                &KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        push_key_bytes(
            &mut buf,
            &KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT),
        );
        assert_eq!(buf, b"\x1b]11;rgb:ff/ff/ff\x1b\\");
        assert_eq!(parse_osc11(&buf), Some(Mode::Light));

        let mut bel = Vec::new();
        push_key_bytes(
            &mut bel,
            &KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );
        push_key_bytes(&mut bel, &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(bel, b"\x07\x1b");
    }
}
