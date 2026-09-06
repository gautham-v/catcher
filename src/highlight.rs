//! Colour inside a fenced code block: syntect parses, catcher decides.
//!
//! syntect ships themes of its own and none of them is catcher's, so this
//! stops at the parse. It walks the scope stack a grammar puts over each
//! token and maps the top scope that means anything onto one of eight roles;
//! `theme` alone says what colour a role is drawn in, the same as everywhere
//! else. Eight roles and not fifty because a note is read at prose distance —
//! keyword, string, number, comment, type, function, operator and punctuation
//! are as much as a fence can carry before it stops being text and starts
//! being a picture. Everything else keeps the code foreground it already had.
//!
//! The syntax set costs a couple of hundred milliseconds to load, so it is
//! loaded once, lazily, and only when a fence that actually names a language
//! syntect knows is about to be drawn: a note without one never pays for it.
//! What comes out is remembered per fence, because both views ask for the
//! same fence once per row per pass and scrolling must never re-parse.
//!
//! The parse itself happens on a worker thread, because the first fence in a
//! language pays for that grammar's regexes being compiled — a second in a
//! debug build, and a note with four fences used to hold the very first frame
//! for long enough that the opener never played. So [`runs`] answers from the
//! cache or answers `None` and asks the worker; the fence draws in the plain
//! code colour for a frame or two and [`poll`] fills the colour in.

use crate::theme;
use ratatui::style::Style;
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{OnceLock, RwLock};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

/// What a token in a fence is, as far as a reader at prose distance cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Keyword,
    Str,
    Number,
    Comment,
    Type,
    Function,
    Operator,
    Punctuation,
}

/// One run of a line that shares a role: how many bytes of it the run covers.
/// Bytes and not chars because the reading view maps runs back onto source
/// offsets; the editor turns them into columns on its way past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: usize,
    pub role: Option<Role>,
}

/// Whether fences are coloured at all — the `code_colors` setting. Off is the
/// old behaviour exactly: no syntax set is loaded and no fence is parsed.
static ENABLED: RwLock<bool> = RwLock::new(true);

pub fn set_enabled(on: bool) {
    if let Ok(mut w) = ENABLED.write() {
        *w = on;
    }
}

pub fn enabled() -> bool {
    ENABLED.read().map(|b| *b).unwrap_or(true)
}

/// Whether the reading view rules a fence with line numbers — the
/// `code_numbers` setting. The indent guides follow it: both are the same
/// answer to "how deep am I in this block", and a reader who turned one off
/// did not ask for the other.
static NUMBERS: RwLock<bool> = RwLock::new(true);

pub fn set_numbers(on: bool) {
    if let Ok(mut w) = NUMBERS.write() {
        *w = on;
    }
}

pub fn numbers() -> bool {
    NUMBERS.read().map(|b| *b).unwrap_or(true)
}

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();

fn syntaxes() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// The language a fence's info string names, when colour is on and syntect
/// has a grammar for it. The gate both views ask before they buffer a fence,
/// so an unknown or missing language costs nothing beyond the lookup.
pub fn language(info: &str) -> Option<&str> {
    if !enabled() {
        return None;
    }
    let token = info.split_whitespace().next()?;
    // the lookup itself is cheap — a grammar's regexes compile later, on the
    // worker — but loading the set is not, and the load is the worker's job
    // from `warm` on. So a thread with a worker takes the set only if it is
    // already there and draws the fence plain until it is; a thread without
    // one (a test) loads it here, as this always did.
    let loaded = WORKER.with(|w| w.borrow().is_none());
    let set = if loaded {
        Some(syntaxes())
    } else {
        SYNTAXES.get()
    };
    set?.find_syntax_by_token(token).map(|_| token)
}

/// The style a role is drawn in: the code block's own foreground and ground,
/// with the role's hue over it. Hue and nothing else — a fence is already a
/// patch of its own, and italics inside one only blur it at terminal sizes.
pub fn style(role: Option<Role>) -> Style {
    let base = theme::code();
    let p = theme::palette();
    match role {
        None => base,
        Some(Role::Keyword) => base.fg(p.code_keyword),
        Some(Role::Str) => base.fg(p.code_string),
        Some(Role::Number) => base.fg(p.code_number),
        Some(Role::Comment) => base.fg(p.code_comment),
        Some(Role::Type) => base.fg(p.code_type),
        Some(Role::Function) => base.fg(p.code_function),
        Some(Role::Operator) => base.fg(p.code_operator),
        Some(Role::Punctuation) => base.fg(p.code_punctuation),
    }
}

