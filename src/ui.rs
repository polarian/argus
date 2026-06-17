//! Ratatui rendering: header + responsive panels + modal + footer. All colors go through `Theme`.

use chrono::{DateTime, Utc};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, Padding, Paragraph, Wrap},
};

use crate::app::{App, Detail, LayoutKind, Modal, Panel, SPINNER};
use crate::backend::Backend;
use crate::github::{Branch, Commit, DetailKind, Issue, Pr, Run, is_failure};
use crate::i18n::{self, Lang};
use crate::theme::Theme;

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Min(0),    // body
        Constraint::Length(1), // footer
    ])
    .split(f.area());

    render_header(f, app, chunks[0]);

    let ([a0, a1, a2, a3], kind) = panel_layout(chunks[1]);
    app.layout = kind; // store current layout to interpret arrow-key movement.
    render_runs(f, app, a0);
    render_prs(f, app, a1);
    // Bitbucket dropped Issues, so it shows Branches in that slot.
    if app.backend == Backend::Bitbucket {
        render_branches(f, app, a2);
    } else {
        render_issues(f, app, a2);
    }
    render_commits(f, app, a3);

    render_footer(f, app, chunks[2]);

    // Detail preview modal as a top overlay (mutable borrow for scroll clamp).
    let lang = app.lang;
    if let Modal::Detail(d) = &mut app.modal {
        let spin = SPINNER[app.spinner % SPINNER.len()];
        render_detail_modal(f, d, spin, lang, &app.theme);
    }
}

/// Splits the body area into 4 panels. Switches automatically by terminal size:
/// - wide and tall → 2×2 grid
/// - short but wide enough → 4 columns (4×1) — stacking vertically would be too flat
/// - otherwise (narrow width) → single column, 4 rows (1×4)
fn panel_layout(body: Rect) -> ([Rect; 4], LayoutKind) {
    const MIN_W_2COL: u16 = 100; // min width of one 2×2 column
    const MIN_H_2ROW: u16 = 22; // min height of one 2×2 row
    const MIN_W_4COL: u16 = 112; // min width of one 4-column cell

    let two_col = body.width >= MIN_W_2COL;
    let two_row = body.height >= MIN_H_2ROW;

    if two_col && two_row {
        let rows = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .split(body);
        let top = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .split(rows[0]);
        let bot = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .spacing(1)
            .split(rows[1]);
        ([top[0], top[1], bot[0], bot[1]], LayoutKind::Grid)
    } else if !two_row && body.width >= MIN_W_4COL {
        let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4])
            .spacing(1)
            .split(body);
        ([cols[0], cols[1], cols[2], cols[3]], LayoutKind::Cols)
    } else {
        let rows = Layout::vertical([Constraint::Ratio(1, 4); 4])
            .spacing(1)
            .split(body);
        ([rows[0], rows[1], rows[2], rows[3]], LayoutKind::Rows)
    }
}

// ─── detail preview modal ──────────────────────────────────────────────────────

fn render_detail_modal(f: &mut Frame, d: &mut Detail, spin: &str, lang: Lang, t: &Theme) {
    let area = centered_rect(82, 80, f.area());
    f.render_widget(Clear, area); // clear the background panel.

    // success (LIVE) while following, otherwise accent.
    let accent = if d.following { t.success } else { t.accent };
    let max_title = (area.width as usize).saturating_sub(16);
    let title = if d.loading {
        format!(" {spin} {} ", truncate(&d.title, max_title))
    } else if d.following {
        format!(" {spin} LIVE · {} ", truncate(&d.title, max_title))
    } else {
        format!(" {} ", truncate(&d.title, max_title))
    };

    let hint = if matches!(d.kind, DetailKind::Run { .. }) {
        i18n::modal_hint_run(lang)
    } else {
        i18n::modal_hint_other(lang)
    };

    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(accent))
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(
            title,
            Style::new().fg(accent).bold(),
        )))
        .title_bottom(Line::from(Span::styled(hint, Style::new().fg(t.muted))).right_aligned());

    if d.loading {
        let body = Line::from(Span::styled(i18n::loading(lang), Style::new().fg(t.muted)));
        f.render_widget(Paragraph::new(body).block(block), area);
    } else {
        // Scroll cap: only allow scrolling up to how far wrapped line count exceeds visible height (prevents endless scroll on blank screen).
        let inner_w = (area.width.saturating_sub(4)).max(1) as usize; // 2 border + 2 horizontal padding
        let inner_h = area.height.saturating_sub(2) as usize; // top/bottom border
        let total: usize = d
            .lines
            .iter()
            .map(|l| l.width().max(1).div_ceil(inner_w))
            .sum();
        d.scroll = d.scroll.min(total.saturating_sub(inner_h) as u16);

        let para = Paragraph::new(Text::from(d.lines.clone()))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((d.scroll, 0));
        f.render_widget(para, area);
    }
}

/// Builds a (pw% × ph%) rectangle centered within the given area.
fn centered_rect(pw: u16, ph: u16, area: Rect) -> Rect {
    let vert = Layout::vertical([
        Constraint::Percentage((100 - ph) / 2),
        Constraint::Percentage(ph),
        Constraint::Percentage((100 - ph) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pw) / 2),
        Constraint::Percentage(pw),
        Constraint::Percentage((100 - pw) / 2),
    ])
    .split(vert[1])[1]
}

