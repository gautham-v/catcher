//! Markdown → ratatui Text, kept deliberately small. This is the piece that
//! grows into live preview later.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub fn render(markdown: &str) -> Text<'static> {
    let parser = Parser::new_ext(
        markdown,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
    );

    let mut lines: Vec<Line> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut prefix = String::new(); // current indent/bullet prefix for new lines
    let mut list_depth: usize = 0;
    let mut in_code_block = false;

    macro_rules! flush {
        () => {
            if !spans.is_empty() {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
        };
    }
    macro_rules! blank {
        () => {
            flush!();
            if !lines.is_empty() && !lines.last().map(|l| l.spans.is_empty()).unwrap_or(true) {
                lines.push(Line::default());
            }
        };
    }

    let style = |stack: &[Style]| *stack.last().unwrap();

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                blank!();
                let s = match level {
                    HeadingLevel::H1 => Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan),
                    HeadingLevel::H2 => Style::new().add_modifier(Modifier::BOLD).fg(Color::Blue),
                    _ => Style::new().add_modifier(Modifier::BOLD),
                };
                style_stack.push(s);
            }
            Event::End(TagEnd::Heading(_)) => {
                style_stack.pop();
                flush!();
            }
            Event::Start(Tag::Paragraph) => {
                if list_depth == 0 {
                    blank!();
                }
            }
            Event::End(TagEnd::Paragraph) => flush!(),
            Event::Start(Tag::BlockQuote(_)) => {
                blank!();
                prefix.push_str("▌ ");
                style_stack.push(
                    Style::new()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                );
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                style_stack.pop();
                prefix.truncate(prefix.len().saturating_sub("▌ ".len()));
                flush!();
            }
            Event::Start(Tag::List(_)) => {
                if list_depth == 0 {
                    blank!();
                }
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                flush!();
            }
            Event::Start(Tag::Item) => {
                flush!();
                spans.push(Span::styled(
                    format!("{}{}• ", prefix, "  ".repeat(list_depth.saturating_sub(1))),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            Event::End(TagEnd::Item) => flush!(),
            Event::TaskListMarker(done) => {
                spans.pop(); // replace the bullet we just pushed
                let (mark, color) = if done {
                    ("✓ ", Color::Green)
                } else {
                    ("☐ ", Color::DarkGray)
                };
                spans.push(Span::styled(
                    format!(
                        "{}{}{mark}",
                        prefix,
                        "  ".repeat(list_depth.saturating_sub(1))
                    ),
                    Style::new().fg(color),
                ));
            }
            Event::Start(Tag::CodeBlock(_)) => {
                blank!();
                in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                flush!();
            }
            Event::Start(Tag::Emphasis) => {
                style_stack.push(style(&style_stack).add_modifier(Modifier::ITALIC))
            }
            Event::End(TagEnd::Emphasis) => {
                style_stack.pop();
            }
            Event::Start(Tag::Strong) => {
                style_stack.push(style(&style_stack).add_modifier(Modifier::BOLD))
            }
            Event::End(TagEnd::Strong) => {
                style_stack.pop();
            }
            Event::Start(Tag::Strikethrough) => {
                style_stack.push(style(&style_stack).add_modifier(Modifier::CROSSED_OUT))
            }
            Event::End(TagEnd::Strikethrough) => {
                style_stack.pop();
            }
            Event::Start(Tag::Link { .. }) => style_stack.push(
                style(&style_stack)
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Event::End(TagEnd::Link) => {
                style_stack.pop();
            }
            Event::Code(code) => {
                spans.push(Span::styled(
                    code.into_string(),
                    Style::new().fg(Color::Yellow),
                ));
            }
            Event::Text(text) => {
                if in_code_block {
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("  {l}"),
                            Style::new().fg(Color::Yellow),
                        )));
                    }
                } else {
                    spans.push(Span::styled(text.into_string(), style(&style_stack)));
                }
            }
            Event::SoftBreak => {
                flush!();
                if !prefix.is_empty() {
                    spans.push(Span::styled(
                        prefix.clone(),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
            Event::HardBreak => flush!(),
            Event::Rule => {
                blank!();
                lines.push(Line::from(Span::styled(
                    "─".repeat(40),
                    Style::new().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }
    flush!();
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_without_panic() {
        let md = "# Title\n\nSome **bold** and *italic* and `code`.\n\n- one\n- [ ] task\n- [x] done\n\n> quote\n\n```\nlet x = 1;\n```\n\n---\n";
        let text = render(md);
        assert!(text.lines.len() > 5);
        let flat: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone()))
            .collect();
        assert!(flat.contains("Title"));
        assert!(flat.contains("bold"));
        assert!(flat.contains("let x = 1;"));
    }
}
