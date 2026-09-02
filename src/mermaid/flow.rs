//! Flowcharts: `flowchart LR`, `graph TD`, and the boxes and arrows between.
//!
//! A flowchart is a graph someone typed in reading order, and reading order is
//! most of the layout: the first time a node is mentioned is where it wants to
//! be. So the layout is layered — every node gets a rank, edges run from one
//! rank to the next, and a rank is a column in `LR`/`RL` and a row in `TD`. It
//! is not a general graph drawer and does not try to be; a diagram a note holds
//! is a dozen boxes at most, and the ones it cannot place cleanly it still
//! places honestly.
//!
//! Everything drawn goes on a [`Canvas`], which does the junctions and the
//! character widths, so this file is only ever deciding *where*.
//!
//! The *where* is worked out on a major and a minor axis rather than on x and
//! y. A rank runs along the major axis and the boxes within one are stacked
//! along the minor axis, which makes `LR` and `TD` the same arithmetic with the
//! two swapped, and `RL` the `LR` answer counted from the far end. [`Frame`] is
//! the only thing in the file that knows which axis is which.
//!
//! An edge that does not step exactly one rank forward — a skip, a back edge,
//! the return leg of a cycle — is routed around the outside rather than across
//! the middle. Every leg of such a route lies either in the gutter between two
//! ranks or in a lane beyond the layout altogether, and no box is ever in
//! either, so a detour cannot be drawn over somebody's words.

use std::collections::{HashMap, VecDeque};

use super::canvas::{Canvas, Shape, Side};
use super::{Dir, Rendered, Role};
use crate::md::str_width;

/// The narrowest gutter between two columns of an `LR` chart: a line long
/// enough to read as a line, an arrowhead, and air at both ends.
const GUTTER_LR: usize = 6;

/// The gutter between two rows of a `TD` chart. Two rows: one for the label,
/// one for the sideways jog that carries the line to the box below.
const GUTTER_TD: usize = 2;

/// Room an edge label wants either side of itself inside a gutter, so the words
/// never sit flush against the box they belong to.
const LABEL_PAD: usize = 4;

/// Blank rows between two boxes stacked in the same `LR` column.
const STACK_LR: usize = 1;

/// Blank columns between two boxes side by side in the same `TD` row.
const STACK_TD: usize = 3;

/// The shortest a label is ever wrapped to. A third of a narrow page would
/// break ordinary words apart, and a broken word is worse than a wide picture.
const MIN_LABEL: usize = 12;

/// How an edge is drawn: mermaid's three line weights, as far as one row of
/// characters can tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stroke {
    /// `-->`, `---`
    Solid,
    /// `-.->`, `-.-`
    Dotted,
    /// `==>`, `===`
    Thick,
}

/// One box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// The identifier the source refers to it by — `A` in `A[Start]`. Nodes are
    /// deduplicated on it, since mermaid lets a node be introduced once and
    /// named many times afterwards.
    pub id: String,
    /// What is drawn inside the box, already split into the lines a `<br/>`
    /// asked for. A node that never got brackets is labelled with its own id.
    pub label: Vec<String>,
    pub shape: Shape,
}

/// One arrow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Index into [`Graph::nodes`].
    pub from: usize,
    pub to: usize,
    /// The words on the arrow — `|yes|` or `-- yes -->` — if it had any.
    pub label: Option<String>,
    pub stroke: Stroke,
    /// Whether the far end carries an arrowhead: `-->` does, `---` does not.
    pub head: bool,
}

/// A parsed flowchart: nodes in the order they were first mentioned, and edges
/// in the order they were written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Draw a flowchart, or `None` when its body says nothing we could make a
/// picture of — no edges and no nodes is not a diagram, it is a typo.
pub fn render(src: &str, dir: Dir, width: usize) -> Option<Rendered> {
    let g = parse(src);
    if g.nodes.is_empty() {
        return None;
    }
    let ranks = rank(&g);
    Some(Rendered::new(draw(&g, &ranks, dir, width).rows()))
}

// ── reading the source ────────────────────────────────────────────────────

/// The statements catcher reads and then leaves out. A subgraph is a box
/// around boxes; the first pass draws what is inside it flat, which loses the
/// grouping but keeps every node and every arrow the author wrote. The rest
/// are styling, and a terminal has no styling to give them.
const IGNORED: [&str; 8] = [
    "subgraph",
    "end",
    "direction",
    "click",
    "style",
    "classdef",
    "class",
    "linkstyle",
];

/// Read the statements of a flowchart body into a [`Graph`].
///
/// Deliberately forgiving: a line it does not understand is skipped rather than
/// failing the whole diagram, because half a picture of a note's diagram is
/// worth more than none of it.
pub fn parse(src: &str) -> Graph {
    let mut g = Graph::default();
    // ids to node indices, and whether a node has had its own bracketed
    // declaration yet: the first one wins, and a bare mention never overwrites
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut declared: Vec<bool> = Vec::new();
    let mut first = true;
    for stmt in statements(src) {
        let word = stmt
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        // the header names the kind and the direction, both of which the
        // caller has already read off it
        if first {
            first = false;
            if word == "flowchart" || word == "graph" {
                continue;
            }
        }
        if IGNORED.contains(&word.as_str()) {
            continue;
        }
        chain(&stmt, &mut g, &mut index, &mut declared);
    }
    g
}

/// The body split into statements: one per line, and one per `;` within a
/// line, with `%%` comments taken off. Blank statements are dropped.
fn statements(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        for part in split_top(uncomment(line), ';') {
            let part = part.trim();
            if !part.is_empty() {
                out.push(part.to_string());
            }
        }
    }
    out
}

/// `line` with any `%%` comment removed. A `%%` inside a quoted label is part
/// of the label — someone's box is allowed to say "100%%".
fn uncomment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' => quoted = !quoted,
            b'%' if !quoted && bytes.get(i + 1) == Some(&b'%') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Split on `sep`, ignoring any that is inside brackets or quotes — a `;` or
/// an `&` in someone's label is text, not punctuation.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = (depth - 1).max(0),
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// One link as the source spells it, and where the statement carries on.
struct Link {
    stroke: Stroke,
    head: bool,
    label: Option<String>,
    next: usize,
}