// ─── header / footer ─────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let lang = app.lang;
    let spin = if app.has_active_runs() {
        format!("{} ", SPINNER[app.spinner % SPINNER.len()])
    } else {
        String::new()
    };

    let refreshed = app
        .last_refresh
        .map(|x| x.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| i18n::loading_short(lang).to_string());

    let mut left_spans = vec![
        Span::raw(" "),
        Span::styled("◉ argus", Style::new().fg(t.accent).bold()),
        Span::raw("  "),
        Span::styled(app.repo.clone(), Style::new().fg(t.text).bold()),
    ];
    if let Some(v) = &app.update {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            i18n::update_available(lang, v.clone()),
            Style::new().fg(t.warning).bold(),
        ));
    }
    let left = Line::from(left_spans);

    let right = Line::from(vec![
        Span::styled(spin, Style::new().fg(t.warning)),
        Span::styled("⟳ ", Style::new().fg(t.muted)),
        Span::styled(refreshed, Style::new().fg(t.muted)),
        Span::styled("  ·  ", Style::new().fg(t.muted)),
        Span::styled(
            format!("{}s", app.poll_secs),
            Style::new().fg(t.accent).bold(),
        ),
        Span::styled(i18n::poll_suffix(lang), Style::new().fg(t.muted)),
    ]);

    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right).right_aligned(), area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let lang = app.lang;

    // Search input mode: show the input line.
    if app.search_active {
        let line = Line::from(vec![
            Span::styled(" 🔍 /", Style::new().fg(t.warning).bold()),
            Span::styled(app.search_buffer.clone(), Style::new().fg(t.text)),
            Span::styled("▏", Style::new().fg(t.warning).bold()),
            Span::styled(i18n::search_confirm_hint(lang), Style::new().fg(t.muted)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    let (accent, muted) = (t.accent, t.muted);
    // Shortcut keys are shown uppercase (the bindings themselves stay lowercase).
    let hint = |k: &'static str, d: &'static str| {
        vec![
            Span::styled(
                format!(" {}", k.to_uppercase()),
                Style::new().fg(accent).bold(),
            ),
            Span::styled(format!(" {d}"), Style::new().fg(muted)),
        ]
    };
    let mut spans = Vec::new();
    spans.extend(hint("Tab", i18n::key_panels(lang)));
    spans.extend(hint("↑/↓", i18n::key_scroll(lang)));
    spans.extend(hint("/", i18n::key_search(lang)));
    spans.extend(hint("↵", i18n::key_open(lang)));
    spans.extend(hint("v", i18n::key_preview(lang)));
    spans.extend(hint("r", i18n::key_refresh(lang)));
    spans.extend(hint("+/-", i18n::key_interval(lang)));
    spans.extend(hint("q", i18n::key_quit(lang)));

    let total: usize = app.runs.highlight_count()
        + app.prs.highlight_count()
        + app.issues.highlight_count()
        + app.commits.highlight_count();
    if total > 0 {
        spans.push(Span::styled(
            i18n::changed(lang, total),
            Style::new().fg(t.highlight).bold(),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ─── shared panel shell ────────────────────────────────────────────────────────

/// Builds a panel border block. Changes title/color by focus, highlight, and filter state.
/// `extra` is a mini visualization appended after the count (success-rate bar, sparkline, etc.).
#[allow(clippy::too_many_arguments)]
fn panel_block(
    title: &str,
    focused: bool,
    total: usize,
    visible: usize,
    hl: usize,
    filter: &str,
    t: &Theme,
    extra: Vec<Span<'static>>,
) -> Block<'static> {
    let (border_color, border_type) = if focused {
        (t.accent, BorderType::Thick)
    } else {
        (t.muted, BorderType::Rounded)
    };

    let mut spans = vec![Span::styled(
        format!(" {title} "),
        Style::new()
            .fg(if focused { t.accent } else { t.muted })
            .bold(),
    )];
    // visible/total when a filter is active, otherwise the total count.
    let count = if filter.is_empty() {
        format!("{total} ")
    } else {
        format!("{visible}/{total} ")
    };
    spans.push(Span::styled(count, Style::new().fg(t.muted)));
    spans.extend(extra);
    if hl > 0 {
        spans.push(Span::styled(
            format!("●{hl} "),
            Style::new().fg(t.highlight).bold(),
        ));
    }
    if !filter.is_empty() {
        spans.push(Span::styled(
            format!("🔍{filter} "),
            Style::new().fg(t.warning).bold(),
        ));
    }

    Block::bordered()
        .border_type(border_type)
        .border_style(Style::new().fg(border_color))
        .padding(Padding::horizontal(1))
        .title(Line::from(spans))
}

/// Renders a ratio (0.0~1.0) as a filled-block mini-bar span. The caller picks the fill color by meaning.
fn ratio_bar(ratio: f32, width: usize, fill: Color) -> Span<'static> {
    let filled = (ratio * width as f32).round() as usize;
    let filled = filled.min(width);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    Span::styled(bar, Style::new().fg(fill))
}

const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Renders a value slice as a Unicode sparkline span (normalized to the max value).
fn sparkline(vals: &[u64], t: &Theme) -> Span<'static> {
    if vals.is_empty() {
        return Span::raw("");
    }
    let max = (*vals.iter().max().unwrap()).max(1);
    let s: String = vals
        .iter()
        .map(|&v| {
            let idx = ((v as f32 / max as f32) * 7.0).round() as usize;
            SPARKS[idx.min(7)]
        })
        .collect();
    Span::styled(s, Style::new().fg(t.accent))
}

/// **Completion mini-bar + failure badge** span for recent runs.
/// Completion = `status=="completed"` (finished runs) / total — skipped and cancelled also count as "finished".
/// Drops below 100% only when there are in-progress/queued runs. 100% uses the success color, otherwise it means
/// in progress, so accent (not "bad"). If any run failed, appends a `✗N` badge in the error color.
/// Empty vec if there are no runs.
fn run_completion_bar(runs: &[Run], t: &Theme) -> Vec<Span<'static>> {
    if runs.is_empty() {
        return vec![];
    }
    let completed = runs.iter().filter(|r| r.status == "completed").count();
    let failure = runs
        .iter()
        .filter(|r| is_failure(r.conclusion.as_deref()))
        .count();
    let ratio = completed as f32 / runs.len() as f32;
    let done = completed == runs.len();
    // Low completion means "in progress", not "bad", so don't use red.
    let fill = if done { t.success } else { t.accent };
    // Keep the percentage consistent with the color: rounding could push 99.x% up to 100 and mismatch fill (accent),
    // so clamp incomplete to a 99% cap (100% shows only with the completion color).
    let pct = if done {
        100
    } else {
        ((ratio * 100.0).round() as u32).min(99)
    };
    let mut spans = vec![
        ratio_bar(ratio, 8, fill),
        Span::styled(format!(" {pct}% "), Style::new().fg(t.muted)),
    ];
    if failure > 0 {
        spans.push(Span::styled(
            format!("✗{failure} "),
            Style::new().fg(t.error).bold(),
        ));
    }
    spans
}

/// Buckets the time distribution of recent commits into 14 sparkline cells (newest on the right).
/// Instead of fixed day/hour units, it normalizes by **splitting the commits' actual time span into 14 parts**,
/// so the distribution pattern shows whether it's 20 commits in a day or 20 commits over two weeks.
fn commit_activity(commits: &[Commit]) -> Vec<u64> {
    const N: usize = 14;
    let ages: Vec<i64> = commits
        .iter()
        .filter_map(|c| age_secs(&c.commit.author.date))
        .collect();
    let Some(&max_age) = ages.iter().max() else {
        return vec![];
    };
    let mut buckets = vec![0u64; N];
    for a in ages {
        // newest (a=0) at the right end (N-1), oldest (a=max_age) at the left (0).
        let idx = if max_age == 0 {
            N - 1
        } else {
            (((max_age - a) as f64 / max_age as f64) * (N - 1) as f64).round() as usize
        };
        buckets[idx.min(N - 1)] += 1;
    }
    buckets
}

/// Renders loading/error/empty state as a single paragraph (when there's no data). Returns false if data exists.
#[allow(clippy::too_many_arguments)]
fn render_placeholder(
    f: &mut Frame,
    area: Rect,
    block: Block,
    loaded: bool,
    err: &Option<String>,
    empty: bool,
    filtered: bool,
    lang: Lang,
    t: &Theme,
) -> bool {
    let content: Option<Text> = if let Some(e) = err {
        // Split multi-line messages (\n) per line so they show as-is (⚠ only on the first line).
        let lines: Vec<Line> = format!("⚠ {e}")
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(t.error))))
            .collect();
        Some(Text::from(lines))
    } else if !loaded {
        // Loading skeleton: dim placeholder bars of varying lengths.
        let widths = [20usize, 28, 15, 24, 18, 26];
        let skel: Vec<Line> = widths
            .iter()
            .map(|&w| Line::from(Span::styled("░".repeat(w), Style::new().fg(t.muted))))
            .collect();
        Some(Text::from(skel))
    } else if empty {
        let text = if filtered {
            i18n::filtered_empty(lang)
        } else {
            i18n::empty(lang)
        };
        Some(Text::from(Line::from(Span::styled(
            text,
            Style::new().fg(t.muted),
        ))))
    } else {
        None
    };

    match content {
        Some(text) => {
            // Wrap to panel width so long messages aren't cut off (preserving indentation).
            f.render_widget(
                Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                area,
            );
            true
        }
        None => false,
    }
}