/// Scopes that name a function wherever the grammar puts them — a
/// definition and a call are the same name doing the same job.
const FUNCTION_SCOPES: [&str; 3] = [
    "entity.name.function",
    "support.function",
    "variable.function",
];

/// Scopes that name a type wherever the grammar puts them.
const TYPE_SCOPES: [&str; 8] = [
    "entity.name.type",
    "entity.name.class",
    "entity.name.struct",
    "entity.name.enum",
    "entity.name.trait",
    "entity.name.interface",
    "support.type",
    "support.class",
];

/// The kinds a `storage.type.KIND` scope can carry that make the word a
/// declaration rather than a type name: `fn`, `def`, `class`, `struct`.
const DECL_KINDS: [&str; 7] = [
    "function",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "module",
];

/// Words that open a declaration even when the grammar spells them with the
/// bare `storage.type`, which is the scope Rust also gives `u32`.
const DECL_WORDS: [&str; 16] = [
    "fn",
    "func",
    "function",
    "def",
    "let",
    "const",
    "var",
    "val",
    "static",
    "mut",
    "type",
    "struct",
    "enum",
    "trait",
    "class",
    "interface",
];

/// The role the scope stack over `text` names — top scope first, first hit
/// wins. Top first is what makes `//` a comment (its own scope is the
/// punctuation that opens one, and the comment is underneath it) and a
/// string's quotes part of the string.
pub fn role(scopes: &[String], text: &str) -> Option<Role> {
    scopes
        .iter()
        .rev()
        .find_map(|s| scope_role(s, text))
        .or_else(|| call_name(scopes))
}

/// A word inside a `meta.function-call` that the grammar named nothing more
/// specific than a variable: the call's own name. Only asked once every
/// other scope has come back empty, so a type or a keyword inside a call
/// keeps what it already is.
fn call_name(scopes: &[String]) -> Option<Role> {
    // the innermost `meta.`, or the arguments would be names too: a call's
    // own arguments sit one `meta.group` deeper than the call
    let inner = scopes.iter().rev().find(|s| under(s, "meta"))?;
    let named = scopes
        .last()
        .is_some_and(|s| under(s, "variable") || under(s, "entity.name") || under(s, "meta.path"));
    (under(inner, "meta.function-call") && named).then_some(Role::Function)
}

fn scope_role(scope: &str, text: &str) -> Option<Role> {
    if under(scope, "comment") {
        return Some(Role::Comment);
    }
    if under(scope, "string") {
        return Some(Role::Str);
    }
    if under(scope, "constant.numeric") {
        return Some(Role::Number);
    }
    if under(scope, "keyword.operator") {
        return Some(Role::Operator);
    }
    if under(scope, "keyword") || under(scope, "storage.modifier") {
        return Some(Role::Keyword);
    }
    if under(scope, "storage.type") {
        // one scope doing two jobs: Rust spells both `let` and `u32`
        // `storage.type`, so the grammar's own sub-kind decides, and the
        // word itself when there isn't one
        return Some(if declares(scope, text) {
            Role::Keyword
        } else {
            Role::Type
        });
    }
    if FUNCTION_SCOPES.iter().any(|p| under(scope, p)) {
        return Some(Role::Function);
    }
    if TYPE_SCOPES.iter().any(|p| under(scope, p)) {
        return Some(Role::Type);
    }
    // `punctuation.definition.string` opens a string and `…comment` opens a
    // comment: those are the thing they open, and the scope under them says
    // so. Only punctuation that defines nothing — a bracket, a comma, a
    // semicolon — is punctuation in its own right.
    (under(scope, "punctuation") && !under(scope, "punctuation.definition"))
        .then_some(Role::Punctuation)
}

