//! Sequence diagrams: `sequenceDiagram`, its participants, and the messages
//! that pass between them.
//!
//! A sequence diagram is easier to lay out than a flowchart and harder to lay
//! out *narrow*: the participants are columns across the top in the order they
//! were introduced, every one of them costs the width of its name, and time
//! runs down the page as lifelines. Nothing here has to be routed — a message
//! is one row, always between two known columns — so the whole of the work is
//! choosing column positions and then drawing rows.
//!
//! Everything drawn goes on a [`Canvas`], which does the junctions and the
//! character widths, so this file is only ever deciding *where*.

use super::canvas::{Canvas, Shape, Side};
use super::{Rendered, Role};
use crate::md::str_width;

/// How a message's line is drawn: mermaid's solid and dotted arrows, the
/// latter being the reply half of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    /// `->`, `->>`, `-x`
    Solid,
    /// `-->`, `-->>`, `--x`
    Dotted,
}

/// What the far end of a message carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    /// `->` and `-->`: a line with no head.
    None,
    /// `->>` and `-->>`: the ordinary arrow.
    Arrow,
    /// `-x` and `--x`: the message that fails.
    Cross,
}

/// A column: someone the messages are between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    /// The identifier messages name it by.
    pub id: String,
    /// What is drawn in its box — the `as` alias when it was given one, and
    /// otherwise the id itself.
    pub label: String,
}

/// One row of the diagram, in the order it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// `A->>B: text`
    Message {
        /// Index into [`Script::participants`].
        from: usize,
        to: usize,
        text: String,
        stroke: Stroke,
        head: Head,
    },
    /// `Note over A,B: text` and `Note right of A: text`.
    Note {
        /// The participants the note is drawn across, by index.
        over: Vec<usize>,
        text: String,
    },
}

/// A parsed sequence diagram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    pub participants: Vec<Participant>,
    pub steps: Vec<Step>,
}

/// The narrowest a gap between two adjacent columns is ever squeezed to. Below
/// three the two lifelines read as one thing rather than two.
const MIN_GAP: usize = 3;

/// Room a message's words want either side of themselves, so a label never sits
/// flush against the lifeline it was sent from.
const LABEL_PAD: usize = 4;

/// A note is one line of text in a box: a border, the words, a border.
const NOTE_ROWS: usize = 3;

/// A message to oneself: out of the lifeline, down past its own words, back.
const SELF_ROWS: usize = 3;

/// How far a self message reaches out from its lifeline before turning back.
const SELF_REACH: usize = 4;

/// One row of lifeline below the last step, before the footer boxes. Without
/// it the final message's arrow lands on the last row a lifeline reaches, and
/// the junction there reads as the corner of something rather than a crossing.
const TAIL: usize = 1;

/// Draw a sequence diagram, or `None` when its body holds no messages — a
/// diagram with participants and nothing passing between them is a list of
/// names, and the fence already prints those.
pub fn render(src: &str, width: usize) -> Option<Rendered> {
    let script = parse(src);
    if !script
        .steps
        .iter()
        .any(|s| matches!(s, Step::Message { .. }))
    {
        return None;
    }
    Some(Rendered::new(draw(&script, width).rows()))
}

// ── reading the source ────────────────────────────────────────────────────

/// The arrow tokens mermaid spells a message with, longest first: at any one
/// position `-->>` has to be tried before `-->`, or the second `>` is read as
/// the start of a participant's name.
const ARROWS: [&str; 8] = ["-->>", "--x", "--)", "-->", "->>", "-x", "-)", "->"];

/// The statements catcher understands but does not draw. Activation is implied
/// by the messages themselves; a block is a bracket around messages that are
/// worth more, kept flat and in order, than the bracket is.
const IGNORED: [&str; 15] = [
    "activate",
    "deactivate",
    "autonumber",
    "title",
    "link",
    "links",
    "loop",
    "alt",
    "else",
    "opt",
    "par",
    "and",
    "critical",
    "break",
    "rect",
];