/// Marker span prefixed to new/changed items.
fn marker(highlighted: bool, t: &Theme) -> Span<'static> {
    if highlighted {
        Span::styled("● ", Style::new().fg(t.highlight).bold())
    } else {
        Span::raw("  ")
    }
}

/// Gray meta span (time, author, etc.).
fn meta(text: String, t: &Theme) -> Span<'static> {
    Span::styled(text, Style::new().fg(t.muted))
}

// ─── time formatting ───────────────────────────────────────────────────────────────

/// ISO8601 → age (seconds). None on parse failure.
fn age_secs(iso: &str) -> Option<i64> {
    iso.parse::<DateTime<Utc>>()
        .ok()
        .map(|t| (Utc::now() - t).num_seconds().max(0))
}

/// ISO8601 → relative time like "5m".
fn rel_time(iso: &str) -> String {
    let Some(secs) = age_secs(iso) else {
        return "—".into();
    };
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Relative time span — bright (text) if recent (within 1 hour), otherwise dim (muted).
fn time_span(iso: &str, t: &Theme) -> Span<'static> {
    let recent = age_secs(iso).map(|s| s < 3600).unwrap_or(false);
    let color = if recent { t.text } else { t.muted };
    Span::styled(rel_time(iso), Style::new().fg(color))
}

// ─── Actions ────────────────────────────────────────────────────────────────