/// Is `scope` `prefix`, or something under it? Whole dot-separated segments
/// only, or `meta.function` would read as a function.
fn under(scope: &str, prefix: &str) -> bool {
    scope
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

fn declares(scope: &str, text: &str) -> bool {
    let kind = scope.split('.').nth(2);
    kind.is_some_and(|k| DECL_KINDS.contains(&k)) || DECL_WORDS.contains(&text.trim())
}

/// One remembered fence: the language, a hash of its body, and the runs.
type Cached = (String, u64, Rc<Vec<Vec<Run>>>);

/// One finished parse on its way back from the worker: the key it was asked
/// for, and what came out — `None` when syntect had no grammar after all.
type Answer = (String, u64, Option<Vec<Vec<Run>>>);

/// The language of the answer the worker sends once the syntax set is loaded
/// and before it has parsed anything. No fence is named the empty string, so
/// it cannot be mistaken for one; what it buys is the redraw that lets the
/// fences on screen ask for a language now that asking is free.
const READY: &str = "";

/// The worker thread's two ends, owned by the thread that asked for a fence.
struct Worker {
    tx: Sender<(String, String, u64)>,
    rx: Receiver<Answer>,
}

thread_local! {
    /// The fences drawn most recently. A handful is plenty: a page holds few
    /// fences, and both views walk them back to back, row by row.
    static CACHE: RefCell<Vec<Cached>> = const { RefCell::new(Vec::new()) };
    /// Fences asked for and not yet answered, so a fence on screen is only
    /// ever sent once however many times a frame asks about it.
    static PENDING: RefCell<Vec<(String, u64)>> = const { RefCell::new(Vec::new()) };
    /// Languages the worker came back empty on. Without this a fence syntect
    /// cannot parse would be sent again every frame, for ever.
    static UNKNOWN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Started on the first request, one per thread that asks. In the app
    /// that is the main thread and nothing else.
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

const CACHE_MAX: usize = 8;

/// Start the worker and have it load the syntax set, so the load is paid off
/// the main thread rather than inside the first frame. Called once at start.
pub fn warm() {
    WORKER.with(|w| {
        with_worker(&mut w.borrow_mut());
    });
}

/// The worker, started if this thread has not started it yet.
fn with_worker(slot: &mut Option<Worker>) -> &Worker {
    slot.get_or_insert_with(|| {
        let (tx, jobs) = std::sync::mpsc::channel::<(String, String, u64)>();
        let (done, rx) = std::sync::mpsc::channel::<Answer>();
        std::thread::spawn(move || {
            // touching the set here is the point of `warm`: the packdump
            // load, and every grammar's regexes after it, are the worker's
            let _ = syntaxes();
            if done.send((READY.to_string(), 0, None)).is_err() {
                return;
            }
            while let Ok((lang, body, key)) = jobs.recv() {
                if done.send((lang.clone(), key, parse(&lang, &body))).is_err() {
                    break;
                }
            }
        });
        Worker { tx, rx }
    })
}

/// The style runs for every line of `body`, parsed as `lang` — one `Vec<Run>`
/// per line, in order, the line ending left out. `None` when the parse has
/// not landed yet, and `None` for good when syntect has no grammar for the
/// language.
///
/// Keyed by language and a hash of the body, so an edit inside a fence simply
/// misses and reparses, and a scroll over one never does. A miss is a request
/// to the worker and nothing more: the caller draws the fence plain, and the
/// colours arrive through [`poll`] a frame or two later.
pub fn runs(lang: &str, body: &str) -> Option<Rc<Vec<Vec<Run>>>> {
    let key = hash(body);
    let hit = CACHE.with(|c| {
        c.borrow()
            .iter()
            .find(|(l, h, _)| l == lang && *h == key)
            .map(|(_, _, runs)| runs.clone())
    });
    if hit.is_some() {
        return hit;
    }
    if UNKNOWN.with(|u| u.borrow().iter().any(|l| l == lang)) {
        return None;
    }
    let fresh = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let fresh = !p.iter().any(|(l, h)| l == lang && *h == key);
        if fresh {
            p.push((lang.to_string(), key));
        }
        fresh
    });
    if fresh {
        WORKER.with(|w| {
            let mut w = w.borrow_mut();
            let sent = with_worker(&mut w)
                .tx
                .send((lang.to_string(), body.to_string(), key))
                .is_ok();
            if !sent {
                // the worker died; drop the claim so a later frame retries
                *w = None;
                PENDING.with(|p| p.borrow_mut().retain(|(_, h)| *h != key));
            }
        });
    }
    None
}

/// Whether a fence is still out with the worker. The event loop asks so it
/// can wait a frame rather than a tenth of a second while colours are coming.
pub fn pending() -> bool {
    PENDING.with(|p| !p.borrow().is_empty())
}

