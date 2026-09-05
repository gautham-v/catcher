//! Hand-rolled argument parsing. No clap: the surface is a handful of shapes.
//!
//! ```text
//! catcher                 open the TUI on the newest note
//! catcher <name>          fuzzy-open a note by title; an error if none matches
//! catcher <file>.md       open the TUI on that file, rooted at its parent
//! catcher <dir>           open the TUI rooted at that directory
//! catcher --root <dir> <file>
//!                         open <file>, rooted at <dir>: what a split runs
//! catcher new <name>      create a note titled <name> and open it
//! catcher today           open today's daily note, creating it if missing
//! catcher add "text"      capture a note without the TUI (stdin if no text)
//! catcher path            print the resolved notes dir
//! ```
//!
//! `--root` and `--keys` are internal or diagnostic and kept out of `--help`.

use std::path::PathBuf;

pub const USAGE: &str = "\
catcher — a tiny markdown notes TUI over plain files

usage:
  catcher                 open the notes TUI
  catcher <name>          open the note whose title best matches
  catcher <file>.md       open that file, rooted at its parent directory
  catcher <dir>           open the TUI rooted at that directory
  catcher new <name>      create a note titled <name> and open it
  catcher today           open today's daily note, creating it if missing
  catcher add [text]      write a new note from text (or stdin) and print its path
  catcher path            print the notes directory
  catcher --version       print the version
  catcher --help          this message
";

/// What a bare argument turned out to be on disk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathKind {
    Missing,
    File,
    Dir,
}

/// Where a TUI session should start.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// The configured notes dir, newest note.
    Default,
    /// Fuzzy-match this against note titles; an error if nothing matches.
    Name(String),
    /// Make a note with this title and open it; an error if one already has it.
    New(String),
    /// This file, with the session rooted at its parent directory.
    File(PathBuf),
    /// This directory, as a per-invocation notes dir.
    Dir(PathBuf),
    /// This file, in a session rooted at this directory: what one catcher
    /// hands another when it opens a note in a new split or tab, so the
    /// second one sees the same vault as the first.
    In { root: PathBuf, file: PathBuf },
    /// Today's daily note, made if missing.
    Today,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cli {
    Tui(Launch),
    /// `add` with inline text, or `None` to read stdin.
    Add(Option<String>),
    PrintPath,
    /// `--keys`: print raw key events until Esc, for debugging a terminal.
    Keys,
    Help,
    Version,
    /// Bad usage: message for stderr, exit 2.
    Error(String),
}

/// Parse argv (without argv[0]). `probe` answers what a bare string is on disk,
/// which is what separates `catcher notes` the directory from `catcher notes`
/// the note title — injected so the dispatch is testable without a filesystem.
pub fn parse(args: &[String], probe: impl Fn(&str) -> PathKind) -> Cli {
    let Some(first) = args.first() else {
        return Cli::Tui(Launch::Default);
    };

    match first.as_str() {
        "-h" | "--help" | "help" => return Cli::Help,
        "-V" | "--version" | "version" => return Cli::Version,
        "new" => {
            let name = args[1..].join(" ");
            let name = name.trim();
            return if name.is_empty() {
                Cli::Error("new takes a title".to_string())
            } else {
                Cli::Tui(Launch::New(name.to_string()))
            };
        }
        "path" if args.len() == 1 => return Cli::PrintPath,
        "today" if args.len() == 1 => return Cli::Tui(Launch::Today),
        "--keys" => return Cli::Keys,
        "--root" => {
            return match args {
                [_, root, file] => Cli::Tui(Launch::In {
                    root: PathBuf::from(root),
                    file: PathBuf::from(file),
                }),
                _ => Cli::Error("--root takes a directory and a file".to_string()),
            };
        }
        "add" => {
            let rest = args[1..].join(" ");
            let rest = rest.trim();
            return Cli::Add(if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            });
        }
        s if s.starts_with('-') => {
            return Cli::Error(format!("unknown option “{s}”"));
        }
        _ => {}
    }

    // A single argument may be a path; anything else is a title to match.
    if args.len() == 1 {
        match probe(first) {
            PathKind::Dir => return Cli::Tui(Launch::Dir(PathBuf::from(first))),
            PathKind::File => return Cli::Tui(Launch::File(PathBuf::from(first))),
            PathKind::Missing if looks_like_path(first) => {
                return Cli::Error(format!("no such file or directory: {first}"));
            }
            PathKind::Missing => {}
        }
    }

    Cli::Tui(Launch::Name(args.join(" ")))
}