fn render_runs(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Actions;
    let t = app.theme;
    let p = &app.runs;
    let block = panel_block(
        "⚙ Actions",
        focused,
        p.items.len(),
        p.visible_count(),
        p.highlight_count(),
        &p.filter,
        &t,
        run_completion_bar(&p.items, &t),
    );

    if render_placeholder(
        f,
        area,
        block.clone(),
        p.loaded,
        &p.error,
        p.visible_count() == 0,
        p.is_filtered(),
        app.lang,
        &t,
    ) {
        return;
    }

    let spin = SPINNER[app.spinner % SPINNER.len()];
    let inner_w = area.width.saturating_sub(2) as usize; // left/right border
    let items: Vec<ListItem> = p
        .filtered
        .iter()
        .map(|&i| {
            let r = &p.items[i];
            run_line(
                r,
                p.highlighted.contains(&r.database_id.to_string()),
                spin,
                &t,
                inner_w,
            )
        })
        .collect();

    let list = focusable_list(items, block, focused, &t);
    f.render_stateful_widget(list, area, &mut app.runs.state);
}

fn run_line(r: &Run, hl: bool, spin: &str, t: &Theme, width: usize) -> ListItem<'static> {
    let (icon, color) = run_status(r, spin, t);
    let marker_s = marker(hl, t);
    let icon_s = Span::styled(format!("{icon} "), Style::new().fg(color));
    let wf_s = Span::styled(
        truncate(&r.workflow_name, 16),
        Style::new().fg(t.text).bold(),
    );
    let gap1 = Span::raw(" ");
    let gap2 = Span::raw("  ");
    let meta_s = meta(
        format!("{} · {} · ", r.event, truncate(&r.head_branch, 12)),
        t,
    );
    let time_s = time_span(&r.created_at, t);
    // Subtract the display width of the fixed spans (excluding the variable title), then allocate the remaining width to the title dynamically.
    let title_w = remaining_width(
        width,
        &[&marker_s, &icon_s, &wf_s, &gap1, &gap2, &meta_s, &time_s],
    );
    let title_s = Span::raw(truncate_width(&r.display_title, title_w));
    ListItem::new(Line::from(vec![
        marker_s, icon_s, wf_s, gap1, title_s, gap2, meta_s, time_s,
    ]))
}

/// Available width minus the display width of fixed spans (for the variable field). Guarantees a minimum of 10 cells.
fn remaining_width(width: usize, fixed: &[&Span]) -> usize {
    let used: usize = fixed.iter().map(|s| display_width(&s.content)).sum();
    width.saturating_sub(used).max(10)
}

fn run_status(r: &Run, spin: &str, t: &Theme) -> (String, Color) {
    match r.status.as_str() {
        "completed" => match r.conclusion.as_deref() {
            Some("success") => ("✓".into(), t.success),
            c if is_failure(c) => ("✗".into(), t.error),
            Some("cancelled") => ("⊘".into(), t.muted),
            Some("skipped") => ("⊝".into(), t.muted),
            Some("action_required") | Some("neutral") => ("!".into(), t.warning),
            _ => ("•".into(), t.muted),
        },
        "in_progress" => (spin.to_string(), t.warning),
        "queued" | "waiting" | "pending" | "requested" => ("◴".into(), t.accent),
        _ => ("•".into(), t.muted),
    }
}

// ─── PRs ────────────────────────────────────────────────────────────────────

fn render_prs(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Prs;
    let t = app.theme;
    let p = &app.prs;
    let block = panel_block(
        "⑃ Pull Requests",
        focused,
        p.items.len(),
        p.visible_count(),
        p.highlight_count(),
        &p.filter,
        &t,
        vec![],
    );

    if render_placeholder(
        f,
        area,
        block.clone(),
        p.loaded,
        &p.error,
        p.visible_count() == 0,
        p.is_filtered(),
        app.lang,
        &t,
    ) {
        return;
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = p
        .filtered
        .iter()
        .map(|&i| {
            let pr = &p.items[i];
            pr_line(
                pr,
                p.highlighted.contains(&pr.number.to_string()),
                &t,
                inner_w,
            )
        })
        .collect();

    let list = focusable_list(items, block, focused, &t);
    f.render_stateful_widget(list, area, &mut app.prs.state);
}

fn pr_line(pr: &Pr, hl: bool, t: &Theme, width: usize) -> ListItem<'static> {
    let (icon, color) = if pr.is_draft {
        ("◑", t.muted)
    } else {
        match pr.review_decision.as_deref() {
            Some("APPROVED") => ("✓", t.success),
            Some("CHANGES_REQUESTED") => ("✗", t.error),
            Some("REVIEW_REQUIRED") => ("◔", t.warning),
            _ => ("◍", t.success),
        }
    };
    let author = pr.author.as_ref().map(|a| a.login.as_str()).unwrap_or("?");
    let marker_s = marker(hl, t);
    let icon_s = Span::styled(format!("{icon} "), Style::new().fg(color));
    let num_s = Span::styled(format!("#{} ", pr.number), Style::new().fg(t.accent));
    let gap = Span::raw("  ");
    let meta_s = meta(
        format!("@{author} ⑃{} · ", truncate(&pr.head_ref_name, 12)),
        t,
    );
    let time_s = time_span(&pr.updated_at, t);
    let title_w = remaining_width(width, &[&marker_s, &icon_s, &num_s, &gap, &meta_s, &time_s]);
    let title_s = Span::raw(truncate_width(&pr.title, title_w));
    ListItem::new(Line::from(vec![
        marker_s, icon_s, num_s, title_s, gap, meta_s, time_s,
    ]))
}

// ─── Issues ─────────────────────────────────────────────────────────────────