/// Take every parse the worker has finished into the cache. Returns whether
/// anything arrived, which is what makes the frame redraw with the colours in
/// it. Called from `App::tick` like the other polls.
pub fn poll() -> bool {
    let mut any = false;
    WORKER.with(|w| {
        let w = w.borrow();
        let Some(worker) = w.as_ref() else {
            return;
        };
        // everything waiting, not one a frame: a page of fences comes back
        // together and the redraw shows all of it at once
        while let Ok((lang, key, parsed)) = worker.rx.try_recv() {
            PENDING.with(|p| p.borrow_mut().retain(|(l, h)| !(*l == lang && *h == key)));
            match parsed {
                Some(runs) => remember(&lang, key, Rc::new(runs)),
                // no grammar after all: never ask for this one again
                None if lang != READY => UNKNOWN.with(|u| u.borrow_mut().push(lang)),
                // the set is loaded; the redraw this returns is what puts the
                // languages on screen back in front of `language`
                None => {}
            }
            any = true;
        }
    });
    any
}

fn remember(lang: &str, key: u64, runs: Rc<Vec<Vec<Run>>>) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.insert(0, (lang.to_string(), key, runs));
        c.truncate(CACHE_MAX);
    });
}

/// [`runs`], parsed here and now rather than asked of the worker. Only the
/// tests use it: they assert on colours a frame at a time, with no loop to
/// poll for them.
#[cfg(test)]
pub fn runs_now(lang: &str, body: &str) -> Option<Rc<Vec<Vec<Run>>>> {
    let key = hash(body);
    if let Some(hit) = CACHE.with(|c| {
        c.borrow()
            .iter()
            .find(|(l, h, _)| l == lang && *h == key)
            .map(|(_, _, runs)| runs.clone())
    }) {
        return Some(hit);
    }
    let runs = Rc::new(parse(lang, body)?);
    remember(lang, key, runs.clone());
    Some(runs)
}

fn hash(body: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    h.finish()
}

fn parse(lang: &str, body: &str) -> Option<Vec<Vec<Run>>> {
    let set = syntaxes();
    let syntax = set.find_syntax_by_token(lang)?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::new();
    for raw in body.split_inclusive('\n') {
        // the grammars are the newline-terminated set, so the line goes to
        // the parser as it is in the file and only the runs drop the ending
        let ops = state.parse_line(raw, set).ok()?;
        let line = raw.trim_end_matches('\n').trim_end_matches('\r');
        let mut runs: Vec<Run> = Vec::new();
        let mut last = 0;
        for (at, op) in &ops {
            let mut at = (*at).min(line.len());
            while at > last && !line.is_char_boundary(at) {
                at -= 1;
            }
            if at > last {
                runs.push(run(&line[last..at], &stack));
                last = at;
            }
            stack.apply(op).ok()?;
        }
        if last < line.len() {
            runs.push(run(&line[last..], &stack));
        }
        out.push(runs);
    }
    Some(out)
}

