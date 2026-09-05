//! Asking the terminal for a new split or tab.
//!
//! Catcher has no panes of its own: two notes side by side is the terminal's
//! job, and every terminal worth the name can be told to open one. What
//! differs is how it is told. tmux and WezTerm have a CLI, kitty a remote
//! control socket, and Ghostty on macOS an AppleScript dictionary. Each is
//! one command line away; this module picks the right one from the
//! environment and runs it. The CLIs return as soon as the terminal has the
//! request, so they are waited for inline; osascript is waited for on a
//! detached thread so no child is ever left unreaped.

use std::process::{Command, Stdio};

/// Where the new surface goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    SplitRight,
    SplitDown,
    Tab,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    Tmux,
    Ghostty,
    Kitty,
    WezTerm,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Tmux => "tmux",
            Backend::Ghostty => "ghostty",
            Backend::Kitty => "kitty",
            Backend::WezTerm => "wezterm",
        }
    }
}

/// Which terminal is listening, from `env`. tmux comes first: inside it the
/// outer terminal's variables are still set, but a split it does not know
/// about would land beside the whole session rather than beside this pane.
fn detect(env: impl Fn(&str) -> Option<String>) -> Option<Backend> {
    if env("TMUX").is_some() {
        return Some(Backend::Tmux);
    }
    let program = env("TERM_PROGRAM").unwrap_or_default();
    if cfg!(target_os = "macos") && program.eq_ignore_ascii_case("ghostty") {
        return Some(Backend::Ghostty);
    }
    if env("KITTY_WINDOW_ID").is_some() || env("TERM").as_deref() == Some("xterm-kitty") {
        return Some(Backend::Kitty);
    }
    if env("WEZTERM_PANE").is_some() || program.eq_ignore_ascii_case("wezterm") {
        return Some(Backend::WezTerm);
    }
    None
}

/// The terminal a split would go to, by name — `None` where there is no way
/// to ask for one.
pub fn backend() -> Option<&'static str> {
    detect(|k| std::env::var(k).ok()).map(Backend::name)
}

/// Ask the terminal this process runs in to open a new surface at `place`
/// running `argv` (argv[0] is the program, absolute path). The new surface
/// takes focus. Err carries a one-line message for the status bar.
pub fn open_beside(place: Place, argv: &[String]) -> Result<(), String> {
    let Some(backend) = detect(|k| std::env::var(k).ok()) else {
        return Err("no splits here — Ghostty, tmux, kitty or WezTerm".to_string());
    };
    let mut cmd = command_for(backend, place, argv);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let spawned = if backend == Backend::Ghostty {
        // osascript can block on the app for a while; reap it off-thread
        cmd.spawn().map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
    } else {
        cmd.status().and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!("exit {s}")))
            }
        })
    };
    spawned.map_err(|e| format!("{} failed: {e}", backend.name()))
}

/// The command line that asks `backend` for the surface.
fn command_for(backend: Backend, place: Place, argv: &[String]) -> Command {
    match backend {
        Backend::Tmux => {
            let mut c = Command::new("tmux");
            match place {
                Place::SplitRight => c.args(["split-window", "-h"]),
                Place::SplitDown => c.args(["split-window", "-v"]),
                Place::Tab => c.arg("new-window"),
            };
            c.args(argv);
            c
        }
        Backend::Ghostty => {
            let mut c = Command::new("osascript");
            c.arg("-e").arg(ghostty_script(place, argv));
            c
        }
        Backend::Kitty => {
            let location = match place {
                Place::SplitRight => "vsplit",
                Place::SplitDown => "hsplit",
                Place::Tab => "tab",
            };
            let mut c = Command::new("kitten");
            c.args(["@", "launch", "--type=window"])
                .arg(format!("--location={location}"))
                .arg("--cwd=current")
                .args(argv);
            c
        }
        Backend::WezTerm => {
            let mut c = Command::new("wezterm");
            c.arg("cli");
            match place {
                Place::SplitRight => c.args(["split-pane", "--right"]),
                Place::SplitDown => c.args(["split-pane", "--bottom"]),
                Place::Tab => c.arg("spawn"),
            };
            c.arg("--").args(argv);
            c
        }
    }
}

/// The AppleScript that Ghostty 1.3 answers to. Its `command` is read by a
/// shell, so the words are shell-quoted first, and the whole thing is then a
/// double-quoted AppleScript literal.
fn ghostty_script(place: Place, argv: &[String]) -> String {
    let command = applescript_string(&shell_join(argv));
    let open = match place {
        Place::SplitRight => "split t direction right with configuration cfg",
        Place::SplitDown => "split t direction down with configuration cfg",
        Place::Tab => "new tab in front window with configuration cfg",
    };
    format!(
        "tell application \"Ghostty\"\n\
         set t to focused terminal of selected tab of front window\n\
         set cfg to new surface configuration\n\
         set command of cfg to {command}\n\
         {open}\n\
         end tell"
    )
}