fn render_issues(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Issues;
    let t = app.theme;
    let p = &app.issues;
    let block = panel_block(
        "◎ Issues",
        focused,
        p.items.len(),
        p.visible_count(),
        p.highlight_count(),
        &p.filter,
        &t,
        vec![],
    );

    if render_placeholder(
        f,
        area,
        block.clone(),
        p.loaded,
        &p.error,
        p.visible_count() == 0,
        p.is_filtered(),
        app.lang,
        &t,
    ) {
        return;
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = p
        .filtered
        .iter()
        .map(|&i| {
            let is = &p.items[i];
            issue_line(
                is,
                p.highlighted.contains(&is.number.to_string()),
                &t,
                inner_w,
            )
        })
        .collect();

    let list = focusable_list(items, block, focused, &t);
    f.render_stateful_widget(list, area, &mut app.issues.state);
}

// ─── Branches (Bitbucket, in the Issues slot) ───────────────────────────────────────

fn render_branches(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Issues; // reuses the Issues slot as-is
    let t = app.theme;
    let p = &app.branches;
    let block = panel_block(
        "⌥ Branches",
        focused,
        p.items.len(),
        p.visible_count(),
        p.highlight_count(),
        &p.filter,
        &t,
        vec![],
    );

    if render_placeholder(
        f,
        area,
        block.clone(),
        p.loaded,
        &p.error,
        p.visible_count() == 0,
        p.is_filtered(),
        app.lang,
        &t,
    ) {
        return;
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = p
        .filtered
        .iter()
        .map(|&i| {
            let b = &p.items[i];
            branch_line(b, p.highlighted.contains(&b.name), &t, inner_w)
        })
        .collect();

    let list = focusable_list(items, block, focused, &t);
    f.render_stateful_widget(list, area, &mut app.branches.state);
}

fn branch_line(b: &Branch, hl: bool, t: &Theme, width: usize) -> ListItem<'static> {
    // Default branch (main/master) is highlighted with ★, others with ⌥.
    let (icon, color) = if b.is_default {
        ("★", t.highlight)
    } else {
        ("⌥", t.muted)
    };
    let marker_s = marker(hl, t);
    let icon_s = Span::styled(format!("{icon} "), Style::new().fg(color));
    let gap1 = Span::raw("  ");
    let gap2 = Span::raw("  ");
    let author_s = Span::styled(
        format!("@{} · ", truncate(&b.author, 12)),
        Style::new().fg(t.muted),
    );
    let time_s = time_span(&b.updated_at, t);
    // Both branch name and commit message are variable → split the remaining width (name first, yielding to the message if short).
    let avail = remaining_width(
        width,
        &[&marker_s, &icon_s, &gap1, &gap2, &author_s, &time_s],
    );
    let name_w = display_width(&b.name).min((avail * 6 / 10).max(12));
    let msg_w = avail.saturating_sub(name_w).max(8);
    let name_s = Span::styled(
        truncate_width(&b.name, name_w),
        Style::new().fg(t.text).bold(),
    );
    let msg_s = Span::raw(truncate_width(b.summary(), msg_w));
    ListItem::new(Line::from(vec![
        marker_s, icon_s, name_s, gap1, msg_s, gap2, author_s, time_s,
    ]))
}

fn issue_line(is: &Issue, hl: bool, t: &Theme, width: usize) -> ListItem<'static> {
    let author = is.author.as_ref().map(|a| a.login.as_str()).unwrap_or("?");
    let marker_s = marker(hl, t);
    let glyph_s = Span::styled("◎ ", Style::new().fg(t.success));
    let num_s = Span::styled(format!("#{} ", is.number), Style::new().fg(t.accent));
    // Labels as actual-color pills (up to 2) — included in the width calculation.
    let mut label_spans: Vec<Span> = Vec::new();
    for l in is.labels.iter().take(2) {
        label_spans.push(Span::raw(" "));
        label_spans.push(label_pill(&truncate(&l.name, 14), &l.color, t));
    }
    let gap = Span::raw("  ");
    let author_s = Span::styled(format!("@{author} · "), Style::new().fg(t.muted));
    let time_s = time_span(&is.updated_at, t);
    let mut fixed: Vec<&Span> = vec![&marker_s, &glyph_s, &num_s];
    fixed.extend(label_spans.iter());
    fixed.push(&gap);
    fixed.push(&author_s);
    fixed.push(&time_s);
    let title_w = remaining_width(width, &fixed);
    drop(fixed);
    let title_s = Span::raw(truncate_width(&is.title, title_w));
    // Assembly order: glyph, number, title, labels, meta, time.
    let mut spans = vec![marker_s, glyph_s, num_s, title_s];
    spans.extend(label_spans);
    spans.push(gap);
    spans.push(author_s);
    spans.push(time_s);
    ListItem::new(Line::from(spans))
}

// ─── Commits ────────────────────────────────────────────────────────────────