/// Read the statements of a sequence body into a [`Script`].
///
/// Deliberately forgiving: a line it does not understand is skipped rather than
/// failing the whole diagram.
pub fn parse(src: &str) -> Script {
    let mut script = Script::default();
    for raw in src.lines() {
        // `%%` comments run to the end of the line, and `;` ends a statement
        let line = raw.split_once("%%").map_or(raw, |(before, _)| before);
        let line = line.trim().trim_end_matches(';').trim();
        if line.is_empty() {
            continue;
        }
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match word.to_ascii_lowercase().as_str() {
            // the header names the kind, which the caller has already read
            "sequencediagram" => {}
            "participant" | "actor" => declare(&mut script, rest),
            "note" => note(&mut script, rest),
            // `box` opens a grouping and `end` closes any block; both go
            "box" | "end" => {}
            w if IGNORED.contains(&w) => {}
            _ => message(&mut script, line),
        }
    }
    script
}

/// `participant A`, or `participant A as Alice`. A name declared twice is the
/// same column: mermaid lets a message introduce someone the author names
/// properly further down, and the later alias is the one they meant.
fn declare(script: &mut Script, rest: &str) {
    if rest.is_empty() {
        return;
    }
    match split_as(rest) {
        Some((id, alias)) => column(script, id, Some(alias)),
        None => column(script, rest, None),
    };
}

/// `A as Alice` split into the id messages use and the label the box shows.
fn split_as(rest: &str) -> Option<(&str, &str)> {
    let at = rest.to_ascii_lowercase().find(" as ")?;
    let (id, alias) = (rest[..at].trim(), rest[at + 4..].trim());
    (!id.is_empty() && !alias.is_empty()).then_some((id, alias))
}

/// The column `id` names, registering it at the end if this is the first time
/// anything has named it. That is the whole of participant ordering: a column
/// sits where it was first mentioned, whether by a declaration or a message.
fn column(script: &mut Script, id: &str, label: Option<&str>) -> usize {
    if let Some(i) = script.participants.iter().position(|p| p.id == id) {
        if let Some(label) = label {
            script.participants[i].label = label.to_string();
        }
        return i;
    }
    script.participants.push(Participant {
        id: id.to_string(),
        label: label.unwrap_or(id).to_string(),
    });
    script.participants.len() - 1
}

/// `A->>B: text` and its seven siblings. The arrow is looked for only in front
/// of the first `:`, so a message whose words hold an `->` keeps them.
fn message(script: &mut Script, line: &str) {
    let (head, text) = match line.split_once(':') {
        Some((head, text)) => (head, text.trim()),
        None => (line, ""),
    };
    let Some((at, arrow)) = find_arrow(head) else {
        return;
    };
    // `A<<->>B` is mermaid's two-way message; drawn one way, but the `<<`
    // belongs to the arrow and never to the name in front of it
    let from = head[..at].trim().trim_end_matches('<').trim();
    // `A->>+B` activates B; the marker is not part of the name
    let to = head[at + arrow.len()..]
        .trim()
        .trim_start_matches(['+', '-'])
        .trim();
    if from.is_empty() || to.is_empty() {
        return;
    }
    let (stroke, tail) = match arrow.strip_prefix("--") {
        Some(tail) => (Stroke::Dotted, tail),
        None => (Stroke::Solid, &arrow[1..]),
    };
    let head = match tail {
        // `-)` is mermaid's async call: an arrow that does not wait
        ">>" | ")" => Head::Arrow,
        "x" => Head::Cross,
        _ => Head::None,
    };
    let text = text.to_string();
    let from = column(script, from, None);
    let to = column(script, to, None);
    script.steps.push(Step::Message {
        from,
        to,
        text,
        stroke,
        head,
    });
}

/// Where the first arrow token starts, and which one it is.
fn find_arrow(head: &str) -> Option<(usize, &'static str)> {
    (0..head.len())
        .filter(|&i| head.is_char_boundary(i))
        .find_map(|i| {
            ARROWS
                .iter()
                .find(|a| head[i..].starts_with(**a))
                .map(|a| (i, *a))
        })
}

/// `Note over A,B: text`, `Note left of A: text`, `Note right of A: text`.
/// Which side it was asked for does not survive: a terminal column is too
/// narrow to hang a box off, so the note is drawn across the participants it
/// names and the reader gets the association either way.
fn note(script: &mut Script, rest: &str) {
    let (place, text) = match rest.split_once(':') {
        Some((place, text)) => (place.trim(), text.trim()),
        None => (rest, ""),
    };
    let Some(names) = ["over", "left of", "right of"]
        .iter()
        .find_map(|kw| strip_word(place, kw))
    else {
        return;
    };
    let mut over: Vec<usize> = names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| column(script, n, None))
        .collect();
    over.sort_unstable();
    over.dedup();
    if over.is_empty() {
        return;
    }
    script.steps.push(Step::Note {
        over,
        text: text.to_string(),
    });
}

