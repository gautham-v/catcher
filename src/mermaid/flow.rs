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
/// Mermaid can spell a dozen shapes and a terminal can draw five, so the rest
/// are mapped onto the nearest: a stadium is round, a subroutine and an
/// asymmetric box are rectangles, a hexagon is a decision.
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
        ("[(", ")]", Shape::Cylinder),
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

    /// The first cell of a run along the major axis: only the edge it leaves
    /// by, so it joins whatever it turns off without reaching back.
    fn stub(&self, c: &mut Canvas, major: usize, minor: usize) {
        let side = match self.dir {
            Dir::Lr => Side::Right,
            Dir::Rl => Side::Left,
            Dir::Td => Side::Down,
        };
        let (x, y) = self.point(major, minor);
        c.stub(x, y, side, Role::Line);
    }

    /// The last cell of a run along the major axis: only the edge it came
    /// by, so it turns a corner instead of reaching on into nothing.
    fn stub_back(&self, c: &mut Canvas, major: usize, minor: usize) {
        let side = match self.dir {
            Dir::Lr => Side::Left,
            Dir::Rl => Side::Right,
            Dir::Td => Side::Up,
        };
        let (x, y) = self.point(major, minor);
        c.stub(x, y, side, Role::Line);
    }

    /// A run along the major axis from `from` up to and including `to`, the
    /// cell where it turns. A run one cell long is the turn alone, and reaches
    /// back only — the same cell drawn as a run would reach both ways and
    /// make a tee of the corner.
    fn leg(&self, c: &mut Canvas, from: usize, to: usize, minor: usize, stroke: Stroke) {
        if to > from {
            self.along(c, from, minor, to + 1 - from, stroke);
        } else {
            self.stub_back(c, to, minor);
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

    /// An arrowhead on a box's `Lo` or `Hi` side: it points across the minor
    /// axis, towards the box, from the side `from_hi` names.
    fn arrow_across(&self, c: &mut Canvas, major: usize, minor: usize, from_hi: bool) {
        let side = match (self.dir, from_hi) {
            (Dir::Td, true) => Side::Left,
            (Dir::Td, false) => Side::Right,
            (_, true) => Side::Up,
            (_, false) => Side::Down,
        };
        let (x, y) = self.point(major, minor);
        c.arrow(x, y, side, Role::Line);
    }

    /// Words beside a leg, starting at `major` and running along it, with a
    /// space either side so they never touch the line's dashes.
    fn label_from(&self, c: &mut Canvas, major: usize, minor: usize, text: &str) {
        let text = format!(" {text} ");
        let len = str_width(&text);
        c.text(self.at(major, len), minor, &text, Role::Label);
    }

    /// Words centred over the major range `lo..=hi`, at one place on the minor
    /// axis. This is the `LR` way round: the run under the label is horizontal,
    /// so the label lies along it.
    fn label_along(&self, c: &mut Canvas, lo: usize, hi: usize, minor: usize, text: &str) {
        let text = format!(" {text} ");
        let len = hi + 1 - lo;
        let pad = len.saturating_sub(str_width(&text)) / 2;
        c.text(self.at(lo, len) + pad, minor, &text, Role::Label);
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
        // the ends of a dotted run are where it touches a box or turns a
        // corner, and a gap there would cut the line loose — or, worse, blank
        // the edge the corner is about to be made from
        if c.get(x, y) != plain || (stroke == Stroke::Dotted && (i == 0 || i + 1 == len)) {
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

/// The four sides of a box an edge can touch. `In` and `Out` face the ranks
/// before and after; `Lo` and `Hi` face the two ends of the minor axis — the
/// top and bottom of an `LR` chart, the left and right of a `TD` one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side4 {
    In,
    Out,
    Lo,
    Hi,
}

impl Side4 {
    const ALL: [Side4; 4] = [Side4::In, Side4::Out, Side4::Lo, Side4::Hi];

    /// Whether ports on this side are spaced along the minor axis.
    fn minor(self) -> bool {
        matches!(self, Side4::In | Side4::Out)
    }

    /// The side facing the lane a detour runs in.
    fn lane(hi: bool) -> Side4 {
        if hi {
            Side4::Hi
        } else {
            Side4::Lo
        }
    }
}

/// How an edge gets from its box to the other one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// One rank forward: out of one side, across the gutter, into the next.
    Step,
    /// More than one rank forward, with nothing in the way: along its own
    /// row from the source, then up under the target and in through its far
    /// side. The usual shape of "one box feeds several down the line".
    Shot,
    /// Anything else: out to a lane beyond the layout, along it, and back in.
    /// `hi` says which side of the layout the lane is on. An end that is the
    /// last box in its rank on that side (`from_edge`, `to_edge`) reaches the
    /// lane straight from that side; any other goes through a gutter first.
    Detour {
        forward: bool,
        hi: bool,
        from_edge: bool,
        to_edge: bool,
    },
}

impl Route {
    /// The side an edge leaves its source by, and the side it reaches its
    /// target on.
    fn sides(self) -> (Side4, Side4) {
        match self {
            Route::Step => (Side4::Out, Side4::In),
            Route::Shot => (Side4::Out, Side4::Hi),
            Route::Detour {
                forward,
                hi,
                from_edge,
                to_edge,
            } => {
                let from = match (from_edge, forward) {
                    (true, _) => Side4::lane(hi),
                    (false, true) => Side4::Out,
                    (false, false) => Side4::In,
                };
                let to = if to_edge { Side4::lane(hi) } else { Side4::In };
                (from, to)
            }
        }
    }
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
    /// How each edge travels, by edge.
    routes: Vec<Route>,
    /// Where each edge touches its two boxes, by edge: a coordinate along the
    /// side it uses — on the minor axis for `In`/`Out`, the major for
    /// `Lo`/`Hi`. Two edges on one side of one box never share a port, so
    /// their lines never merge and their labels never land on each other.
    ports: Vec<(usize, usize)>,
    /// Where each edge's legs across a gutter run, by edge: the major
    /// coordinate it leaves along, in the gutter after its source (or before
    /// it, for a back edge), and the one it arrives along, before its target.
    /// Every leg in one gutter has a coordinate of its own, so two never
    /// merge into one line. `None` for a leg the route does not have.
    legs: Vec<(Option<usize>, Option<usize>)>,
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

/// Port `i` of `k` spread over `lo..=hi`: evenly, with the same air at both
/// ends as between neighbours. A lone port sits at `centre` — the middle of
/// the box — so a plain chain is drawn exactly as it always was.
fn spread(lo: usize, hi: usize, k: usize, i: usize, centre: usize) -> usize {
    if k <= 1 || hi < lo {
        return centre;
    }
    let span = hi + 1 - lo;
    lo + ((2 * i + 1) * span) / (2 * k)
}

/// A label with blank lines above it, so a box stretched past what its words
/// need still reads as centred.
fn pad(label: &[String], room: usize) -> Vec<String> {
    if room == 0 {
        return label.to_vec();
    }
    let mut out = vec![String::new(); room / 2];
    out.extend_from_slice(label);
    out
}

/// One leg across a gutter, waiting for a coordinate: the edge, and the two
/// minor positions it runs between — where it leaves the line it came along
/// and where it joins the one it goes on by.
#[derive(Clone, Copy)]
struct Jog {
    edge: usize,
    from: usize,
    to: usize,
}

impl Jog {
    fn covers(&self, minor: usize) -> bool {
        (self.from.min(self.to)..=self.from.max(self.to)).contains(&minor)
    }
}

/// Crossings between step jogs in one gutter, given the order they take
/// coordinates in — `order[0]` nearest the boxes the jogs arrive at. A jog's
/// leg in from its source runs as far as its own coordinate and crosses every
/// jog nearer the source that spans its row; its leg out to the target runs
/// from its coordinate onward and crosses every jog nearer the target that
/// spans that row.
fn crossings(jogs: &[Jog], order: &[usize]) -> usize {
    let mut n = 0;
    for (ia, &a) in order.iter().enumerate() {
        for (ib, &b) in order.iter().enumerate() {
            if a == b {
                continue;
            }
            // `ib > ia` puts b nearer the source than a
            if ib > ia && jogs[b].covers(jogs[a].from) {
                n += 1;
            }
            if ib < ia && jogs[b].covers(jogs[a].to) {
                n += 1;
            }
        }
    }
    n
}

/// The order step jogs in one gutter should take their coordinates in, nearest
/// the target first, so the fewest legs cross. Small enough to try every
/// order; a gutter with more jogs than that keeps the order they were written.
fn best_order(jogs: &[Jog]) -> Vec<usize> {
    let k = jogs.len();
    let mut order: Vec<usize> = (0..k).collect();
    if k > 6 {
        return order;
    }
    let mut best = (crossings(jogs, &order), order.clone());
    // Heap's algorithm, iteratively
    let mut c = vec![0usize; k];
    let mut i = 0;
    while i < k {
        if c[i] < i {
            if i % 2 == 0 {
                order.swap(0, i);
            } else {
                order.swap(c[i], i);
            }
            let n = crossings(jogs, &order);
            if n < best.0 {
                best = (n, order.clone());
            }
            c[i] += 1;
            i = 0;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
    best.1
}

/// How each edge travels. A forward edge is a shot if `shot` allows it — the
/// layout below vetoes one whose row is not clear — else a step if it goes
/// one rank on, else a detour, on whichever side of the layout more of its
/// ends are free to reach straight; a tie goes below for a forward edge,
/// above for a back edge, so the two kinds keep out of each other's way.
fn routes(g: &Graph, ranks: &[usize], by_rank: &[Vec<usize>], shot: &[bool]) -> Vec<Route> {
    g.edges
        .iter()
        .enumerate()
        .map(|(k, e)| {
            let (s, t) = (ranks[e.from], ranks[e.to]);
            let forward = t > s;
            let last = |i: usize| by_rank[ranks[i]].last() == Some(&i);
            let first = |i: usize| by_rank[ranks[i]].first() == Some(&i);
            // a shot is open to a step too: a box stacked under the chain
            // that feeds a box on it comes up from below, and the chain's own
            // line into that box stays straight
            if forward && shot[k] && last(e.to) {
                return Route::Shot;
            }
            if t == s + 1 {
                return Route::Step;
            }
            let hi_score = usize::from(last(e.from)) + usize::from(last(e.to));
            let lo_score = usize::from(first(e.from)) + usize::from(first(e.to));
            let hi = match hi_score.cmp(&lo_score) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => forward,
            };
            let (from_edge, to_edge) = if hi {
                (last(e.from), last(e.to))
            } else {
                (first(e.from), first(e.to))
            };
            Route::Detour {
                forward,
                hi,
                from_edge,
                to_edge,
            }
        })
        .collect()
}

/// Work out where every box, every port and every lane goes.
///
/// The minor axis is settled first — how boxes stack and where their ports
/// are — because nothing about it depends on the gutters, and the gutters
/// depend on it: a gutter is as wide as the words and the jogs it has to hold,
/// and which edges jog is only known once the ports are.
fn plan(g: &Graph, ranks: &[usize], dir: Dir, width: usize) -> Plan {
    let td = dir == Dir::Td;
    let count = ranks.iter().max().map_or(1, |r| r + 1);
    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (i, &r) in ranks.iter().enumerate() {
        by_rank[r].push(i);
    }
    let labels: Vec<Vec<String>> = g
        .nodes
        .iter()
        .map(|n| wrap(&n.label, (width / 3).max(MIN_LABEL)))
        .collect();

    // every skip starts out hoping for a straight shot; one whose row turns
    // out to have a box on it is demoted to a detour and the layout redone,
    // since its ports and its box's size change with it
    let mut shot = vec![true; g.edges.len()];
    let mut laid = None;
    for _ in 0..=g.edges.len() {
        let routes = routes(g, ranks, &by_rank, &shot);
        let (places, ports, from, to) = stack(g, ranks, &by_rank, &routes, &labels, td);
        let blocked: Vec<usize> = g
            .edges
            .iter()
            .enumerate()
            .filter(|(k, e)| {
                routes[*k] == Route::Shot && {
                    let (s, _) = ports[*k];
                    let t = &places[e.to];
                    s < t.minor + t.mn
                        || places.iter().enumerate().any(|(i, p)| {
                            i != e.to
                                && ranks[i] > ranks[e.from]
                                && ranks[i] <= ranks[e.to]
                                && (p.minor..p.minor + p.mn).contains(&s)
                        })
                }
            })
            .map(|(k, _)| k)
            .collect();
        laid = Some((routes, places, ports, from, to));
        if blocked.is_empty() {
            break;
        }
        for k in blocked {
            shot[k] = false;
        }
    }
    let (routes, mut places, _, from, to) = laid.expect("at least one layout");

    // along the major axis: every gutter as wide as the words on the legs
    // that leave into it and the legs that cross it, then rank after rank,
    // each as deep as its deepest box
    let ports = assign_ports(g, &routes, &places, td);
    let (gaps, legs) = gutters(g, ranks, &routes, &ports, count, td);
    let depth: Vec<usize> = by_rank
        .iter()
        .map(|rank| rank.iter().map(|&i| places[i].ms).max().unwrap_or(0))
        .collect();
    let mut starts = Vec::with_capacity(count);
    let mut at = 0;
    for r in 0..count {
        at += gaps[r];
        starts.push(at);
        at += depth[r];
    }
    let span = at;
    for (i, p) in places.iter_mut().enumerate() {
        p.major = starts[ranks[i]];
    }
    // the ports on a box's top and bottom sit along the major axis, which is
    // only now known
    let ports = assign_ports(g, &routes, &places, td);
    let legs: Vec<(Option<usize>, Option<usize>)> = legs
        .iter()
        .map(|&(a, b)| {
            let place = |(r, off): (usize, usize)| starts[r] - gaps[r] + off;
            (a.map(place), b.map(place))
        })
        .collect();
    let gaps: Vec<(usize, usize)> = (0..count)
        .map(|r| (starts[r] - gaps[r], starts[r]))
        .collect();

    Plan {
        frame: Frame { dir, span },
        ranks: ranks.to_vec(),
        places,
        gaps,
        from,
        to,
        routes,
        ports,
        legs,
    }
}

/// Size every box and stack the ranks along the minor axis, given how the
/// edges travel. Returns the boxes (with no major position yet), the ports,
/// and where the ranks start and end on the minor axis.
fn stack(
    g: &Graph,
    ranks: &[usize],
    by_rank: &[Vec<usize>],
    routes: &[Route],
    labels: &[Vec<String>],
    td: bool,
) -> (Vec<Place>, Vec<(usize, usize)>, usize, usize) {
    let stack = if td { STACK_TD } else { STACK_LR };

    // how many edges touch each side of each box: a side with several grows
    // the box until each can have a row (or column) of its own
    let mut touches = vec![[0usize; 4]; g.nodes.len()];
    for (e, route) in g.edges.iter().zip(routes) {
        let (a, b) = route.sides();
        touches[e.from][a as usize] += 1;
        touches[e.to][b as usize] += 1;
    }
    let mut major = Vec::with_capacity(g.nodes.len());
    let mut minor = Vec::with_capacity(g.nodes.len());
    let mut natural = Vec::with_capacity(g.nodes.len());
    for (i, (n, label)) in g.nodes.iter().zip(labels).enumerate() {
        let (w, h) = Canvas::node_size(n.shape, label);
        let (ms, mn) = if td { (h, w) } else { (w, h) };
        let t = &touches[i];
        let on_minor = t[Side4::In as usize].max(t[Side4::Out as usize]);
        let on_major = t[Side4::Lo as usize].max(t[Side4::Hi as usize]);
        // the lid takes a row, and a row is on the minor axis in `LR`
        let (lid_minor, lid_major) = if td {
            (0, n.shape.lid())
        } else {
            (n.shape.lid(), 0)
        };
        natural.push(mn);
        minor.push(mn.max(on_minor + 2 + lid_minor));
        major.push(ms.max(on_major + 2 + lid_major));
    }
    let depth: Vec<usize> = by_rank
        .iter()
        .map(|rank| rank.iter().map(|&i| major[i]).max().unwrap_or(0))
        .collect();

    // above the layout is a lane for each detour that goes that way, and a
    // row for the stem of one landing on a box's top
    let above = routes
        .iter()
        .filter(|r| matches!(r, Route::Detour { hi: false, .. }))
        .count();
    let from = if above > 0 { above + 1 } else { 0 };
    let mut places: Vec<Place> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| Place {
            major: 0,
            minor: 0,
            ms: depth[ranks[i]],
            mn: minor[i],
            // a box given more room than its words need holds them in the
            // middle of it rather than at the top: along the major axis when
            // its rank is deeper than it is (`TD`), and along the minor axis
            // when its ports made it taller (`LR`)
            label: pad(
                &labels[i],
                if td {
                    depth[ranks[i]] - major[i]
                } else {
                    minor[i] - natural[i]
                },
            ),
            shape: node.shape,
        })
        .collect();

    // across the minor axis: a box sits so that the middle one of the edges
    // feeding it runs straight, and anything else in its rank stacks on past
    // it. The ports are worked out from where the boxes are, and the boxes
    // from where the ports are, so it goes round twice: the first pass lines
    // centres up, the second lines the ports themselves up. Positions are
    // signed until the end: a box that wants to sit above where its rank
    // starts pulls the whole picture down rather than being pushed off line.
    let mut ports = vec![(0usize, 0usize); g.edges.len()];
    let mut to = from;
    for pass in 0..2 {
        let mut at_of: Vec<i64> = vec![0; g.nodes.len()];
        let mut low: i64 = 0;
        let mut high: i64 = 0;
        for rank in by_rank {
            let mut at: i64 = i64::MIN;
            for &i in rank {
                let mut feeds: Vec<(usize, &Edge)> = g
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(k, e)| routes[*k] == Route::Step && e.to == i)
                    .collect();
                feeds.sort_by_key(|(_, e)| at_of[e.from]);
                let wanted = feeds
                    .get((feeds.len().saturating_sub(1)) / 2)
                    .map_or(0, |&(k, e)| {
                        let p = &places[e.from];
                        if pass == 0 {
                            at_of[e.from] + (p.mn / 2) as i64 - (places[i].mn / 2) as i64
                        } else {
                            // a port's offset within its box does not depend
                            // on where the box is, which is what makes the
                            // first pass's ports usable for the second
                            let (s, t) = ports[k];
                            let s_off = s as i64 - p.minor as i64;
                            let t_off = t as i64 - places[i].minor as i64;
                            at_of[e.from] + s_off - t_off
                        }
                    });
                at = at.max(wanted);
                at_of[i] = at;
                low = low.min(at);
                at += (places[i].mn + stack) as i64;
                high = high.max(at - stack as i64);
            }
        }
        let shift = from as i64 - low;
        for (i, p) in places.iter_mut().enumerate() {
            p.minor = (at_of[i] + shift) as usize;
        }
        to = (high + shift) as usize;
        ports = assign_ports(g, routes, &places, td);
    }
    (places, ports, from, to)
}

/// Every edge's port on each of its boxes: the edges on one side of a box,
/// in the order their other ends lie, spread along that side.
fn assign_ports(g: &Graph, routes: &[Route], places: &[Place], td: bool) -> Vec<(usize, usize)> {
    let mut on_side: Vec<[Vec<usize>; 4]> =
        (0..g.nodes.len()).map(|_| Default::default()).collect();
    for (k, (e, route)) in g.edges.iter().zip(routes).enumerate() {
        let (a, b) = route.sides();
        on_side[e.from][a as usize].push(k);
        on_side[e.to][b as usize].push(k);
    }
    let centre = |i: usize, side: Side4| {
        let p = &places[i];
        if side.minor() {
            p.minor + p.mn / 2
        } else {
            p.major + p.ms / 2
        }
    };
    // where the far end of edge `k` is, along the axis this side's ports are
    // spread on: the far box's middle, or — for an end that lands on its top
    // or bottom — that edge of it, which is where the line will be
    let far = |k: usize, i: usize, side: Side4| {
        let e = &g.edges[k];
        let (a, b) = routes[k].sides();
        let (other, end) = if e.from == i { (e.to, b) } else { (e.from, a) };
        let p = &places[other];
        match (side.minor(), end) {
            (true, Side4::Hi) => p.minor + p.mn,
            (true, Side4::Lo) => p.minor.saturating_sub(1),
            (true, _) => p.minor + p.mn / 2,
            (false, _) => p.major + p.ms / 2,
        }
    };
    let mut ports = vec![(0usize, 0usize); g.edges.len()];
    for (i, sides) in on_side.iter_mut().enumerate() {
        for (s, edges) in sides.iter_mut().enumerate() {
            let side = Side4::ALL[s];
            edges.sort_by_key(|&k| far(k, i, side));
            let p = &places[i];
            // the lid takes the first row on the vertical axis
            let lid = if side.minor() != td { p.shape.lid() } else { 0 };
            let (lo, hi) = if side.minor() {
                (p.minor + 1 + lid, p.minor + p.mn - 2)
            } else {
                (p.major + 1 + lid, p.major + p.ms - 2)
            };
            let k = edges.len();
            for (n, &edge) in edges.iter().enumerate() {
                let at = spread(lo, hi, k, n, centre(i, side));
                if g.edges[edge].from == i {
                    ports[edge].0 = at;
                } else {
                    ports[edge].1 = at;
                }
            }
        }
    }
    ports
}

/// The gutter a detour's legs cross, if they do: the one after the source
/// (before it, going back) for the leg that leaves, and the one before the
/// target for the leg that arrives.
fn detour_gutters(ranks: &[usize], e: &Edge, route: Route) -> (Option<usize>, Option<usize>) {
    match route {
        Route::Detour {
            forward,
            from_edge,
            to_edge,
            ..
        } => (
            (!from_edge).then(|| {
                if forward {
                    ranks[e.from] + 1
                } else {
                    ranks[e.from]
                }
            }),
            (!to_edge).then(|| ranks[e.to]),
        ),
        _ => (None, None),
    }
}

/// How wide each gutter is, and where in it each edge's legs run, as `(rank,
/// offset)` pairs to be placed once the ranks are.
///
/// In `LR` a gutter holds, left to right: a dash, the longest label on a leg
/// leaving into it, a dash, one column per leg that crosses it, a dash and
/// the arrowhead. In `TD` it is rows: one for the labels if there are any,
/// one per leg, and the arrowhead's. The legs nearest the boxes they arrive
/// at are the steps', in the order that crosses least; the detours' legs sit
/// behind them, nearer the boxes they leave.
#[allow(clippy::type_complexity)]
fn gutters(
    g: &Graph,
    ranks: &[usize],
    routes: &[Route],
    ports: &[(usize, usize)],
    count: usize,
    td: bool,
) -> (
    Vec<usize>,
    Vec<(Option<(usize, usize)>, Option<(usize, usize)>)>,
) {
    let mut label_w = vec![0usize; count];
    let mut steps: Vec<Vec<Jog>> = vec![Vec::new(); count];
    // (edge, leaving) — a detour's leg in this gutter, and whether it is the
    // leg leaving its source rather than the one arriving at its target
    let mut detours: Vec<Vec<(usize, bool)>> = vec![Vec::new(); count];
    let mut arrivals = vec![false; count];
    for (k, (e, route)) in g.edges.iter().zip(routes).enumerate() {
        let (s, t) = ports[k];
        // words go beside the leg leaving the source, in the gutter after it
        let forward = ranks[e.to] > ranks[e.from];
        if let (Some(label), true) = (&e.label, forward) {
            let r = ranks[e.from] + 1;
            label_w[r] = label_w[r].max(str_width(label));
        }
        match *route {
            Route::Step => {
                let r = ranks[e.to];
                arrivals[r] = true;
                if s != t {
                    steps[r].push(Jog {
                        edge: k,
                        from: s,
                        to: t,
                    });
                }
            }
            Route::Shot => {}
            Route::Detour { .. } => {
                let (a, b) = detour_gutters(ranks, e, *route);
                if let Some(r) = a {
                    detours[r].push((k, true));
                }
                if let Some(r) = b {
                    arrivals[r] = true;
                    detours[r].push((k, false));
                }
            }
        }
    }
    let mut widths = vec![0usize; count];
    let mut legs: Vec<(Option<(usize, usize)>, Option<(usize, usize)>)> =
        vec![(None, None); g.edges.len()];
    for r in 0..count {
        let n = steps[r].len() + detours[r].len();
        let (head, words) = if td {
            (1, usize::from(label_w[r] > 0))
        } else {
            (2, if label_w[r] > 0 { label_w[r] + 4 } else { 1 })
        };
        let least = if td { GUTTER_TD } else { GUTTER_LR };
        let width = if r == 0 && !arrivals[0] && n == 0 {
            0
        } else {
            (words + n + head).max(least)
        };
        widths[r] = width;
        // coordinates from the arriving end backwards: the last `head` cells
        // are the arrowhead's, then the steps', then the detours'
        let mut next = width.saturating_sub(head);
        let mut take = || {
            next -= 1;
            next
        };
        let order = best_order(&steps[r]);
        for i in order {
            legs[steps[r][i].edge].1 = Some((r, take()));
        }
        for &(k, leaving) in &detours[r] {
            let at = Some((r, take()));
            if leaving {
                legs[k].0 = at;
            } else {
                legs[k].1 = at;
            }
        }
    }
    (widths, legs)
}

impl Plan {
    /// The last leg of a route that comes in through the gutter: from the
    /// turn to the box, stopping a cell short so the arrowhead has somewhere
    /// to sit.
    fn arrive(&self, c: &mut Canvas, e: &Edge, from: usize, minor: usize) {
        let edge = self.places[e.to].major;
        // an edge with no head runs onto the box's own border, where `put`
        // turns it into a tee; one with a head stops a cell short and the head
        // is what touches
        let end = if e.head { edge - 1 } else { edge + 1 };
        // a run one cell long reaches both ways and would draw the turn as a
        // tee; that short, the turn is a stub that only reaches onward
        if end > from + 1 {
            self.frame.along(c, from, minor, end - from, e.stroke);
        } else {
            self.frame.stub(c, from, minor);
        }
        if e.head {
            self.frame.arrow(c, edge - 1, minor);
        }
    }

    /// The last leg of a route that comes in across the minor axis, from a
    /// lane at `lane` to the box's `Lo` or `Hi` side at major `at`.
    fn land(&self, c: &mut Canvas, e: &Edge, at: usize, lane: usize) {
        let t = &self.places[e.to];
        let from_hi = lane >= t.minor + t.mn;
        let edge = if from_hi {
            t.minor + t.mn
        } else {
            t.minor.saturating_sub(1)
        };
        self.frame
            .across(c, at, edge.min(lane), edge.abs_diff(lane) + 1, e.stroke);
        if e.head {
            self.frame.arrow_across(c, at, edge, from_hi);
        }
    }

    /// The stem of a route leaving a box across the minor axis, from the
    /// box's `Lo` or `Hi` side at major `at` out to the lane.
    fn depart(&self, c: &mut Canvas, e: &Edge, at: usize, lane: usize) {
        let s = &self.places[e.from];
        let edge = if lane >= s.minor + s.mn {
            s.minor + s.mn
        } else {
            s.minor.saturating_sub(1)
        };
        self.frame
            .across(c, at, edge.min(lane), edge.abs_diff(lane) + 1, e.stroke);
    }

    /// The words on an edge, beside the leg that leaves its source: after the
    /// first dash, so `─ label ─`. `TD` has no room beside a leg one column
    /// wide, and keeps its words in the gutter's own row, centred between the
    /// two ends of the jog.
    fn label_leg(&self, c: &mut Canvas, e: &Edge, out: usize, s: usize, t: usize) {
        if let Some(label) = &e.label {
            if self.frame.td() {
                self.frame.label_across(c, out, s.min(t), s.max(t), label);
            } else {
                self.frame.label_from(c, out + 1, s, label);
            }
        }
    }

    /// An edge between neighbouring ranks: out of one box, a jog across the
    /// gutter to line up with the other, and in.
    fn step(&self, c: &mut Canvas, k: usize, e: &Edge) {
        let (s, t) = self.ports[k];
        let source = &self.places[e.from];
        let out = source.major + source.ms;
        let gap = self.gaps[self.ranks[e.to]];
        let turn = self.legs[k].1.unwrap_or(gap.1 - 1);
        self.frame.leg(c, out, turn, s, e.stroke);
        // a jog of one cell would be drawn as a crossing rather than as
        // nothing, so a straight line is left straight
        if s != t {
            self.frame
                .across(c, turn, s.min(t), s.abs_diff(t) + 1, e.stroke);
        }
        self.arrive(c, e, turn, t);
        let words = if self.frame.td() { gap.0 } else { out };
        self.label_leg(c, e, words, s, t);
    }

    /// A straight shot: along the source's own row, then in through the
    /// target's far side.
    fn shot(&self, c: &mut Canvas, k: usize, e: &Edge) {
        let (s, t) = self.ports[k];
        let source = &self.places[e.from];
        let out = source.major + source.ms;
        self.frame
            .along(c, out, s, (t + 1).saturating_sub(out), e.stroke);
        self.land(c, e, t, s);
        let words = if self.frame.td() {
            self.gaps[self.ranks[e.from] + 1].0
        } else {
            out
        };
        self.label_leg(c, e, words, s, s);
    }

    /// A detour: out of the source — straight from the side facing the lane,
    /// or along a gutter to it — along the lane, and into the target the same
    /// two ways. Every leg lies in a gutter or a lane, never over a box.
    fn detour(&self, c: &mut Canvas, k: usize, e: &Edge, forward: bool, lane: usize) {
        let (s, t) = self.ports[k];
        let (leave, arrive) = self.legs[k];
        let source = &self.places[e.from];
        let out = source.major + source.ms;
        // where on the lane the route comes from and goes to
        let a = match leave {
            Some(a) => {
                if forward {
                    self.frame.leg(c, out, a, s, e.stroke);
                } else {
                    self.frame
                        .along(c, a, s, source.major.saturating_sub(a), e.stroke);
                }
                self.frame
                    .across(c, a, lane.min(s), lane.abs_diff(s) + 1, e.stroke);
                a
            }
            None => {
                self.depart(c, e, s, lane);
                s
            }
        };
        let b = arrive.unwrap_or(t);
        self.frame
            .along(c, a.min(b), lane, a.abs_diff(b) + 1, e.stroke);
        match arrive {
            Some(b) => {
                self.frame
                    .across(c, b, lane.min(t), lane.abs_diff(t) + 1, e.stroke);
                self.arrive(c, e, b, t);
            }
            None => self.land(c, e, t, lane),
        }
        let Some(label) = &e.label else { return };
        if self.frame.td() {
            // the words go on a leg that runs across the page: the one
            // leaving, the one arriving, or the stem out of the source
            match (leave, arrive) {
                (Some(a), _) => self
                    .frame
                    .label_across(c, a, lane.min(s), lane.max(s), label),
                (None, Some(b)) => self
                    .frame
                    .label_across(c, b, lane.min(t), lane.max(t), label),
                (None, None) => {
                    let edge = if lane >= source.minor + source.mn {
                        source.minor + source.mn
                    } else {
                        source.minor.saturating_sub(1)
                    };
                    self.frame
                        .label_across(c, s, lane.min(edge), lane.max(edge), label);
                }
            }
        } else if forward && leave.is_some() {
            self.label_leg(c, e, out, s, s);
        } else {
            self.frame.label_along(c, a.min(b), a.max(b), lane, label);
        }
    }
}

/// Place the ranked graph on a canvas, running `dir`, trying to fit `width`.
fn draw(g: &Graph, ranks: &[usize], dir: Dir, width: usize) -> Canvas {
    let plan = plan(g, ranks, dir, width);
    let (_, _, w, h) = plan.frame.rect(0, 0, plan.frame.span, plan.to);
    let mut c = Canvas::new(w.max(1), h.max(1));

    for p in &plan.places {
        let (x, y, w, h) = plan.frame.rect(p.major, p.minor, p.ms, p.mn);
        c.node(x, y, w, h, p.shape, &p.label);
    }
    // one lane per detour, above the layout or below it, in the order they
    // were written — the first one written stays nearest. A row is left
    // between the layout and the first lane for the stem of a line leaving
    // or landing on a box's top or bottom.
    let (mut above, mut below) = (0, 0);
    for (k, e) in g.edges.iter().enumerate() {
        match plan.routes[k] {
            Route::Step => plan.step(&mut c, k, e),
            Route::Shot => plan.shot(&mut c, k, e),
            Route::Detour { forward, hi, .. } => {
                let lane = if hi {
                    below += 1;
                    plan.to + below
                } else {
                    above += 1;
                    plan.from - 1 - above
                };
                plan.detour(&mut c, k, e, forward, lane);
            }
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
                Shape::Cylinder,
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
            "│ A │─ yes ──▶│ B │"
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

    /// The two flowcharts from the note that first showed the layout up: a
    /// chain with a second box feeding into it and skipping down it, and a
    /// fan-in of three labelled edges into a store.
    const KITCHEN: &str = "flowchart LR\n\
        A[Raw data<br/>the pantry] --> B[Cleaned features<br/>prep station]\n\
        B --> C[Training runs<br/>test kitchen]\n\
        C --> D[Eval<br/>tasting]\n\
        G[Orchestrator<br/>kitchen manager] -.runs the order.-> B\n\
        G -.-> C\n\
        G -.-> D";

    pub(super) const LINEAGE: &str = "flowchart LR\n\
        C[Training run] -->|\"which data, which config\"| N[(NM lineage)]\n\
        D[Eval run] -->|\"which model, score\"| N\n\
        E[Registry approve] -->|\"who approved\"| N\n\
        N -->|\"policy check\"| F{Allowed?}\n\
        F -->|yes| S[Serving]\n\
        F -->|no| X[Blocked]";

    #[test]
    fn edges_into_one_box_get_a_row_each_and_their_labels_stay_whole() {
        let rows = drawn(LINEAGE, Dir::Lr);
        // the store sits level with the first box feeding it, and the two
        // below come up under it, each on its own row and its own column, so
        // every label is drawn whole rather than over the next one
        assert_eq!(row_of(&rows, "Training run"), row_of(&rows, "NM lineage"));
        assert!(rows[row_of(&rows, "NM lineage")].contains("──▶│ NM lineage │"));
        let under = row_of(&rows, "╰────────────╯") + 1;
        assert_eq!(rows[under].matches('▲').count(), 2, "{rows:#?}");
        for label in [
            "which data, which config",
            "which model, score",
            "who approved",
        ] {
            assert_eq!(
                rows.iter().filter(|r| r.contains(label)).count(),
                1,
                "{label:?} in {rows:#?}"
            );
        }
        assert!(!rows.iter().any(|r| r.contains('┼')), "{rows:#?}");
    }

    #[test]
    fn a_cylinder_is_drawn_with_a_lid() {
        let rows = drawn("flowchart LR\nA[(db)] --> B", Dir::Lr);
        assert!(rows[1].starts_with("├────┤"), "{rows:#?}");
        assert_eq!(rows[2], "│ db │─────▶│ B │");
    }

    #[test]
    fn a_chain_stays_level_and_a_skip_with_a_clear_row_lands_from_below() {
        let rows = drawn(KITCHEN, Dir::Lr);
        // the chain runs straight across, whatever else feeds its second box
        let top = row_of(&rows, "Raw data");
        for name in ["Cleaned features", "Training runs", "Eval"] {
            assert_eq!(row_of(&rows, name), top, "{name} is off the line");
        }
        // the orchestrator's three edges run along their own rows and come up
        // under their targets, on the row below the chain's boxes — the one
        // to the neighbouring rank too, so the chain's line into that box
        // stays straight
        let under = row_of(&rows, "╰") + 1;
        assert_eq!(rows[under].matches('▲').count(), 3, "{rows:#?}");
        // and nothing crosses anything: every leg has a column of its own
        assert!(!rows.iter().any(|r| r.contains('┼')), "{rows:#?}");
        assert_eq!(
            rows.iter().filter(|r| r.contains("runs the order")).count(),
            1
        );
    }

    #[test]
    fn a_back_edge_goes_under_when_both_its_ends_are_at_the_bottom() {
        let src = "flowchart LR\nA[Start] --> B{ok?}\nB -->|yes| C[Ship it]\nB -->|no| D[Fix it]\nD --> B";
        let rows = drawn(src, Dir::Lr);
        // the loop from the lower box back to the decision runs under the
        // picture and lands on the decision's underside, crossing nothing
        assert!(rows.iter().any(|r| r.contains('▲')), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains('▼')));
        assert!(!rows.iter().any(|r| r.contains('┼')), "{rows:#?}");
        assert!(rows[row_of(&rows, "Ship it")].ends_with("│─ yes ───▶│ Ship it │"));
        assert!(rows[row_of(&rows, "Start")].starts_with("│ Start │─────▶│"));
    }

    #[test]
    fn a_fan_out_takes_a_column_per_jog_so_nothing_merges() {
        let src = "flowchart LR\nA -->|one| B\nA -->|two| C\nA -->|three| D";
        let rows = drawn(src, Dir::Lr);
        assert!(
            !rows
                .iter()
                .any(|r| r.contains('┼') || r.contains('┤') || r.contains('├')),
            "{rows:#?}"
        );
        assert_eq!(rows[row_of(&rows, "one")], "│   │─ one ──────▶│ B │");
        assert_eq!(rows[row_of(&rows, "two")], "│ A │─ two ────╮  ╰───╯");
        assert_eq!(rows[row_of(&rows, "three")], "│   │─ three ─╮│");
    }

    #[test]
    fn a_top_down_jog_turns_on_a_row_of_its_own() {
        let src = "graph TD\nA[Start] --> B{ok?}\nB -->|yes| C[Ship it]\nB -->|no| D[Fix it]";
        let rows = drawn(src, Dir::Td);
        // the jog to the second box is a corner and a run, not a tee on the
        // arrow's row
        assert!(
            rows.iter().any(|r| r.contains("╰─") && r.contains('╮')),
            "{rows:#?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains('┤') || r.contains('├')),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.trim() == "▼             ▼"),
            "{rows:#?}"
        );
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