fn render_commits(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Commits;
    let t = app.theme;
    let p = &app.commits;
    let extra = if p.items.is_empty() {
        vec![]
    } else {
        vec![Span::raw(" "), sparkline(&commit_activity(&p.items), &t)]
    };
    let block = panel_block(
        "⎇ Commits",
        focused,
        p.items.len(),
        p.visible_count(),
        p.highlight_count(),
        &p.filter,
        &t,
        extra,
    );

    if render_placeholder(
        f,
        area,
        block.clone(),
        p.loaded,
        &p.error,
        p.visible_count() == 0,
        p.is_filtered(),
        app.lang,
        &t,
    ) {
        return;
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = p
        .filtered
        .iter()
        .map(|&i| {
            let c = &p.items[i];
            commit_line(c, p.highlighted.contains(&c.sha), &t, inner_w)
        })
        .collect();

    let list = focusable_list(items, block, focused, &t);
    f.render_stateful_widget(list, area, &mut app.commits.state);
}

fn commit_line(c: &Commit, hl: bool, t: &Theme, width: usize) -> ListItem<'static> {
    let marker_s = marker(hl, t);
    let sha_s = Span::styled(format!("{} ", c.short_sha()), Style::new().fg(t.sha));
    let gap = Span::raw("  ");
    let meta_s = meta(format!("{} · ", truncate(c.author_name(), 12)), t);
    let time_s = time_span(&c.commit.author.date, t);
    let msg_w = remaining_width(width, &[&marker_s, &sha_s, &gap, &meta_s, &time_s]);
    let msg_s = Span::raw(truncate_width(c.summary(), msg_w));
    ListItem::new(Line::from(vec![
        marker_s, sha_s, msg_s, gap, meta_s, time_s,
    ]))
}

// ─── list helpers ─────────────────────────────────────────────────────────────

/// Shared List builder that turns on selection highlight (background tint + left bar `▌`) when focused.
fn focusable_list<'a>(
    items: Vec<ListItem<'a>>,
    block: Block<'a>,
    focused: bool,
    t: &Theme,
) -> List<'a> {
    let mut list = List::new(items).block(block);
    if focused {
        list = list
            .highlight_style(Style::new().bg(t.sel).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌ ");
    }
    list
}

/// Renders a GitHub label as a pill with an actual-color background. Falls back to highlight if color parsing fails.
fn label_pill(name: &str, hex: &str, t: &Theme) -> Span<'static> {
    match parse_hex(hex) {
        Some((r, g, b)) => {
            // Contrast the foreground black/white by background brightness (relative luminance).
            let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
            let fg = if lum > 140.0 {
                Color::Black
            } else {
                Color::White
            };
            Span::styled(
                format!(" {name} "),
                Style::new().bg(Color::Rgb(r, g, b)).fg(fg),
            )
        }
        None => Span::styled(format!(" {name} "), Style::new().fg(t.highlight)),
    }
}

/// "RRGGBB" (leading `#` allowed) → (r, g, b).
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Ellipsizes if it exceeds the display width. (multibyte handled roughly per char)
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Character display width (CJK/fullwidth count as 2). Approximation for terminal alignment.
fn char_width(c: char) -> usize {
    let u = c as u32;
    if (0x1100..=0x115F).contains(&u)       // Hangul Jamo
        || (0x2E80..=0x303E).contains(&u)   // CJK radicals/symbols
        || (0x3041..=0x33FF).contains(&u)   // Hiragana/Katakana/CJK symbols
        || (0x3400..=0x4DBF).contains(&u)   // CJK Extension A
        || (0x4E00..=0x9FFF).contains(&u)   // CJK Unified Ideographs
        || (0xA000..=0xA4CF).contains(&u)   // Yi
        || (0xAC00..=0xD7A3).contains(&u)   // Hangul Syllables
        || (0xF900..=0xFAFF).contains(&u)   // CJK Compatibility Ideographs
        || (0xFF00..=0xFF60).contains(&u)   // Fullwidth Forms
        || (0xFFE0..=0xFFE6).contains(&u)
    {
        2
    } else {
        1
    }
}