/// `s` with `word` taken off the front, case-insensitively, when `s` starts
/// with that whole word — `overture` does not begin a note over anyone.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let rest = s.get(word.len()..)?;
    let matched =
        s[..word.len()].eq_ignore_ascii_case(word) && rest.starts_with(char::is_whitespace);
    matched.then(|| rest.trim())
}

// ── laying it out ─────────────────────────────────────────────────────────

/// Where every column sits: the left edge, the centre its lifeline runs down,
/// and the width of the box at its head and foot.
struct Columns {
    x: Vec<usize>,
    cx: Vec<usize>,
    w: Vec<usize>,
    /// The height of a participant box — the same for all of them, so the
    /// lifelines all start on one row.
    h: usize,
    /// The width the whole picture wants.
    total: usize,
}

/// Place the columns across the page.
///
/// The only real constraint is a message's own words: a label is centred over
/// its arrow, so the gap it crosses has to be wide enough to hold it or the
/// words spill over a lifeline. Every gap under the message's span pays that
/// price, which is generous for a message that leaps three columns and exactly
/// right for the ordinary one that goes next door.
fn columns(script: &Script, width: usize) -> Columns {
    let sizes: Vec<(usize, usize)> = script
        .participants
        .iter()
        .map(|p| Canvas::node_size(Shape::Round, std::slice::from_ref(&p.label)))
        .collect();
    let w: Vec<usize> = sizes.iter().map(|s| s.0).collect();
    let h = sizes.iter().map(|s| s.1).max().unwrap_or(0);

    let mut gaps = vec![MIN_GAP; w.len().saturating_sub(1)];
    for step in &script.steps {
        if let Step::Message { from, to, text, .. } = step {
            let (lo, hi) = (*from.min(to), *from.max(to));
            let need = str_width(text) + LABEL_PAD;
            for gap in gaps.iter_mut().take(hi).skip(lo) {
                *gap = (*gap).max(need);
            }
        }
    }
    // when the picture will not fit, every gap goes back to its minimum and a
    // long label is left to run past its own arrow: the reading view pans a
    // wide diagram, and truncating someone's words would be the worse answer
    if w.iter().sum::<usize>() + gaps.iter().sum::<usize>() > width {
        gaps.fill(MIN_GAP);
    }

    let mut x = Vec::with_capacity(w.len());
    let mut at = 0;
    for (i, wi) in w.iter().enumerate() {
        x.push(at);
        at += wi + gaps.get(i).copied().unwrap_or(0);
    }
    let total = x.last().map_or(0, |last| last + w[w.len() - 1]);
    let cx = x.iter().zip(&w).map(|(x, w)| x + w / 2).collect();
    Columns { x, cx, w, h, total }
}

/// How many rows a step takes. A message is its words and then its arrow; a
/// message to oneself needs a third row for the loop to turn round in.
fn rows_of(step: &Step) -> usize {
    match step {
        Step::Message { from, to, .. } if from == to => SELF_ROWS,
        Step::Message { .. } => 2,
        Step::Note { .. } => NOTE_ROWS,
    }
}

/// Place the script on a canvas, trying to fit `width`.
fn draw(script: &Script, width: usize) -> Canvas {
    let col = columns(script, width);
    let body: usize = script.steps.iter().map(rows_of).sum::<usize>() + TAIL;
    let mut canvas = Canvas::new(col.total, col.h * 2 + body);

    for (i, p) in script.participants.iter().enumerate() {
        head_box(&mut canvas, col.x[i], 0, col.w[i], col.h, &p.label);
    }
    // lifelines first, so anything drawn on top of one wins: a message's words
    // are the point of the row, and a lifeline may be crossed out for them
    for &cx in &col.cx {
        canvas.vline(cx, col.h, body, Role::Line);
    }

    let mut y = col.h;
    for step in &script.steps {
        match step {
            Step::Message {
                from,
                to,
                text,
                head,
                ..
            } if from == to => self_message(&mut canvas, col.cx[*from], y, text, *head),
            Step::Message {
                from,
                to,
                text,
                stroke,
                head,
            } => arrow(&mut canvas, &col, y, *from, *to, text, *stroke, *head),
            Step::Note { over, text } => note_box(&mut canvas, &col, y, over, text),
        }
        y += rows_of(step);
    }

    let foot = y + TAIL;
    for (i, p) in script.participants.iter().enumerate() {
        head_box(&mut canvas, col.x[i], foot, col.w[i], col.h, &p.label);
    }
    canvas
}

