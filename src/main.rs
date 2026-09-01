mod app;
mod cli;
mod clipboard;
mod config;
mod editor;
mod history;
mod images;
mod index;
mod keys;
mod md;
mod mentions;
mod notes;
mod render;
mod search;
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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match cli::parse(&args, probe) {
        cli::Cli::Tui(l) => l,
        cli::Cli::Help => {
            print!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Cli::PrintPath => {
            println!("{}", config::Config::load()?.notes_dir.display());
            return Ok(());
        }
        cli::Cli::Add(text) => return add(text),
        cli::Cli::Keys => return keys(),
        cli::Cli::Error(msg) => {
            eprintln!("tinynote: {msg}\n\n{}", cli::USAGE);
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

/// `tinynote add` — capture without ever starting the TUI. The first line
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

/// `tinynote --keys` — ground truth about a terminal.
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

fn tui(launch: cli::Launch) -> Result<()> {
    let mut app = app::App::launch(launch)?;
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
        hook(info);
    }));

    // ask the terminal about graphics support once, in raw mode
    app.images.probe();

    let result = run(&mut terminal, &mut app);

    pop_keyboard();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) if k.kind != event::KeyEventKind::Release => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                _ => {}
            }
        }
        app.tick();
    }
    app.save_now();
    Ok(())
}