/// Display width of a string (cell count).
fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Cuts by display width, appending … (width 1) at the end if it overflows.
fn truncate_width(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1); // reserve room for …
    let mut w = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::app::App;
    use crate::backend::Backend;
    use crate::github::{Branch, DataMsg, Run};
    use crate::i18n::Lang;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    fn make_app() -> App {
        let (tx, _rx) = mpsc::channel(1);
        let (dtx, _drx) = mpsc::channel(1);
        App::new(
            "owner/repo".into(),
            Backend::Github,
            15,
            Theme::default(),
            Lang::Ko,
            tx,
            dtx,
        )
    }

    fn make_app_bitbucket() -> App {
        let (tx, _rx) = mpsc::channel(1);
        let (dtx, _drx) = mpsc::channel(1);
        App::new(
            "ws/repo".into(),
            Backend::Bitbucket,
            15,
            Theme::default(),
            Lang::Ko,
            tx,
            dtx,
        )
    }

    fn sample_branch(name: &str, default: bool, msg: &str) -> Branch {
        Branch {
            name: name.into(),
            is_default: default,
            commit_message: msg.into(),
            author: "alice".into(),
            updated_at: "2026-06-01T00:00:00Z".into(),
            commit_sha: "abc1234".into(),
            url: "https://bitbucket.org/ws/repo/branch/x".into(),
        }
    }

    fn draw_sized(app: &mut App, w: u16, h: u16) {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
    }

    fn draw(app: &mut App) {
        draw_sized(app, 120, 40);
    }

    #[test]
    fn renders_empty_without_panic() {
        // Render loading state before data arrives.
        draw(&mut make_app());
    }

    #[test]
    fn renders_narrow_1x4_layout_without_panic() {
        // Narrow width → 1×4 vertical stack path.
        draw_sized(&mut make_app(), 48, 40);
    }

    #[test]
    fn renders_wide_short_4x1_layout_without_panic() {
        // Wide enough + short height → 4 columns (4×1) path.
        draw_sized(&mut make_app(), 160, 12);
    }

    #[test]
    fn renders_themed_without_panic() {
        // Render with all preset themes (no panic).
        for name in crate::theme::NAMES {
            let mut app = make_app();
            app.theme = Theme::from_name(name);
            draw(&mut app);
        }
    }

    #[test]
    fn renders_search_mode_without_panic() {
        let mut app = make_app();
        app.search_active = true;
        app.search_buffer = "fix".into();
        app.runs.set_filter("fix".into());
        draw(&mut app);
    }

    #[test]
    fn renders_detail_modal_without_panic() {
        use crate::app::{Detail, Modal};
        use crate::github::DetailKind;
        use ratatui::text::Line;
        // Modal filled with markdown body (including scroll).
        let mut app = make_app();
        app.modal = Modal::Detail(Detail {
            title: "⑃ #1 some pull request with a long title".into(),
            url: "https://github.com/o/r/pull/1".into(),
            lines: crate::markdown::render(
                "# Heading\n\nSome **bold** and `code`.\n\n- item\n- item\n\n> quote\n",
                &Theme::default(),
            ),
            loading: false,
            following: false,
            scroll: 10,
            kind: DetailKind::Pr { number: 1 },
            follow_ticks: 0,
            epoch: 0,
        });
        draw(&mut app);
        // run modal while following (LIVE).
        app.modal = Modal::Detail(Detail {
            title: "⚙ CI · build".into(),
            url: String::new(),
            lines: vec![Line::from("▸ build (in_progress)".to_string())],
            loading: false,
            following: true,
            scroll: 0,
            kind: DetailKind::Run { id: 1, log: false },
            follow_ticks: 0,
            epoch: 0,
        });
        draw(&mut app);
        // loading modal.
        app.modal = Modal::Detail(Detail {
            title: "loading".into(),
            url: String::new(),
            lines: vec![],
            loading: true,
            following: false,
            scroll: 0,
            kind: DetailKind::Issue { number: 1 },
            follow_ticks: 0,
            epoch: 0,
        });
        draw(&mut app);
    }

    #[test]
    fn modal_scroll_clamped() {
        use crate::app::{Detail, Modal};
        use crate::github::DetailKind;
        use ratatui::text::Line;
        let mut app = make_app();
        app.modal = Modal::Detail(Detail {
            title: "t".into(),
            url: "u".into(),
            // short body (3 lines) but an excessive scroll value.
            lines: vec![
                Line::from("a".to_string()),
                Line::from("b".to_string()),
                Line::from("c".to_string()),
            ],
            loading: false,
            following: false,
            scroll: 9999,
            kind: DetailKind::Issue { number: 1 },
            follow_ticks: 0,
            epoch: 0,
        });
        draw(&mut app); // 120×40 → the whole body fits, so scroll clamps to 0.
        match &app.modal {
            Modal::Detail(d) => assert_eq!(d.scroll, 0, "scroll=0 when the body fits on screen"),
            _ => panic!("modal disappeared"),
        }
    }

    #[test]
    fn renders_with_data_without_panic() {
        let mut app = make_app();
        let runs: Vec<Run> = serde_json::from_str(
            r#"[
            {"databaseId":1,"workflowName":"CI","displayTitle":"fix a very long title that must be truncated cleanly","status":"completed","conclusion":"success","headBranch":"main","event":"push","createdAt":"2026-06-12T07:54:25Z","url":"https://github.com/o/r/actions/runs/1"},
            {"databaseId":2,"workflowName":"Deploy","displayTitle":"release","status":"in_progress","conclusion":null,"headBranch":"feature/x","event":"workflow_dispatch","createdAt":"2026-06-12T08:00:00Z","url":"https://github.com/o/r/actions/runs/2"}
        ]"#,
        )
        .unwrap();
        app.apply(DataMsg::Runs(Ok(runs)));
        // A second apply exercises the change-detection (new/changed highlight) path too.
        let runs2: Vec<Run> = serde_json::from_str(
            r#"[{"databaseId":2,"workflowName":"Deploy","displayTitle":"release","status":"completed","conclusion":"failure","headBranch":"feature/x","event":"workflow_dispatch","createdAt":"2026-06-12T08:00:00Z","url":"https://github.com/o/r/actions/runs/2"}]"#,
        )
        .unwrap();
        app.apply(DataMsg::Runs(Ok(runs2)));
        draw(&mut app);
    }

    #[test]
    fn renders_error_state_without_panic() {
        let mut app = make_app();
        app.apply(DataMsg::Prs(Err("gh: not authenticated".into())));
        draw(&mut app);
    }

    #[test]
    fn parse_hex_works() {
        assert_eq!(super::parse_hex("0dd8ac"), Some((0x0d, 0xd8, 0xac)));
        assert_eq!(super::parse_hex("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(super::parse_hex("000000"), Some((0, 0, 0)));
        assert_eq!(super::parse_hex("xyz"), None); // not 6 chars
        assert_eq!(super::parse_hex("12345"), None);
        assert_eq!(super::parse_hex("gggggg"), None); // not hex
    }

    #[test]
    fn ratio_bar_and_sparkline() {
        let t = Theme::default();
        let count_filled = |s: &super::Span| s.content.chars().filter(|&c| c == '█').count();
        assert_eq!(count_filled(&super::ratio_bar(1.0, 8, t.success)), 8);
        assert_eq!(count_filled(&super::ratio_bar(0.5, 8, t.accent)), 4);
        assert_eq!(count_filled(&super::ratio_bar(0.0, 8, t.accent)), 0);

        assert_eq!(
            super::sparkline(&[0, 1, 2, 3], &t).content.chars().count(),
            4
        );
        assert!(super::sparkline(&[], &t).content.is_empty());
    }

    #[test]
    fn width_aware_truncate() {
        // CJK (Hangul) counts as 2 cells.
        assert_eq!(super::display_width("abc"), 3);
        assert_eq!(super::display_width("한글"), 4);
        assert_eq!(super::display_width("a한b"), 4);
        // returned as-is when it fits within the width.
        assert_eq!(super::truncate_width("hello", 10), "hello");
        // when it overflows, cut by display width and append … (width 1).
        assert_eq!(super::truncate_width("hello world", 5), "hell…");
        assert_eq!(super::truncate_width("한글입니다", 5), "한글…");
    }

    #[test]
    fn renders_bitbucket_branches_without_panic() {
        let mut app = make_app_bitbucket();
        app.apply(DataMsg::Branches(Ok(vec![
            sample_branch("master", true, "Merged in feature/x (pull request #1)"),
            sample_branch(
                "feature/long-branch-name-for-truncation",
                false,
                "fix: 한글 커밋 메시지 동적 말줄임 테스트입니다",
            ),
        ])));
        // The Bitbucket backend renders Branches in the Issues slot. It must draw without
        // panic (including dynamic ellipsis width calculation) on both wide and narrow screens.
        draw_sized(&mut app, 120, 40);
        draw_sized(&mut app, 50, 20);
    }

    #[test]
    fn commit_activity_buckets() {
        use crate::github::Commit;
        let mk = |date: &str| -> Commit {
            serde_json::from_str(&format!(
                r#"{{"sha":"a","commit":{{"message":"m","author":{{"name":"n","date":"{date}"}}}},"author":null,"html_url":"u"}}"#
            ))
            .unwrap()
        };
        // empty input → empty vec.
        assert!(super::commit_activity(&[]).is_empty());
        // multiple commits → 14 cells, sum = commit count.
        let commits = vec![
            mk("2026-06-12T08:00:00Z"),
            mk("2026-06-11T08:00:00Z"),
            mk("2026-06-10T08:00:00Z"),
        ];
        let acts = super::commit_activity(&commits);
        assert_eq!(acts.len(), 14);
        assert_eq!(acts.iter().sum::<u64>(), 3);
    }

    #[test]
    fn completion_bar_counts_status_and_failures() {
        use crate::github::Run;
        let t = Theme::default();
        // status feeds the completion rate, conclusion feeds the failure badge. in-progress/queued have null conclusion.
        let mk = |status: &str, concl: &str| -> Run {
            let c = if concl.is_empty() {
                "null".to_string()
            } else {
                format!("\"{concl}\"")
            };
            serde_json::from_str(&format!(
                r#"{{"databaseId":1,"workflowName":"w","displayTitle":"d","status":"{status}","conclusion":{c},"headBranch":"main","event":"push","createdAt":"2026-06-12T08:00:00Z","url":"u"}}"#
            ))
            .unwrap()
        };
        let text = |runs: &[Run]| -> String {
            super::run_completion_bar(runs, &t)
                .iter()
                .map(|s| s.content.to_string())
                .collect()
        };

        // 8 completed (success6, failure1, skipped1) + 1 in-progress + 1 queued → 8/10 = 80%, 1 failure.
        let runs = vec![
            mk("completed", "success"),
            mk("completed", "success"),
            mk("completed", "success"),
            mk("completed", "success"),
            mk("completed", "success"),
            mk("completed", "success"),
            mk("completed", "failure"),
            mk("completed", "skipped"),
            mk("in_progress", ""),
            mk("queued", ""),
        ];
        let out = text(&runs);
        assert!(out.contains("80%"), "got: {out}");
        assert!(out.contains("✗1"), "got: {out}");

        // all completed + no failures → 100%, no badge (skipped/cancelled also count as completed).
        let all_done = vec![
            mk("completed", "success"),
            mk("completed", "skipped"),
            mk("completed", "cancelled"),
        ];
        let out = text(&all_done);
        assert!(out.contains("100%"), "got: {out}");
        assert!(!out.contains("✗"), "got: {out}");

        // no bar is drawn when there are no runs.
        assert!(super::run_completion_bar(&[], &t).is_empty());
    }

    #[test]
    fn renders_commit_sparkline_without_panic() {
        use crate::github::Commit;
        let mut app = make_app();
        let commits: Vec<Commit> = serde_json::from_str(
            r#"[{"sha":"abc1234def","commit":{"message":"fix bug","author":{"name":"kim","date":"2026-06-12T08:00:00Z"}},"author":null,"html_url":"https://github.com/o/r/commit/abc1234def"}]"#,
        )
        .unwrap();
        app.apply(DataMsg::Commits(Ok(commits)));
        draw(&mut app);
    }

    #[test]
    fn renders_issue_label_pills_without_panic() {
        use crate::github::Issue;
        let mut app = make_app();
        let issues: Vec<Issue> = serde_json::from_str(
            r#"[
            {"number":99,"title":"crash on startup","author":{"login":"kim"},"updatedAt":"2026-06-12T08:00:00Z","labels":[{"name":"bug","color":"d73a4a"},{"name":"good first issue","color":"7057ff"}],"url":"https://github.com/o/r/issues/99"},
            {"number":98,"title":"no color label","author":{"login":"lee"},"updatedAt":"2026-06-10T08:00:00Z","labels":[{"name":"weird","color":""}],"url":"https://github.com/o/r/issues/98"}
        ]"#,
        )
        .unwrap();
        app.apply(DataMsg::Issues(Ok(issues)));
        draw(&mut app);
    }
}