/// A participant's box. [`Canvas::node`] writes a label as [`Role::Node`]; a
/// participant's name is the one thing on the row that leads, so it is written
/// again over the top at [`Role::Bright`].
fn head_box(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, label: &str) {
    canvas.node(x, y, w, h, Shape::Round, &[label.to_string()]);
    let pad = w.saturating_sub(2).saturating_sub(str_width(label)) / 2;
    canvas.text(x + 1 + pad, y + 1, label, Role::Bright);
}

/// One message between two columns: its words on one row, its arrow on the
/// next. The arrow runs lifeline to lifeline, so every lifeline it passes
/// under becomes a `┼` by itself — a crossing, which is what it is.
#[allow(clippy::too_many_arguments)]
fn arrow(
    canvas: &mut Canvas,
    col: &Columns,
    y: usize,
    from: usize,
    to: usize,
    text: &str,
    stroke: Stroke,
    head: Head,
) {
    let (src, dst) = (col.cx[from], col.cx[to]);
    let (lo, hi) = (src.min(dst), src.max(dst));
    if !text.is_empty() {
        let mid = (lo + hi) / 2;
        canvas.text(
            mid.saturating_sub(str_width(text) / 2),
            y,
            text,
            Role::Label,
        );
    }
    let len = hi - lo + 1;
    match stroke {
        Stroke::Solid => canvas.hline(lo, y + 1, len, Role::Line),
        // a dashed run, drawn cell by cell: `put` still merges each dash it
        // lands on a lifeline with, so a reply reads dotted and the crossings
        // survive. There is no dashed box-drawing character the junction table
        // knows, so this is the only way to have both. It is dashed outwards
        // from the sender, so the run always leaves its own lifeline on a dash
        // whichever way it runs; the far end is the head's to draw.
        Stroke::Dotted => {
            let out = if dst > src { Side::Right } else { Side::Left };
            canvas.stub(src, y + 1, out, Role::Line);
            for i in (2..len).step_by(2) {
                let x = if dst > src { lo + i } else { hi - i };
                canvas.put(x, y + 1, '─', Role::Line);
            }
        }
    }
    let side = if dst > src { Side::Right } else { Side::Left };
    match head {
        Head::Arrow => canvas.arrow(dst, y + 1, side, Role::Line),
        Head::Cross => canvas.set(dst, y + 1, '✕', Role::Line),
        Head::None => {}
    }
}

/// A message to oneself: out of the lifeline, down past its own words, and
/// back into the lifeline it left.
fn self_message(canvas: &mut Canvas, cx: usize, y: usize, text: &str, head: Head) {
    let far = cx + SELF_REACH - 1;
    canvas.put(cx, y, '├', Role::Line);
    canvas.hline(cx, y, SELF_REACH, Role::Line);
    canvas.put(far, y, '╮', Role::Line);
    canvas.put(far, y + 1, '│', Role::Line);
    if !text.is_empty() {
        canvas.text(far + 2, y + 1, text, Role::Label);
    }
    canvas.hline(cx, y + 2, SELF_REACH, Role::Line);
    canvas.put(far, y + 2, '╯', Role::Line);
    match head {
        Head::Arrow => canvas.arrow(cx + 1, y + 2, Side::Left, Role::Line),
        Head::Cross => canvas.set(cx + 1, y + 2, '✕', Role::Line),
        Head::None => {}
    }
}

