//! Markdown → Ratatui `Line` converter.
//!
//! Walks the pulldown-cmark event stream, turning headings, emphasis, code,
//! lists, quotes, links, etc. into a list of terminal-styled `Line<'static>`.
//! Some blocks (e.g. tables) are flattened to text. A lightweight renderer for
//! use in the preview modal.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Render markdown source into a list of styled lines.
pub fn render(src: &str, t: &Theme) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut r = Renderer {
        t: *t,
        ..Default::default()
    };
    for ev in Parser::new_ext(src, opts) {
        r.handle(ev);
    }
    r.finish()
}

#[derive(Default)]
struct Renderer {
    t: Theme,
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    // Nesting counters (track depth only, not a stack).
    bold: u32,
    italic: u32,
    strike: u32,
    heading: bool,
    quote: u32,
    in_code: bool,
    list: Vec<Option<u64>>, // None=bullet, Some(n)=next number
    link_dest: Option<String>,
}

impl Renderer {
    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => self.inline_code(&t),
            Event::SoftBreak => self.cur.push(Span::raw(" ")),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.lines
                    .push(Line::from("─".repeat(48)).style(Style::new().fg(self.t.muted)));
                self.blank();
            }
            Event::TaskListMarker(done) => {
                let m = if done { "[✓] " } else { "[ ] " };
                self.cur
                    .push(Span::styled(m.to_string(), Style::new().fg(self.t.accent)));
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = true;
                let hashes = "#".repeat(heading_depth(level));
                self.cur.push(Span::styled(
                    format!("{hashes} "),
                    Style::new().fg(self.t.accent).add_modifier(Modifier::BOLD),
                ));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote += 1;
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.in_code = true;
            }
            Tag::List(start) => self.list.push(start),
            Tag::Item => {
                self.flush();
                let depth = self.list.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = match self.list.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    }
                    _ => "• ".to_string(),
                };
                self.cur.push(Span::styled(
                    format!("{indent}{marker}"),
                    Style::new().fg(self.t.warning),
                ));
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link_dest = Some(dest_url.to_string()),
            Tag::Image { .. } => self.cur.push(Span::raw("🖼 ")),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank();
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = false;
                self.blank();
            }
            TagEnd::BlockQuote(_) => self.quote = self.quote.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.in_code = false;
                self.blank();
            }
            TagEnd::List(_) => {
                self.list.pop();
                self.blank();
            }
            TagEnd::Item => self.flush(),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(url) = self.link_dest.take() {
                    self.cur.push(Span::styled(
                        format!(" ({url})"),
                        Style::new().fg(self.t.muted),
                    ));
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_code {
            // Code blocks render line by line as gray lines.
            for (i, raw) in t.split('\n').enumerate() {
                if i > 0 {
                    self.flush();
                }
                self.cur.push(Span::styled(
                    format!("  {raw}"),
                    Style::new().fg(self.t.code),
                ));
            }
            return;
        }
        let mut style = self.inline_style();
        if self.link_dest.is_some() {
            style = style.fg(self.t.link).add_modifier(Modifier::UNDERLINED);
        }
        self.cur.push(Span::styled(t.to_string(), style));
    }

    fn inline_code(&mut self, t: &str) {
        self.cur
            .push(Span::styled(format!(" {t} "), Style::new().fg(self.t.code)));
    }

    fn inline_style(&self) -> Style {
        let mut s = Style::new();
        if self.heading {
            s = s.fg(self.t.accent).add_modifier(Modifier::BOLD);
        }
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        s
    }

    /// Commit the current span buffer as one line (applying the quote prefix).
    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let mut spans = Vec::new();
        if self.quote > 0 {
            spans.push(Span::styled(
                "▌ ".repeat(self.quote as usize),
                Style::new().fg(self.t.muted),
            ));
        }
        spans.append(&mut self.cur);
        self.lines.push(Line::from(spans));
    }

    /// Add a single blank line (consecutive blank lines are collapsed).
    fn blank(&mut self) {
        if self.lines.last().map(|l| l.spans.is_empty()) == Some(true) {
            return;
        }
        self.lines.push(Line::default());
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self.lines.last().map(|l| l.spans.is_empty()) == Some(true) {
            self.lines.pop();
        }
        self.lines
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::theme::Theme;

    #[test]
    fn renders_basic_markdown() {
        let md = "# Title\n\nSome **bold** and *italic* and `code`.\n\n- a\n- b\n\n> quote\n\n```\nfn x() {}\n```\n";
        let lines = render(md, &Theme::default());
        assert!(!lines.is_empty());
        // Every line must be a valid Line (no panic).
        let total: usize = lines.iter().map(|l| l.spans.len()).sum();
        assert!(total > 0);
    }

    #[test]
    fn handles_empty() {
        assert!(render("", &Theme::default()).is_empty());
    }
}