/// Something the user clearly meant as a path, so a missing target is reported
/// as such rather than as a title that matched nothing.
fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.starts_with('~') || s.ends_with(".md")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn nothing(_: &str) -> PathKind {
        PathKind::Missing
    }

    #[test]
    fn no_args_opens_the_tui() {
        assert_eq!(parse(&[], nothing), Cli::Tui(Launch::Default));
    }

    #[test]
    fn help_and_unknown_flags() {
        assert_eq!(parse(&args(&["--help"]), nothing), Cli::Help);
        assert_eq!(parse(&args(&["-h"]), nothing), Cli::Help);
        assert!(matches!(parse(&args(&["--wat"]), nothing), Cli::Error(_)));
    }

    #[test]
    fn root_takes_a_directory_and_a_file() {
        assert_eq!(
            parse(&args(&["--root", "/v", "/v/a/b.md"]), nothing),
            Cli::Tui(Launch::In {
                root: "/v".into(),
                file: "/v/a/b.md".into()
            })
        );
        assert!(matches!(
            parse(&args(&["--root", "/v"]), nothing),
            Cli::Error(_)
        ));
    }

    #[test]
    fn path_subcommand() {
        assert_eq!(parse(&args(&["path"]), nothing), Cli::PrintPath);
        assert_eq!(parse(&args(&["--keys"]), nothing), Cli::Keys);
        // "path" with more words is a title, not the subcommand
        assert_eq!(
            parse(&args(&["path", "of", "least"]), nothing),
            Cli::Tui(Launch::Name("path of least".into()))
        );
    }

    #[test]
    fn today_opens_the_daily_note_unless_more_words_follow() {
        assert_eq!(parse(&args(&["today"]), nothing), Cli::Tui(Launch::Today));
        assert_eq!(
            parse(&args(&["today", "plans"]), nothing),
            Cli::Tui(Launch::Name("today plans".into()))
        );
    }

    #[test]
    fn version() {
        assert_eq!(parse(&args(&["--version"]), nothing), Cli::Version);
        assert_eq!(parse(&args(&["-V"]), nothing), Cli::Version);
    }

    #[test]
    fn new_takes_a_title() {
        assert_eq!(
            parse(&args(&["new", "meeting", "notes"]), nothing),
            Cli::Tui(Launch::New("meeting notes".into()))
        );
        assert!(matches!(parse(&args(&["new"]), nothing), Cli::Error(_)));
        assert!(matches!(
            parse(&args(&["new", " "]), nothing),
            Cli::Error(_)
        ));
    }

    #[test]
    fn add_takes_text_or_stdin() {
        assert_eq!(
            parse(&args(&["add", "buy milk"]), nothing),
            Cli::Add(Some("buy milk".into()))
        );
        assert_eq!(
            parse(&args(&["add", "buy", "milk"]), nothing),
            Cli::Add(Some("buy milk".into()))
        );
        assert_eq!(parse(&args(&["add"]), nothing), Cli::Add(None));
        assert_eq!(parse(&args(&["add", "  "]), nothing), Cli::Add(None));
    }

    #[test]
    fn a_bare_word_is_a_note_title() {
        assert_eq!(
            parse(&args(&["groceries"]), nothing),
            Cli::Tui(Launch::Name("groceries".into()))
        );
        assert_eq!(
            parse(&args(&["meeting", "notes"]), nothing),
            Cli::Tui(Launch::Name("meeting notes".into()))
        );
    }

    #[test]
    fn existing_paths_win_over_titles() {
        let as_dir = |_: &str| PathKind::Dir;
        assert_eq!(
            parse(&args(&["notes"]), as_dir),
            Cli::Tui(Launch::Dir("notes".into()))
        );
        let as_file = |_: &str| PathKind::File;
        assert_eq!(
            parse(&args(&["a/b.md"]), as_file),
            Cli::Tui(Launch::File("a/b.md".into()))
        );
    }

    #[test]
    fn a_missing_path_is_a_path_error_not_a_title() {
        assert!(matches!(
            parse(&args(&["/nope/x.md"]), nothing),
            Cli::Error(_)
        ));
        assert!(matches!(parse(&args(&["./nope"]), nothing), Cli::Error(_)));
        // but a plain word with no slash is a title
        assert_eq!(
            parse(&args(&["nope"]), nothing),
            Cli::Tui(Launch::Name("nope".into()))
        );
    }
}