/// A note, drawn as a box across the columns it was written over.
///
/// Its footprint is blanked before the box goes down. Without that the
/// lifelines underneath would merge into the box's own edges and every one of
/// them would push a tee through the border — the note has to sit *over* the
/// diagram, the way a sticky note does.
fn note_box(canvas: &mut Canvas, col: &Columns, y: usize, over: &[usize], text: &str) {
    let (lo, hi) = match (over.iter().min(), over.iter().max()) {
        (Some(lo), Some(hi)) => (*lo, *hi),
        _ => return,
    };
    let x = col.x[lo];
    let label = [text.to_string()];
    let span = col.x[hi] + col.w[hi] - x;
    // a note over one narrow column still has to hold its own words
    let w = span.max(Canvas::node_size(Shape::Rect, &label).0);
    for row in y..y + NOTE_ROWS {
        for cell in x..x + w {
            canvas.set(cell, row, ' ', Role::Line);
        }
    }
    canvas.node(x, y, w, NOTE_ROWS, Shape::Rect, &label);
    let pad = w.saturating_sub(2).saturating_sub(str_width(text)) / 2;
    canvas.text(x + 1 + pad, y + 1, text, Role::Label);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn(src: &str) -> Vec<String> {
        render(src, 80)
            .expect("a diagram with messages is drawn")
            .text()
    }

    fn ids(script: &Script) -> Vec<&str> {
        script.participants.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn a_message_draws_an_arrow_between_two_lifelines() {
        assert_eq!(
            drawn("sequenceDiagram\n  A->>B: hi"),
            vec![
                "╭───╮      ╭───╮",
                "│ A │      │ B │",
                "╰───╯      ╰───╯",
                "  │   hi     │",
                "  ├──────────▶",
                "  │          │",
                "╭───╮      ╭───╮",
                "│ A │      │ B │",
                "╰───╯      ╰───╯",
            ]
        );
    }

    #[test]
    fn a_participant_is_registered_the_first_time_a_message_names_it() {
        let script = parse("sequenceDiagram\n A->>B: one\n C->>A: two");
        assert_eq!(ids(&script), vec!["A", "B", "C"]);
        // and a declaration still puts its column ahead of any message's
        let script = parse("sequenceDiagram\n participant Z\n A->>Z: hi");
        assert_eq!(ids(&script), vec!["Z", "A"]);
    }

    #[test]
    fn an_alias_is_what_the_box_says_and_the_id_is_what_messages_use() {
        let src = "sequenceDiagram\n participant A as Alice\n actor B as Bob\n A->>B: hi";
        let script = parse(src);
        assert_eq!(ids(&script), vec!["A", "B"]);
        assert_eq!(script.participants[0].label, "Alice");
        assert_eq!(script.participants[1].label, "Bob");
        assert!(drawn(src)[1].contains("Alice"));
        assert!(drawn(src)[1].contains("Bob"));
    }

    #[test]
    fn a_dotted_reply_is_drawn_differently_from_the_call() {
        let rows = drawn("sequenceDiagram\n A->>B: call\n B-->>A: reply");
        let call = rows.iter().find(|r| r.contains('▶')).unwrap();
        assert!(call.contains("────"), "the call is a solid run: {call}");
        let reply = rows.iter().find(|r| r.contains('◀')).unwrap();
        assert!(reply.contains("─ ─"), "the reply is dashed: {reply}");
    }

    #[test]
    fn a_crossed_message_ends_in_a_cross_not_an_arrow() {
        let rows = drawn("sequenceDiagram\n A-xB: lost");
        assert!(rows.iter().any(|r| r.contains('✕')));
        assert!(!rows.iter().any(|r| r.contains('▶')));
    }

    #[test]
    fn an_arrow_with_no_head_draws_a_bare_line() {
        let rows = drawn("sequenceDiagram\n A->B: plain");
        assert!(rows.iter().any(|r| r.contains("────")));
        assert!(!rows.iter().any(|r| r.contains('▶') || r.contains('◀')));
    }

    #[test]
    fn a_message_to_oneself_loops_on_its_own_lifeline() {
        let rows = drawn("sequenceDiagram\n A->>B: hi\n A->>A: think");
        let loop_rows: Vec<&String> = rows
            .iter()
            .skip_while(|r| !r.contains('╮') || r.contains("╭"))
            .take(3)
            .collect();
        assert!(loop_rows[0].contains('╮'));
        assert!(loop_rows[1].contains("think"));
        assert!(loop_rows[2].contains('╯') && loop_rows[2].contains('◀'));
    }

    #[test]
    fn a_note_over_two_participants_spans_both_and_hides_the_lifelines_under_it() {
        let rows = drawn("sequenceDiagram\n A->>B: hi\n Note over A,B: they agree");
        let top = rows
            .iter()
            .position(|r| r.starts_with('╭') && !r.contains("╮ "))
            .unwrap();
        // the note's borders are unbroken: no lifeline pokes a tee through them
        assert!(!rows[top].contains('┬'), "top border: {}", rows[top]);
        assert!(rows[top + 1].contains("they agree"));
        assert!(
            !rows[top + 2].contains('┴'),
            "bottom border: {}",
            rows[top + 2]
        );
    }

    #[test]
    fn block_keywords_are_skipped_and_the_messages_inside_them_kept() {
        let script = parse(
            "sequenceDiagram\n\
             loop every minute\n\
               A->>B: poll\n\
               alt it answered\n\
                 B-->>A: yes\n\
               else it did not\n\
                 B-->>A: no\n\
               end\n\
             end",
        );
        assert_eq!(ids(&script), vec!["A", "B"]);
        assert_eq!(script.steps.len(), 3);
        let texts: Vec<&str> = script
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Message { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["poll", "yes", "no"]);
    }

    #[test]
    fn an_activation_suffix_does_not_become_part_of_the_participant_name() {
        let script =
            parse("sequenceDiagram\n A->>+B: hi\n B-->>-A: ok\n activate B\n deactivate B");
        assert_eq!(ids(&script), vec!["A", "B"]);
        assert_eq!(script.steps.len(), 2);
        // and neither does the near half of a two-way arrow
        assert_eq!(
            ids(&parse("sequenceDiagram\n A<<->>B: both ways")),
            vec!["A", "B"]
        );
    }

    #[test]
    fn the_gap_between_two_columns_fits_the_longest_message_across_it() {
        let rows = drawn("sequenceDiagram\n A->>B: a long thing to say");
        let words = rows.iter().find(|r| r.contains("a long thing")).unwrap();
        let words = words.trim();
        // the words sit between the two lifelines, and neither is written over
        assert!(words.starts_with('│'), "{words}");
        assert!(words.ends_with('│'), "{words}");
    }

    #[test]
    fn a_message_backwards_points_its_arrow_the_other_way() {
        let rows = drawn("sequenceDiagram\n A->>B: there\n B->>A: back");
        let back = rows.iter().filter(|r| r.contains('◀')).count();
        assert_eq!(back, 1);
        let row = rows.iter().find(|r| r.contains('◀')).unwrap();
        assert!(row.trim_start().starts_with('◀'), "{row}");
        assert!(row.trim_end().ends_with('┤'), "{row}");
    }

    #[test]
    fn the_participant_boxes_are_repeated_at_the_foot() {
        let rows = drawn("sequenceDiagram\n A->>B: hi");
        assert_eq!(rows[..3], rows[rows.len() - 3..]);
    }

    #[test]
    fn lifelines_run_unbroken_from_the_header_to_the_footer() {
        let rows = drawn("sequenceDiagram\n A->>B: one\n B->>A: two\n A->>B: three");
        // "A" is five columns wide, so its lifeline runs down column 2
        for row in &rows[3..rows.len() - 3] {
            let at = row.chars().nth(2).unwrap_or(' ');
            assert!(at != ' ', "a gap in the lifeline: {row}");
        }
    }

    #[test]
    fn a_diagram_with_participants_and_no_messages_is_not_drawn() {
        let src = "sequenceDiagram\n participant A\n participant B as Bob";
        assert_eq!(parse(src).participants.len(), 2);
        assert_eq!(render(src, 80), None);
        assert_eq!(render("sequenceDiagram", 80), None);
    }

    #[test]
    fn no_row_is_wider_than_the_diagram_says_it_is() {
        let src = "sequenceDiagram\n A->>B: hi\n Note over A,B: a note that runs on\n B-->>A: ok";
        let rendered = render(src, 80).unwrap();
        for row in rendered.text() {
            assert!(str_width(&row) <= rendered.width, "{row}");
        }
        assert_eq!(
            rendered.text().iter().map(|r| str_width(r)).max(),
            Some(rendered.width)
        );
    }
}