/// One word for a POSIX shell: single-quoted, with a quote inside written
/// as `'\''`. Bare only when nothing in it means anything to a shell.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,~".contains(c));
    if plain {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A double-quoted AppleScript string literal: backslash and the quote are
/// the only characters it escapes.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn each_terminal_is_recognised() {
        let ghostty = if cfg!(target_os = "macos") {
            Some(Backend::Ghostty)
        } else {
            None
        };
        assert_eq!(detect(env(&[("TERM_PROGRAM", "ghostty")])), ghostty);
        assert_eq!(
            detect(env(&[("KITTY_WINDOW_ID", "1")])),
            Some(Backend::Kitty)
        );
        assert_eq!(
            detect(env(&[("TERM", "xterm-kitty")])),
            Some(Backend::Kitty)
        );
        assert_eq!(
            detect(env(&[("WEZTERM_PANE", "0")])),
            Some(Backend::WezTerm)
        );
        assert_eq!(
            detect(env(&[("TERM_PROGRAM", "WezTerm")])),
            Some(Backend::WezTerm)
        );
        assert_eq!(detect(env(&[("TERM_PROGRAM", "Apple_Terminal")])), None);
        assert_eq!(detect(env(&[])), None);
    }

    #[test]
    fn tmux_wins_over_the_terminal_it_runs_in() {
        assert_eq!(
            detect(env(&[
                ("TMUX", "/tmp/tmux-501/default,1,0"),
                ("TERM_PROGRAM", "ghostty")
            ])),
            Some(Backend::Tmux)
        );
    }

    #[test]
    fn shell_quoting_leaves_plain_words_alone_and_wraps_the_rest() {
        assert_eq!(
            shell_quote("/usr/local/bin/catcher"),
            "/usr/local/bin/catcher"
        );
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn applescript_escapes_quotes_and_backslashes() {
        assert_eq!(
            applescript_string(r#"say "hi" \ bye"#),
            r#""say \"hi\" \\ bye""#
        );
    }

    #[test]
    fn the_ghostty_script_quotes_a_path_with_a_space_and_an_apostrophe() {
        let s = ghostty_script(
            Place::SplitRight,
            &argv(&[
                "/opt/catcher",
                "--root",
                "/Users/g/my notes",
                "/Users/g/my notes/it's.md",
            ]),
        );
        // the shell sees `it'\''s.md`; the backslash is doubled once more
        // for the AppleScript literal it travels in
        assert!(s.contains(
            r#"set command of cfg to "/opt/catcher --root '/Users/g/my notes' '/Users/g/my notes/it'\\''s.md'""#
        ), "{s}");
        assert!(s.contains("split t direction right with configuration cfg"));
        assert!(s.starts_with("tell application \"Ghostty\"\n"));
        assert!(s.ends_with("\nend tell"));
    }

    #[test]
    fn the_ghostty_script_has_a_down_and_a_tab_form() {
        let down = ghostty_script(Place::SplitDown, &argv(&["catcher"]));
        assert!(down.contains("split t direction down with configuration cfg"));
        let tab = ghostty_script(Place::Tab, &argv(&["catcher"]));
        assert!(tab.contains("new tab in front window with configuration cfg"));
        assert!(!tab.contains("split t"));
    }

    #[test]
    fn the_other_backends_get_argv_as_separate_words() {
        let words = |c: &Command| -> Vec<String> {
            std::iter::once(c.get_program().to_string_lossy().into_owned())
                .chain(c.get_args().map(|a| a.to_string_lossy().into_owned()))
                .collect()
        };
        let a = argv(&["/opt/catcher", "/n/a b.md"]);
        assert_eq!(
            words(&command_for(Backend::Tmux, Place::SplitDown, &a)),
            argv(&["tmux", "split-window", "-v", "/opt/catcher", "/n/a b.md"])
        );
        assert_eq!(
            words(&command_for(Backend::Tmux, Place::Tab, &a)),
            argv(&["tmux", "new-window", "/opt/catcher", "/n/a b.md"])
        );
        assert_eq!(
            words(&command_for(Backend::Kitty, Place::SplitRight, &a)),
            argv(&[
                "kitten",
                "@",
                "launch",
                "--type=window",
                "--location=vsplit",
                "--cwd=current",
                "/opt/catcher",
                "/n/a b.md"
            ])
        );
        assert_eq!(
            words(&command_for(Backend::WezTerm, Place::SplitRight, &a)),
            argv(&[
                "wezterm",
                "cli",
                "split-pane",
                "--right",
                "--",
                "/opt/catcher",
                "/n/a b.md"
            ])
        );
        assert_eq!(
            words(&command_for(Backend::WezTerm, Place::Tab, &a)),
            argv(&["wezterm", "cli", "spawn", "--", "/opt/catcher", "/n/a b.md"])
        );
    }
}