/// One run of `text` under `stack`. Whitespace never asks the stack: it has
/// no colour to show anyway, and skipping it keeps the scope names off the
/// heap for most of a line.
fn run(text: &str, stack: &ScopeStack) -> Run {
    let role = if text.trim().is_empty() {
        None
    } else {
        let names: Vec<String> = stack.as_slice().iter().map(|s| s.build_string()).collect();
        role(&names, text)
    };
    Run {
        len: text.len(),
        role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// every run of `body`, as the text it covers and the role it took
    fn roles(lang: &str, body: &str) -> Vec<(String, Option<Role>)> {
        let runs = runs_now(lang, body).expect("a language syntect knows");
        pairs(body, &runs)
    }

    /// the runs of `body` spelt out as the text each covers and its role
    fn pairs(body: &str, runs: &[Vec<Run>]) -> Vec<(String, Option<Role>)> {
        let mut out = Vec::new();
        for (raw, line_runs) in body.split_inclusive('\n').zip(runs.iter()) {
            let line = raw.trim_end_matches('\n');
            let mut at = 0;
            for r in line_runs {
                out.push((line[at..at + r.len].to_string(), r.role));
                at += r.len;
            }
        }
        out
    }

    /// the role a snippet gave `word`, the first time it appears
    fn role_of(pairs: &[(String, Option<Role>)], word: &str) -> Option<Role> {
        pairs
            .iter()
            .find(|(t, _)| t == word)
            .unwrap_or_else(|| panic!("{word} is not a run of its own in {pairs:?}"))
            .1
    }

    #[test]
    fn a_rust_snippet_takes_the_eight_roles() {
        set_enabled(true);
        let src = "// a note\npub fn wikilink_at(a: u32) -> u32 {\n    let n = a + 12;\n    if n != 0 { \"hi\" }\n}\n";
        let r = roles("rust", src);
        assert_eq!(role_of(&r, "//"), Some(Role::Comment));
        assert_eq!(role_of(&r, " a note"), Some(Role::Comment));
        assert_eq!(role_of(&r, "fn"), Some(Role::Keyword));
        assert_eq!(role_of(&r, "pub"), Some(Role::Keyword));
        assert_eq!(role_of(&r, "let"), Some(Role::Keyword));
        // the same `storage.type` scope as `let`, told apart by the word
        assert_eq!(role_of(&r, "u32"), Some(Role::Type));
        assert_eq!(role_of(&r, "12"), Some(Role::Number));
        assert_eq!(role_of(&r, "hi"), Some(Role::Str));
        // a name is a name whether it is being declared or called
        assert_eq!(role_of(&r, "wikilink_at"), Some(Role::Function));
        assert_eq!(role_of(&r, "+"), Some(Role::Operator));
        assert_eq!(role_of(&r, "!="), Some(Role::Operator));
        assert_eq!(role_of(&r, "("), Some(Role::Punctuation));
        assert_eq!(role_of(&r, "{"), Some(Role::Punctuation));
        // and every run together is the source back again
        let back: String = r.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(back, src.replace('\n', ""));
    }

    #[test]
    fn a_script_snippet_takes_them_too() {
        set_enabled(true);
        let r = roles(
            "js",
            "function wikilink_at(a) {\n  return wikilink_at(a + 1) != 2;\n}\n",
        );
        assert_eq!(role_of(&r, "function"), Some(Role::Keyword));
        // declared, and called again inside itself
        assert_eq!(role_of(&r, "wikilink_at"), Some(Role::Function));
        assert_eq!(role_of(&r, "+"), Some(Role::Operator));
        assert_eq!(role_of(&r, "!="), Some(Role::Operator));
        assert_eq!(role_of(&r, "("), Some(Role::Punctuation));
        assert_eq!(role_of(&r, "{"), Some(Role::Punctuation));
        // a call's argument is not its name: only the innermost `meta.` scope
        // being the call itself makes a bare word a function
        assert_eq!(role_of(&r, "a"), None);
    }

    #[test]
    fn typescript_is_a_language_too() {
        // syntect's own set stops at javascript; bat's, through two-face,
        // knows the languages people actually paste into notes
        set_enabled(true);
        assert_eq!(language("ts"), Some("ts"));
        assert_eq!(language("tsx"), Some("tsx"));
        let r = roles(
            "ts",
            "interface Roster { owner: string }\nexport async function load(id: string): Promise<Roster[]> {\n  return await fetch(id);\n}\n",
        );
        assert_eq!(role_of(&r, "interface"), Some(Role::Keyword));
        assert_eq!(role_of(&r, "load"), Some(Role::Function));
        assert_eq!(role_of(&r, "fetch"), Some(Role::Function));
        assert!(matches!(
            role_of(&r, "string"),
            Some(Role::Type | Role::Keyword)
        ));
    }

    #[test]
    fn a_shell_snippet_takes_them_as_well() {
        set_enabled(true);
        let r = roles(
            "sh",
            "# a note\nname=\"world\"\nfor i in 1 2 3; do\n  echo ok\ndone\n",
        );
        assert_eq!(role_of(&r, "#"), Some(Role::Comment));
        assert_eq!(role_of(&r, " a note"), Some(Role::Comment));
        assert_eq!(role_of(&r, "for"), Some(Role::Keyword));
        assert_eq!(role_of(&r, "done"), Some(Role::Keyword));
        assert_eq!(role_of(&r, "world"), Some(Role::Str));
        // a builtin is the shell's function, and reads as one
        assert_eq!(role_of(&r, "echo"), Some(Role::Function));
    }

    #[test]
    fn a_scope_is_matched_by_whole_segments_and_from_the_top_down() {
        // `meta.function` is not a function, and the punctuation that opens a
        // comment is read as the comment underneath it
        assert_eq!(role(&["meta.function.rust".into()], "add"), None);
        let stack = [
            "source.rust".into(),
            "comment.line.double-slash.rust".into(),
            "punctuation.definition.comment.rust".into(),
        ];
        assert_eq!(role(&stack, "//"), Some(Role::Comment));
        // `storage.type` is a declaration when the grammar says so, or when
        // the word itself is one; otherwise it names a type
        assert_eq!(
            role(&["storage.type.function.rust".into()], "fn"),
            Some(Role::Keyword)
        );
        assert_eq!(
            role(&["storage.type.rust".into()], "let"),
            Some(Role::Keyword)
        );
        assert_eq!(role(&["storage.type.rust".into()], "u32"), Some(Role::Type));
        assert_eq!(
            role(&["storage.modifier.rust".into()], "pub"),
            Some(Role::Keyword)
        );
        assert_eq!(
            role(&["support.type.builtin.go".into()], "error"),
            Some(Role::Type)
        );
        assert_eq!(role(&["variable.other.rust".into()], "x"), None);
        // `keyword.operator` is an operator now, not a keyword
        assert_eq!(
            role(&["keyword.operator.arithmetic.rust".into()], "+"),
            Some(Role::Operator)
        );
        assert_eq!(
            role(&["entity.name.function.rust".into()], "add"),
            Some(Role::Function)
        );
        assert_eq!(
            role(&["punctuation.section.group.begin.rust".into()], "("),
            Some(Role::Punctuation)
        );
        // but punctuation that *defines* something is that thing: the quotes
        // are the string they open
        let quoted = [
            "string.quoted.double.rust".into(),
            "punctuation.definition.string.begin.rust".into(),
        ];
        assert_eq!(role(&quoted, "\""), Some(Role::Str));
    }

    #[test]
    fn a_fence_with_a_language_nobody_knows_is_left_alone() {
        let _lock = crate::testutil::serial();
        set_enabled(true);
        assert_eq!(language("rust"), Some("rust"));
        // the info string's first word, and nothing else on the line
        assert_eq!(language("python {highlight: 2}"), Some("python"));
        assert_eq!(language("gibberish"), None);
        assert_eq!(language(""), None);
        assert!(runs_now("gibberish", "x\n").is_none());
    }

    #[test]
    fn colour_off_stops_a_fence_being_offered_a_language_at_all() {
        let _lock = crate::testutil::serial();
        set_enabled(false);
        assert_eq!(language("rust"), None);
        set_enabled(true);
        assert_eq!(language("rust"), Some("rust"));
    }

    #[test]
    fn a_fence_is_parsed_once_and_answered_from_the_cache_after() {
        set_enabled(true);
        let body = "fn main() {}\n";
        let first = runs_now("rust", body).unwrap();
        let again = runs_now("rust", body).unwrap();
        assert!(Rc::ptr_eq(&first, &again));
        // an edit changes the hash and so misses
        let edited = runs_now("rust", "fn other() {}\n").unwrap();
        assert!(!Rc::ptr_eq(&first, &edited));
    }

    #[test]
    fn a_miss_is_answered_by_the_worker_and_only_asked_for_once() {
        set_enabled(true);
        let body = "fn worker_test() {}\n";
        // nothing in the cache: the fence is drawn plain and the worker asked
        assert!(runs("rust", body).is_none());
        assert!(pending());
        // asking again while it is out sends nothing more
        assert!(runs("rust", body).is_none());
        // the worker has a grammar to compile the first time round, so this
        // waits for it rather than assuming a frame is long enough
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while pending() {
            assert!(
                std::time::Instant::now() < deadline,
                "the worker never answered"
            );
            poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        let hit = runs("rust", body).expect("the parse landed in the cache");
        assert_eq!(hit.len(), 1);
        assert_eq!(role_of(&pairs(body, &hit), "fn"), Some(Role::Keyword));
    }

    #[test]
    fn every_role_is_a_hue_and_nothing_but_a_hue() {
        theme::set_palette(theme::DARK);
        let p = theme::DARK;
        assert_eq!(style(None), theme::code());
        for (role, fg) in [
            (Role::Keyword, p.code_keyword),
            (Role::Str, p.code_string),
            (Role::Number, p.code_number),
            (Role::Type, p.code_type),
            (Role::Comment, p.code_comment),
            (Role::Function, p.code_function),
            (Role::Operator, p.code_operator),
            (Role::Punctuation, p.code_punctuation),
        ] {
            let s = style(Some(role));
            assert_eq!(s.fg, Some(fg), "{role:?}");
            // the block's own ground is kept, whatever the role, and no role
            // leans or bolds: a comment used to, and it only blurred
            assert_eq!(s.bg, Some(p.code_bg), "{role:?}");
            assert!(s.add_modifier.is_empty(), "{role:?}");
        }
    }
}