/// Read one statement: a run of node groups with links between them.
///
/// `A --> B --> C` is a chain of three groups and two links, and `A & B --> C`
/// is a group of two — so the edges a statement makes are every node on one
/// side of a link paired with every node on the other.
fn chain(stmt: &str, g: &mut Graph, index: &mut HashMap<String, usize>, declared: &mut Vec<bool>) {
    let chars: Vec<char> = stmt.chars().collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let (mut i, mut start) = (0, 0);
    let mut depth = 0i32;
    let mut quoted = false;
    while i < chars.len() {
        match chars[i] {
            '"' => quoted = !quoted,
            _ if quoted => {}
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => {
                if let Some(link) = link_at(&chars, i) {
                    let text: String = chars[start..i].iter().collect();
                    groups.push(group(&text, g, index, declared));
                    let link = piped(&chars, link);
                    i = link.next;
                    start = i;
                    links.push(link);
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let text: String = chars[start..].iter().collect();
    groups.push(group(&text, g, index, declared));

    for (i, link) in links.iter().enumerate() {
        for &from in &groups[i] {
            for &to in &groups[i + 1] {
                g.edges.push(Edge {
                    from,
                    to,
                    label: link.label.clone(),
                    stroke: link.stroke,
                    head: link.head,
                });
            }
        }
    }
}

/// A link's `|yes|` label, if it wrote one after the arrow rather than inside
/// it. Both `A -->|yes| B` and `A --> |yes| B` are the same thing.
fn piped(chars: &[char], mut link: Link) -> Link {
    let mut i = link.next;
    while chars.get(i) == Some(&' ') {
        i += 1;
    }
    if chars.get(i) != Some(&'|') {
        return link;
    }
    let Some(end) = chars[i + 1..].iter().position(|&c| c == '|') else {
        return link;
    };
    link.label = label_text(&chars[i + 1..i + 1 + end].iter().collect::<String>());
    link.next = i + end + 2;
    link
}

/// The link starting at `i`, if one does.
///
/// Length carries no meaning in mermaid — `-->` and `----->` are one arrow —
/// so a link is only ever its weight, whether it ends in a head, and whether
/// it carried its words in the middle of itself.
fn link_at(c: &[char], i: usize) -> Option<Link> {
    // `<-->` is a link both ways; catcher draws the one head it can, and the
    // leading `<` is not part of the run
    let start = usize::from(c[i] == '<' && matches!(c.get(i + 1), Some('-' | '=')));
    let j = i + start;
    let ch = *c.get(j)?;
    if ch == '-' && c.get(j + 1) == Some(&'.') {
        return dotted(c, j);
    }
    if ch != '-' && ch != '=' {
        return None;
    }
    let stroke = if ch == '=' {
        Stroke::Thick
    } else {
        Stroke::Solid
    };
    let mut k = j;
    while c.get(k) == Some(&ch) {
        k += 1;
    }
    // a single dash is a hyphen in somebody's id, not an arrow
    if k - j < 2 {
        return None;
    }
    if head_at(c, k) {
        return Some(Link {
            stroke,
            head: true,
            label: None,
            next: k + 1,
        });
    }
    // `-- yes -->`: a two-character run that is not the end of the link is the
    // start of one, and the words run up to the next run of the same character
    if k - j == 2 {
        if let Some((label, end)) = closing_run(c, k, ch) {
            let head = head_at(c, end);
            return Some(Link {
                stroke,
                head,
                label: label_text(&label),
                next: end + usize::from(head),
            });
        }
    }
    Some(Link {
        stroke,
        head: false,
        label: None,
        next: k,
    })
}

/// The dotted forms, which spell themselves with dots between two dashes:
/// `-.->`, `-.-`, and `-. no .->` with the words in the middle.
fn dotted(c: &[char], j: usize) -> Option<Link> {
    let mut k = j + 1;
    while c.get(k) == Some(&'.') {
        k += 1;
    }
    if c.get(k) == Some(&'-') {
        let head = head_at(c, k + 1);
        return Some(Link {
            stroke: Stroke::Dotted,
            head,
            label: None,
            next: k + 1 + usize::from(head),
        });
    }
    let (label, end) = closing_dots(c, k)?;
    let head = head_at(c, end);
    Some(Link {
        stroke: Stroke::Dotted,
        head,
        label: label_text(&label),
        next: end + usize::from(head),
    })
}

/// Whether the character at `k` ends a link with a head. `>` always does;
/// mermaid's `x` and `o` heads only do when what follows them could not be the
/// start of a node's name, so `A---x` is a crossed arrow and `A---xyz` is not.
fn head_at(c: &[char], k: usize) -> bool {
    match c.get(k) {
        Some('>') => true,
        Some('x' | 'o') => !c.get(k + 1).copied().is_some_and(is_id),
        _ => false,
    }
}

/// The words of a `-- yes -->` link and the index just past the run that closed
/// it, or `None` when nothing closes it and the run was a plain link after all.
fn closing_run(c: &[char], from: usize, ch: char) -> Option<(String, usize)> {
    let mut k = from;
    while k < c.len() {
        if c[k] == ch && c.get(k + 1) == Some(&ch) {
            let mut end = k;
            while c.get(end) == Some(&ch) {
                end += 1;
            }
            return Some((c[from..k].iter().collect(), end));
        }
        k += 1;
    }
    None
}

/// The same, for `-. no .->`: the words end where the closing dots begin.
fn closing_dots(c: &[char], from: usize) -> Option<(String, usize)> {
    let mut k = from;
    while k < c.len() {
        if c[k] == '.' {
            let mut end = k;
            while c.get(end) == Some(&'.') {
                end += 1;
            }
            if c.get(end) == Some(&'-') {
                return Some((c[from..k].iter().collect(), end + 1));
            }
        }
        k += 1;
    }
    None
}

/// One side of a link: the nodes an `&` list names, each of them registered.
fn group(
    text: &str,
    g: &mut Graph,
    index: &mut HashMap<String, usize>,
    declared: &mut Vec<bool>,
) -> Vec<usize> {
    split_top(text, '&')
        .into_iter()
        .filter_map(spec)
        .map(|s| touch(g, index, declared, s))
        .collect()
}

/// A node as one mention of it spells it.
struct Spec {
    id: String,
    /// The lines of its label, when this mention brought brackets.
    label: Option<Vec<String>>,
    shape: Shape,
}

/// Read one node mention: an id, and the bracket that gives it a shape.
fn spec(s: &str) -> Option<Spec> {
    let s = s.trim();
    let end = s.find(|c: char| !is_id(c)).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let id = s[..end].to_string();
    // whatever follows the brackets is somebody else's business: `:::class`
    // names a style, and an edge id is a token of its own
    let rest = s[end..].trim_start();
    let Some((open, close, shape)) = bracket(rest) else {
        return Some(Spec {
            id,
            label: None,
            shape: Shape::Rect,
        });
    };
    let body = &rest[open.len()..];
    let inner = match find_close(body, open, close) {
        Some(at) => &body[..at],
        // an unclosed bracket is a diagram someone is still typing; take what
        // is there rather than dropping the node
        None => body,
    };
    let label = lines_of(inner);
    Some(Spec {
        id,
        label: Some(label),
        shape,
    })
}

/// The bracket `rest` opens with, as `(open, close, shape)`.
///
/// Mermaid can spell a dozen shapes and a terminal can draw four, so the rest
/// are mapped onto the nearest: a stadium and a cylinder are round, a
/// subroutine and an asymmetric box are rectangles, a hexagon is a decision.
/// Longest spellings first — `((` has to be tried before `(`.
fn bracket(rest: &str) -> Option<(&'static str, &'static str, Shape)> {
    const FORMS: [(&str, &str, Shape); 13] = [
        ("((", "))", Shape::Circle),
        // parallelograms and trapezoids: a box, since a slant is a diamond's
        // to draw and the label is what matters
        ("[/", "/]", Shape::Rect),
        ("[\\", "\\]", Shape::Rect),
        ("[/", "\\]", Shape::Rect),
        ("[\\", "/]", Shape::Rect),
        ("([", "])", Shape::Round),
        ("[[", "]]", Shape::Rect),
        ("[(", ")]", Shape::Round),
        ("{{", "}}", Shape::Diamond),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Round),
        ("{", "}", Shape::Diamond),
        (">", "]", Shape::Rect),
    ];
    FORMS
        .iter()
        .find(|(open, ..)| rest.starts_with(open))
        .map(|&(open, close, shape)| (open, close, shape))
}

/// Where the bracket opened at the start of `body` closes: the first `close`
/// outside quotes that is not closing a nested pair. A quoted label is how
/// mermaid lets a box say `a] b`, so quotes win over brackets.
fn find_close(body: &str, open: &str, close: &str) -> Option<usize> {
    let mut quoted = false;
    let mut depth = 0usize;
    let mut i = 0;
    while i < body.len() {
        if !body.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &body[i..];
        if rest.starts_with('"') {
            quoted = !quoted;
            i += 1;
        } else if quoted {
            i += 1;
        } else if rest.starts_with(close) {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
            i += close.len();
        } else if rest.starts_with(open) {
            depth += 1;
            i += open.len();
        } else {
            i += 1;
        }
    }
    None
}

/// A label's text split into the lines it asked for. `<br>`, `<br/>` and
/// `<br />` all break a line; surrounding quotes are the author telling the
/// parser where the label ends, not something to draw.
fn lines_of(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = inner;
    loop {
        match find_break(rest) {
            Some((at, end)) => {
                out.push(unquote(&rest[..at]));
                rest = &rest[end..];
            }
            None => {
                out.push(unquote(rest));
                break;
            }
        }
    }
    out.retain(|l| !l.is_empty());
    out
}

/// The next `<br…>` in `s`, as the byte range it occupies.
fn find_break(s: &str) -> Option<(usize, usize)> {
    let lower = s.to_ascii_lowercase();
    let at = lower.find("<br")?;
    let end = at + lower[at..].find('>')? + 1;
    let between = &lower[at + 3..end - 1];
    between
        .chars()
        .all(|c| c.is_whitespace() || c == '/')
        .then_some((at, end))
}

/// `s` trimmed, with the quotes mermaid wraps an awkward label in taken off.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    inner.trim().to_string()
}

/// An edge's words, or `None` when it turned out to have none.
fn label_text(s: &str) -> Option<String> {
    let s = unquote(s);
    (!s.is_empty()).then_some(s)
}

/// Whether `c` can be part of a node's identifier.
fn is_id(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Register one mention of a node and answer its index.
///
/// A node keeps the first label it is given: mermaid lets `A[Start]` be
/// written once and `A` referred to a dozen times afterwards, and every one of
/// those bare mentions would otherwise rename the box to `A`. A bare mention
/// that comes *first* is upgraded when the declaration finally arrives, so a
/// node may be named before it is declared.
fn touch(
    g: &mut Graph,
    index: &mut HashMap<String, usize>,
    declared: &mut Vec<bool>,
    spec: Spec,
) -> usize {
    let Spec { id, label, shape } = spec;
    let label = label.filter(|l| !l.is_empty());
    if let Some(&i) = index.get(&id) {
        if let Some(label) = label {
            if !declared[i] {
                g.nodes[i].label = label;
                g.nodes[i].shape = shape;
                declared[i] = true;
            }
        }
        return i;
    }
    let i = g.nodes.len();
    index.insert(id.clone(), i);
    declared.push(label.is_some());
    g.nodes.push(Node {
        label: label.unwrap_or_else(|| vec![id.clone()]),
        id,
        shape,
    });
    i
}

// ── ranking ───────────────────────────────────────────────────────────────

/// The rank of every node, by index — longest path from a root, so an edge
/// always points from a lower rank to a higher one and the picture reads the
/// way the arrows do.
///
/// Back edges are taken out first, or a cycle would have no roots and no
/// ranking at all; they are drawn afterwards, around the outside.
pub fn rank(g: &Graph) -> Vec<usize> {
    let n = g.nodes.len();
    let back = back_edges(g);
    let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut incoming = vec![0usize; n];
    for (i, e) in g.edges.iter().enumerate() {
        if back[i] {
            continue;
        }
        adjacent[e.from].push(e.to);
        incoming[e.to] += 1;
    }
    let mut rank = vec![0usize; n];
    let mut queued = vec![false; n];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for i in 0..n {
        if incoming[i] == 0 {
            queued[i] = true;
            queue.push_back(i);
        }
    }
    let mut done = 0;
    while done < n {
        let Some(v) = queue.pop_front() else {
            // nothing is a root: the graph is one long cycle, and the node the
            // author mentioned first is the one they meant to start at
            let Some(v) = (0..n).find(|&i| !queued[i]) else {
                break;
            };
            queued[v] = true;
            queue.push_back(v);
            continue;
        };
        done += 1;
        for &t in &adjacent[v] {
            rank[t] = rank[t].max(rank[v] + 1);
            incoming[t] -= 1;
            if incoming[t] == 0 && !queued[t] {
                queued[t] = true;
                queue.push_back(t);
            }
        }
    }
    rank
}

/// Which edges point back into the path they came down, by index. A depth
/// first walk in mention order: an edge whose target is still on the stack
/// closes a loop, and taking those out is what leaves a graph that can be
/// ranked at all.
fn back_edges(g: &Graph) -> Vec<bool> {
    const UNSEEN: u8 = 0;
    const OPEN: u8 = 1;
    const CLOSED: u8 = 2;

    let n = g.nodes.len();
    let mut out = vec![false; g.edges.len()];
    let mut leaving: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, e) in g.edges.iter().enumerate() {
        leaving[e.from].push(i);
    }
    let mut state = vec![UNSEEN; n];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for root in 0..n {
        if state[root] != UNSEEN {
            continue;
        }
        state[root] = OPEN;
        stack.push((root, 0));
        while let Some((v, i)) = stack.pop() {
            let Some(&edge) = leaving[v].get(i) else {
                state[v] = CLOSED;
                continue;
            };
            stack.push((v, i + 1));
            let t = g.edges[edge].to;
            match state[t] {
                OPEN => out[edge] = true,
                UNSEEN => {
                    state[t] = OPEN;
                    stack.push((t, 0));
                }
                _ => {}
            }
        }
    }
    out
}

// ── laying it out ─────────────────────────────────────────────────────────

/// Which way the two axes of the layout point once they reach the canvas.
///
/// `span` is how far the whole picture reaches along the major axis, which is
/// what `RL` needs: right to left is left to right measured from the other end,
/// so nothing above this struct has to think about it twice.
#[derive(Clone, Copy)]
struct Frame {
    dir: Dir,
    span: usize,
}

impl Frame {
    fn td(&self) -> bool {
        self.dir == Dir::Td
    }

    /// Where a run of `len` starting at `major` begins, in canvas terms.
    fn at(&self, major: usize, len: usize) -> usize {
        match self.dir {
            Dir::Rl => self.span.saturating_sub(major + len),
            _ => major,
        }
    }

    fn point(&self, major: usize, minor: usize) -> (usize, usize) {
        match self.dir {
            Dir::Td => (minor, major),
            _ => (self.at(major, 1), minor),
        }
    }

    fn rect(
        &self,
        major: usize,
        minor: usize,
        ms: usize,
        mn: usize,
    ) -> (usize, usize, usize, usize) {
        match self.dir {
            Dir::Td => (minor, major, mn, ms),
            _ => (self.at(major, ms), minor, ms, mn),
        }
    }

    /// A run along the major axis: across the page in `LR`, down it in `TD`.
    fn along(&self, c: &mut Canvas, major: usize, minor: usize, len: usize, stroke: Stroke) {
        match self.dir {
            Dir::Td => line(c, minor, major, len, false, stroke),
            _ => line(c, self.at(major, len), minor, len, true, stroke),
        }
    }

    /// A run across the minor axis: the jog that carries a line from one row
    /// to another in `LR`, and from one column to another in `TD`.
    fn across(&self, c: &mut Canvas, major: usize, minor: usize, len: usize, stroke: Stroke) {
        match self.dir {
            Dir::Td => line(c, minor, major, len, true, stroke),
            _ => line(c, self.at(major, 1), minor, len, false, stroke),
        }
    }

    fn arrow(&self, c: &mut Canvas, major: usize, minor: usize) {
        let side = match self.dir {
            Dir::Lr => Side::Right,
            Dir::Rl => Side::Left,
            Dir::Td => Side::Down,
        };
        let (x, y) = self.point(major, minor);
        c.arrow(x, y, side, Role::Line);
    }

    /// Words centred over the major range `lo..=hi`, at one place on the minor
    /// axis. This is the `LR` way round: the run under the label is horizontal,
    /// so the label lies along it.
    fn label_along(&self, c: &mut Canvas, lo: usize, hi: usize, minor: usize, text: &str) {
        let len = hi + 1 - lo;
        let pad = len.saturating_sub(str_width(text)) / 2;
        c.text(self.at(lo, len) + pad, minor, text, Role::Label);
    }

    /// Words centred over the minor range `lo..=hi`, at one place on the major
    /// axis — the `TD` way round, where the horizontal run is the jog.
    fn label_across(&self, c: &mut Canvas, major: usize, lo: usize, hi: usize, text: &str) {
        let len = hi + 1 - lo;
        let pad = len.saturating_sub(str_width(text)) / 2;
        c.text(lo + pad, major, text, Role::Label);
    }
}

/// Draw one straight leg of an edge, in the weight the arrow asked for.
///
/// The leg is drawn solid first, because [`Canvas::hline`] is what grows the
/// corners and the crossings; only the plain cells between its ends are then
/// restyled. So a dotted line still joins the box it leaves, and a thick one
/// still turns a corner and still crosses whatever it crosses.
fn line(c: &mut Canvas, x: usize, y: usize, len: usize, horizontal: bool, stroke: Stroke) {
    if len == 0 {
        return;
    }
    if horizontal {
        c.hline(x, y, len, Role::Line);
    } else {
        c.vline(x, y, len, Role::Line);
    }
    if stroke == Stroke::Solid {
        return;
    }
    let (plain, thick) = if horizontal {
        ('─', '━')
    } else {
        ('│', '┃')
    };
    for i in 0..len {
        let (x, y) = if horizontal { (x + i, y) } else { (x, y + i) };
        if c.get(x, y) != plain {
            continue;
        }
        // the dash phase follows the canvas column, not the run, so a route
        // drawn as several runs still reads as one evenly dotted line
        let odd = (if horizontal { x } else { y }) % 2 == 1;
        match stroke {
            // a gap every other cell is what says "dotted"; leaving the
            // junctions alone is what keeps the picture joined up
            Stroke::Dotted if odd => c.set(x, y, ' ', Role::Line),
            Stroke::Thick => c.set(x, y, thick, Role::Line),
            _ => {}
        }
    }
}

/// One box, placed. Sizes are in major/minor terms, so `ms` is a width in
/// `LR` and a height in `TD`.
struct Place {
    major: usize,
    minor: usize,
    ms: usize,
    mn: usize,
    label: Vec<String>,
    shape: Shape,
}

/// Everywhere everything goes.
struct Plan {
    frame: Frame,
    ranks: Vec<usize>,
    places: Vec<Place>,
    /// The gutter before each rank, as a major range. The one before rank 0 is
    /// a margin, and is only given room when a back edge has to land there.
    gaps: Vec<(usize, usize)>,
    /// Where the ranks themselves start and end on the minor axis. Outside
    /// them, at either end, are the lanes the detours run in.
    from: usize,
    to: usize,
}

/// Word-wrap a label so a long name deepens the diagram rather than pushing
/// the page sideways. A word longer than the limit is left alone: breaking one
/// mid-way costs more than the column it saves.
fn wrap(label: &[String], width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in label {
        if str_width(line) <= width {
            out.push(line.clone());
            continue;
        }
        let mut row = String::new();
        for word in line.split_whitespace() {
            if row.is_empty() {
                row = word.to_string();
            } else if str_width(&row) + 1 + str_width(word) <= width {
                row.push(' ');
                row.push_str(word);
            } else {
                out.push(std::mem::take(&mut row));
                row = word.to_string();
            }
        }
        if !row.is_empty() {
            out.push(row);
        }
    }
    out
}

/// Work out where every box and every lane goes.
fn plan(g: &Graph, ranks: &[usize], dir: Dir, width: usize) -> Plan {
    let td = dir == Dir::Td;
    let gutter = if td { GUTTER_TD } else { GUTTER_LR };
    let stack = if td { STACK_TD } else { STACK_LR };

    let labels: Vec<Vec<String>> = g
        .nodes
        .iter()
        .map(|n| wrap(&n.label, (width / 3).max(MIN_LABEL)))
        .collect();
    let sizes: Vec<(usize, usize)> = g
        .nodes
        .iter()
        .zip(&labels)
        .map(|(n, label)| Canvas::node_size(n.shape, label))
        .collect();
    let major: Vec<usize> = sizes.iter().map(|&(w, h)| if td { h } else { w }).collect();
    let minor: Vec<usize> = sizes.iter().map(|&(w, h)| if td { w } else { h }).collect();

    let count = ranks.iter().max().map_or(1, |r| r + 1);
    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (i, &r) in ranks.iter().enumerate() {
        by_rank[r].push(i);
    }

    // a gutter is as wide as the widest label that has to sit in it. In `TD`
    // it is rows rather than columns, and a label is one row whatever it says,
    // so there the minimum is already the answer.
    let mut gaps = vec![gutter; count];
    gaps[0] = if g.edges.iter().any(|e| ranks[e.to] == 0) {
        gutter
    } else {
        0
    };
    if !td {
        for e in &g.edges {
            let (from, to) = (ranks[e.from], ranks[e.to]);
            match &e.label {
                Some(label) if to == from + 1 => {
                    gaps[to] = gaps[to].max(str_width(label) + LABEL_PAD);
                }
                _ => {}
            }
        }
    }

    // along the major axis: rank after rank, each as deep as its deepest box
    let depth: Vec<usize> = by_rank
        .iter()
        .map(|rank| rank.iter().map(|&i| major[i]).max().unwrap_or(0))
        .collect();
    let mut starts = Vec::with_capacity(count);
    let mut at = 0;
    for r in 0..count {
        at += gaps[r];
        starts.push(at);
        at += depth[r];
    }
    let span = at;
    let gaps: Vec<(usize, usize)> = (0..count)
        .map(|r| (starts[r] - gaps[r], starts[r]))
        .collect();

    // across the minor axis: every rank centred against the deepest one, with
    // room above it for the lane each back edge needs
    let widths: Vec<usize> = by_rank
        .iter()
        .map(|rank| {
            rank.iter().map(|&i| minor[i]).sum::<usize>() + stack * rank.len().saturating_sub(1)
        })
        .collect();
    let widest = widths.iter().copied().max().unwrap_or(0);
    let from = g
        .edges
        .iter()
        .filter(|e| ranks[e.to] <= ranks[e.from])
        .count();

    let mut places: Vec<Place> = Vec::with_capacity(g.nodes.len());
    for (i, node) in g.nodes.iter().enumerate() {
        places.push(Place {
            major: starts[ranks[i]],
            minor: 0,
            ms: depth[ranks[i]],
            mn: minor[i],
            // a box given more depth than it asked for, so its rank lines up,
            // holds its words in the middle of it rather than at the top —
            // which along the major axis is only something `TD` can mean
            label: pad(&labels[i], td, depth[ranks[i]] - major[i]),
            shape: node.shape,
        });
    }
    for (r, rank) in by_rank.iter().enumerate() {
        let mut at = from + (widest - widths[r]) / 2;
        for &i in rank {
            places[i].minor = at;
            at += places[i].mn + stack;
        }
    }

    Plan {
        frame: Frame { dir, span },
        ranks: ranks.to_vec(),
        places,
        gaps,
        from,
        to: from + widest,
    }
}

/// A label with blank lines above it, so a box stretched to its rank's depth
/// still reads as centred.
fn pad(label: &[String], td: bool, room: usize) -> Vec<String> {
    if !td || room == 0 {
        return label.to_vec();
    }
    let mut out = vec![String::new(); room / 2];
    out.extend_from_slice(label);
    out
}

impl Plan {
    /// The middle of the gutter before rank `r` — where a line turns.
    fn mid(&self, r: usize) -> usize {
        let (from, to) = self.gaps[r];
        (from + to) / 2
    }

    /// Where an edge leaves a box and where it arrives, on the minor axis.
    fn ends(&self, e: &Edge) -> (usize, usize) {
        let (s, t) = (&self.places[e.from], &self.places[e.to]);
        (s.minor + s.mn / 2, t.minor + t.mn / 2)
    }

    /// The last leg of every route: in through the gutter before the target,
    /// stopping a cell short of the box so the arrowhead has somewhere to sit.
    fn arrive(&self, c: &mut Canvas, e: &Edge, from: usize, minor: usize) {
        let edge = self.places[e.to].major;
        // an edge with no head runs onto the box's own border, where `put`
        // turns it into a tee; one with a head stops a cell short and the head
        // is what touches
        let end = if e.head { edge - 1 } else { edge + 1 };
        self.frame
            .along(c, from, minor, end.saturating_sub(from), e.stroke);
        if e.head {
            self.frame.arrow(c, edge - 1, minor);
        }
    }

    /// An edge between neighbouring ranks: out of one box, a jog across the
    /// gutter to line up with the other, and in.
    fn step(&self, c: &mut Canvas, e: &Edge) {
        let (s, t) = self.ends(e);
        let source = &self.places[e.from];
        let out = source.major + source.ms;
        let mid = self.mid(self.ranks[e.to]);
        self.frame
            .along(c, out, s, (mid + 1).saturating_sub(out), e.stroke);
        // a jog of one cell would be drawn as a crossing rather than as
        // nothing, so a straight line is left straight
        if s != t {
            self.frame
                .across(c, mid, s.min(t), s.abs_diff(t) + 1, e.stroke);
        }
        self.arrive(c, e, mid, t);
        if let Some(label) = &e.label {
            let (from, to) = self.gaps[self.ranks[e.to]];
            if self.frame.td() {
                self.frame.label_across(c, from, s.min(t), s.max(t), label);
            } else {
                self.frame.label_along(c, from, to - 1, t, label);
            }
        }
    }

    /// An edge that does not step one rank forward: a skip over a rank, or a
    /// back edge closing a loop. It goes out into a lane of its own beyond the
    /// layout, along it, and back in — every leg of it in a gutter or a lane,
    /// which is to say never over a box.
    fn detour(&self, c: &mut Canvas, e: &Edge, lane: usize, back: bool) {
        let (s, t) = self.ends(e);
        let source = &self.places[e.from];
        let (a, b) = if back {
            (self.mid(self.ranks[e.from]), self.mid(self.ranks[e.to]))
        } else {
            (self.mid(self.ranks[e.from] + 1), self.mid(self.ranks[e.to]))
        };
        if back {
            let edge = source.major;
            self.frame.along(c, a, s, edge.saturating_sub(a), e.stroke);
        } else {
            let out = source.major + source.ms;
            self.frame
                .along(c, out, s, (a + 1).saturating_sub(out), e.stroke);
        }
        self.frame
            .across(c, a, lane.min(s), lane.abs_diff(s) + 1, e.stroke);
        self.frame
            .along(c, a.min(b), lane, a.abs_diff(b) + 1, e.stroke);
        self.frame
            .across(c, b, lane.min(t), lane.abs_diff(t) + 1, e.stroke);
        self.arrive(c, e, b, t);
        if let Some(label) = &e.label {
            // the words go on whichever leg of the route runs across the page:
            // the lane itself in `LR`, and the leg that comes back in in `TD`
            if self.frame.td() {
                self.frame
                    .label_across(c, b, lane.min(t), lane.max(t), label);
            } else {
                self.frame.label_along(c, a.min(b), a.max(b), lane, label);
            }
        }
    }
}

/// Place the ranked graph on a canvas, running `dir`, trying to fit `width`.
fn draw(g: &Graph, ranks: &[usize], dir: Dir, width: usize) -> Canvas {
    let plan = plan(g, ranks, dir, width);
    let skips = g
        .edges
        .iter()
        .filter(|e| ranks[e.to] > ranks[e.from] + 1)
        .count();
    let (_, _, w, h) = plan.frame.rect(0, 0, plan.frame.span, plan.to + skips);
    let mut c = Canvas::new(w.max(1), h.max(1));

    for p in &plan.places {
        let (x, y, w, h) = plan.frame.rect(p.major, p.minor, p.ms, p.mn);
        c.node(x, y, w, h, p.shape, &p.label);
    }
    // one lane per detour, back edges above the layout and skips below it, in
    // the order they were written — the first one written stays nearest
    let (mut back, mut skip) = (0, 0);
    for e in &g.edges {
        let (s, t) = (ranks[e.from], ranks[e.to]);
        if t == s + 1 {
            plan.step(&mut c, e);
        } else if t > s {
            plan.detour(&mut c, e, plan.to + skip, false);
            skip += 1;
        } else {
            plan.detour(&mut c, e, plan.from - 1 - back, true);
            back += 1;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `render` drew, as plain rows.
    fn drawn(src: &str, dir: Dir) -> Vec<String> {
        render(src, dir, 80).expect("a diagram").text()
    }

    /// The row a piece of text was drawn on.
    fn row_of(rows: &[String], text: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("{text:?} is not in {rows:#?}"))
    }

    #[test]
    fn a_bare_arrow_makes_two_boxes_and_a_line() {
        assert_eq!(
            drawn("flowchart LR\nA --> B", Dir::Lr),
            vec!["╭───╮      ╭───╮", "│ A │─────▶│ B │", "╰───╯      ╰───╯",]
        );
    }

    #[test]
    fn node_brackets_pick_the_shape_and_the_label() {
        let g = parse("flowchart LR\nA[Start] --> B(Go) --> C{Ok} --> D((End))");
        let shapes: Vec<Shape> = g.nodes.iter().map(|n| n.shape).collect();
        assert_eq!(
            shapes,
            vec![Shape::Rect, Shape::Round, Shape::Diamond, Shape::Circle]
        );
        let labels: Vec<&str> = g.nodes.iter().map(|n| n.label[0].as_str()).collect();
        assert_eq!(labels, vec!["Start", "Go", "Ok", "End"]);
        // the spellings a terminal cannot honour land on the nearest shape it can
        let g = parse("flowchart LR\nA([a]) --> B[[b]] --> C[(c)] --> D{{d}} --> E>e]");
        let shapes: Vec<Shape> = g.nodes.iter().map(|n| n.shape).collect();
        assert_eq!(
            shapes,
            vec![
                Shape::Round,
                Shape::Rect,
                Shape::Round,
                Shape::Diamond,
                Shape::Rect
            ]
        );
        // a quoted label may hold the bracket that would otherwise close it
        let g = parse("flowchart LR\nA[\"a] b\"]");
        assert_eq!(g.nodes[0].label, vec!["a] b".to_string()]);
    }

    #[test]
    fn a_node_declared_once_keeps_its_label_when_named_again() {
        let g = parse("flowchart LR\nA[Start] --> B\nA --> C");
        assert_eq!(g.nodes[0].label, vec!["Start".to_string()]);
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn a_node_named_before_it_is_declared_still_gets_its_label() {
        let g = parse("flowchart LR\nA --> B\nB[Finish]");
        assert_eq!(g.nodes[1].label, vec!["Finish".to_string()]);
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn a_chain_becomes_one_edge_per_arrow() {
        let g = parse("flowchart LR\nA --> B --> C");
        assert_eq!(g.nodes.len(), 3);
        let pairs: Vec<(usize, usize)> = g.edges.iter().map(|e| (e.from, e.to)).collect();
        assert_eq!(pairs, vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn an_ampersand_list_fans_out_to_every_pair() {
        let g = parse("flowchart LR\nA[a] & B --> C & D");
        let pairs: Vec<(usize, usize)> = g.edges.iter().map(|e| (e.from, e.to)).collect();
        assert_eq!(pairs, vec![(0, 2), (0, 3), (1, 2), (1, 3)]);
        assert_eq!(g.nodes[0].label, vec!["a".to_string()]);
    }

    #[test]
    fn an_edge_label_is_drawn_on_the_line_between_the_boxes() {
        assert_eq!(
            drawn("flowchart LR\nA -->|yes| B", Dir::Lr)[1],
            "│ A │──yes─▶│ B │"
        );
        // and the other spelling of the same thing means the same thing
        let g = parse("flowchart LR\nA -- yes --> B");
        assert_eq!(g.edges[0].label.as_deref(), Some("yes"));
        assert!(g.edges[0].head);
    }

    #[test]
    fn a_dotted_and_a_thick_arrow_are_told_apart() {
        let g = parse("flowchart LR\nA -.-> B\nC ==> D\nE -. no .-> F\nG == go ==> H");
        let strokes: Vec<Stroke> = g.edges.iter().map(|e| e.stroke).collect();
        assert_eq!(
            strokes,
            vec![Stroke::Dotted, Stroke::Thick, Stroke::Dotted, Stroke::Thick]
        );
        assert_eq!(g.edges[2].label.as_deref(), Some("no"));
        assert_eq!(g.edges[3].label.as_deref(), Some("go"));
        // length says nothing: a long arrow is the same arrow
        let g = parse("flowchart LR\nA -....-> B\nC =====> D\nE ----> F");
        let strokes: Vec<Stroke> = g.edges.iter().map(|e| e.stroke).collect();
        assert_eq!(strokes, vec![Stroke::Dotted, Stroke::Thick, Stroke::Solid]);

        // and the three are drawn as three different lines
        let solid = drawn("flowchart LR\nA --> B", Dir::Lr)[1].clone();
        let dotted = drawn("flowchart LR\nA -.-> B", Dir::Lr)[1].clone();
        let thick = drawn("flowchart LR\nA ==> B", Dir::Lr)[1].clone();
        assert_ne!(solid, dotted);
        assert_ne!(solid, thick);
        assert_ne!(dotted, thick);
        assert!(thick.contains('━'));
    }

    #[test]
    fn an_arrow_without_a_head_draws_no_arrowhead() {
        let g = parse("flowchart LR\nA --- B");
        assert!(!g.edges[0].head);
        let rows = drawn("flowchart LR\nA --- B", Dir::Lr);
        assert!(!rows.iter().any(|r| r.contains('▶')));
        assert_eq!(rows[1], "│ A │──────┤ B │");
    }

    #[test]
    fn ranks_are_the_longest_path_from_a_root() {
        // C could be one step from A, but it is two from B, and the longest
        // path is the one that keeps every arrow pointing forward
        let g = parse("flowchart LR\nA --> B\nB --> C\nA --> C");
        assert_eq!(rank(&g), vec![0, 1, 2]);
        // anything nothing points at starts at the beginning — and the ranks
        // come back in mention order, which here is A, C, B
        let g = parse("flowchart LR\nA --> C\nB --> C");
        assert_eq!(rank(&g), vec![0, 1, 0]);
    }

    #[test]
    fn a_cycle_still_ranks_every_node() {
        let g = parse("flowchart LR\nA --> B\nB --> A");
        assert_eq!(rank(&g), vec![0, 1]);
        // a loop with no way in at all still starts where the author started
        let g = parse("flowchart LR\nA --> B --> C --> A");
        assert_eq!(rank(&g), vec![0, 1, 2]);
    }

    #[test]
    fn a_back_edge_is_routed_around_the_layout_rather_than_through_it() {
        let rows = drawn("flowchart LR\nA --> B --> C --> A", Dir::Lr);
        // the boxes are all still whole, and the return leg is on a row of its
        // own above them rather than drawn across the middle
        assert!(rows[0].contains('─'));
        for name in ["│ A │", "│ B │", "│ C │"] {
            assert!(
                rows.iter().any(|r| r.contains(name)),
                "{name} is missing from {rows:#?}"
            );
        }
        let boxes = row_of(&rows, "│ A │");
        assert!(boxes > 0, "the lane should be above the boxes");
        assert!(rows[boxes].contains('▶'));
    }

    #[test]
    fn left_to_right_makes_ranks_columns_and_top_down_makes_them_rows() {
        let rows = drawn("flowchart LR\nA --> B", Dir::Lr);
        assert_eq!(row_of(&rows, "A"), row_of(&rows, "B"));
        let rows = drawn("graph TD\nA --> B", Dir::Td);
        assert!(row_of(&rows, "A") < row_of(&rows, "B"));
        assert_eq!(
            rows,
            vec![
                "╭───╮",
                "│ A │",
                "╰───╯",
                "  │",
                "  ▼",
                "╭───╮",
                "│ B │",
                "╰───╯",
            ]
        );
    }

    #[test]
    fn right_to_left_puts_the_first_rank_on_the_right() {
        assert_eq!(
            drawn("flowchart RL\nA --> B", Dir::Rl)[1],
            "│ B │◀─────│ A │"
        );
    }

    #[test]
    fn a_subgraph_is_ignored_and_its_nodes_are_still_drawn() {
        let g =
            parse("flowchart TD\nsubgraph one [Group]\n  direction LR\n  A --> B\nend\nB --> C");
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C"]);
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn comments_and_style_lines_are_skipped() {
        let g = parse(
            "flowchart LR\n\
             %% the whole line\n\
             A --> B %% and the tail of one\n\
             style A fill:#f00\n\
             classDef big font-size:20px\n\
             class A big\n\
             click A \"https://example.com\"\n\
             linkStyle 0 stroke:#333\n\
             C:::big --> D",
        );
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "B", "C", "D"]);
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn a_br_tag_splits_a_label_over_two_lines() {
        let g = parse("flowchart LR\nA[read<br/>the file] --> B[one<br />two<BR>three]");
        assert_eq!(
            g.nodes[0].label,
            vec!["read".to_string(), "the file".to_string()]
        );
        assert_eq!(g.nodes[1].label.len(), 3);
        let rows = drawn("flowchart LR\nA[read<br/>the file]", Dir::Lr);
        assert_eq!(
            rows,
            vec![
                "╭──────────╮",
                "│   read   │",
                "│ the file │",
                "╰──────────╯",
            ]
        );
    }

    #[test]
    fn a_long_label_is_wrapped_rather_than_pushing_the_page_sideways() {
        let src = "flowchart LR\nA[the quick brown fox jumps over the lazy dog] --> B";
        let wide = render(src, Dir::Lr, 80).expect("a diagram");
        assert!(wide.width <= 60, "{} columns is too many", wide.width);
        assert!(wide.height() > 3, "a wrapped label makes a taller box");
        // a narrower page wraps harder, and never below a readable width
        let narrow = render(src, Dir::Lr, 20).expect("a diagram");
        assert!(narrow.height() > wide.height());
        assert!(narrow.width < wide.width);
    }

    #[test]
    fn a_flowchart_with_nothing_in_it_is_not_drawn() {
        assert_eq!(render("flowchart LR", Dir::Lr, 80), None);
        assert_eq!(render("graph TD\n%% nothing yet\n\n", Dir::Td, 80), None);
        assert_eq!(render("flowchart LR\nstyle A fill:#f00", Dir::Lr, 80), None);
    }

    /// A diagram with everything in it at once: a fork, a join, a skip over a
    /// rank, a back edge, labels, and more boxes than a rank can hold.
    pub(super) const CROWDED: &str = "flowchart TD\n\
        Start[Start here] --> Check{ok?}\n\
        Check -->|yes| Work[Do the work]\n\
        Check -->|no| Fix[Fix it]\n\
        Fix --> Work\n\
        Work --> Log & Notify\n\
        Log --> Done((Done))\n\
        Notify --> Done\n\
        Start --> Done\n\
        Done --> Check\n\
        Work -.-> Audit\n\
        Audit ==> Done";

    #[test]
    fn a_crowded_diagram_never_puts_one_box_on_another() {
        for dir in [Dir::Lr, Dir::Rl, Dir::Td] {
            let g = parse(CROWDED);
            let ranks = rank(&g);
            let plan = plan(&g, &ranks, dir, 80);
            for (i, a) in plan.places.iter().enumerate() {
                for b in plan.places.iter().skip(i + 1) {
                    let apart = a.major + a.ms <= b.major
                        || b.major + b.ms <= a.major
                        || a.minor + a.mn <= b.minor
                        || b.minor + b.mn <= a.minor;
                    assert!(apart, "two boxes share a cell in {dir:?}");
                }
            }
            // and every box's words survive the arrows drawn around them
            let rows = drawn(CROWDED, dir);
            for node in &g.nodes {
                for line in &node.label {
                    assert!(
                        rows.iter().any(|r| r.contains(line.as_str())),
                        "{line:?} was drawn over in {dir:?}: {rows:#?}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_row_is_wider_than_the_diagram_says_it_is() {
        for dir in [Dir::Lr, Dir::Rl, Dir::Td] {
            let rendered = render(CROWDED, dir, 80).expect("a diagram");
            for row in rendered.text() {
                assert!(str_width(&row) <= rendered.width);
            }
        }
    }
}

#[cfg(test)]
mod eyeball {
    use super::*;
    #[test]
    fn look() {
        for src in [
            "flowchart LR\nA[Start] --> B{ok?}\nB -->|yes| C[Ship it]\nB -->|no| D[Fix it]\nD --> B",
            super::tests::CROWDED,
            "graph TD\nA[Start] --> B{ok?}\nB -->|yes| C[Ship it]\nB -->|no| D[Fix it]\nD --> B",
            "flowchart LR\nA --> B --> C --> D\nA --> D",
            "graph TD\nA --> B --> C --> D\nA --> D",
        ] {
            for dir in [Dir::Lr, Dir::Td] {
                if (dir == Dir::Td) != src.starts_with("graph") { continue; }
                println!("──── {dir:?} ────");
                for row in render(src, dir, 80).unwrap().text() { println!("{row}"); }
            }
        }
    }
}
