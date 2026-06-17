//! App state, per-panel data storage, new/changed detection, key input handling.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

use crate::backend::Backend;
use crate::github::{
    Branch, Commit, DataMsg, DetailKind, DetailReq, Entry, FetchCmd, Issue, Pr, Run,
};
use crate::i18n::{self, Lang};
use crate::theme::Theme;

/// follow (auto-refresh of in-progress run) polling interval — in tick units (≈120ms × 16 ≈ 2s).
const FOLLOW_INTERVAL_TICKS: u32 = 16;

/// Polling interval steps (seconds) cycled by `+`/`-`.
const INTERVAL_STEPS: [u64; 9] = [2, 3, 5, 10, 15, 30, 60, 120, 300];

/// Panel identifiers in the 2×2 grid. The order is also the Tab cycle order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Actions,
    Prs,
    Issues,
    Commits,
}

impl Panel {
    pub fn next(self) -> Self {
        match self {
            Panel::Actions => Panel::Prs,
            Panel::Prs => Panel::Issues,
            Panel::Issues => Panel::Commits,
            Panel::Commits => Panel::Actions,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Panel::Actions => Panel::Commits,
            Panel::Prs => Panel::Actions,
            Panel::Issues => Panel::Prs,
            Panel::Commits => Panel::Issues,
        }
    }

    // Spatial moves on the 2×2 grid (stays put at the edges).
    //   Actions  Prs
    //   Issues   Commits
    pub fn right(self) -> Self {
        match self {
            Panel::Actions => Panel::Prs,
            Panel::Issues => Panel::Commits,
            other => other,
        }
    }
    pub fn left(self) -> Self {
        match self {
            Panel::Prs => Panel::Actions,
            Panel::Commits => Panel::Issues,
            other => other,
        }
    }
    pub fn down(self) -> Self {
        match self {
            Panel::Actions => Panel::Issues,
            Panel::Prs => Panel::Commits,
            other => other,
        }
    }
    pub fn up(self) -> Self {
        match self {
            Panel::Issues => Panel::Actions,
            Panel::Commits => Panel::Prs,
            other => other,
        }
    }
}

/// Current panel layout — used to interpret Shift+arrow moves (ui updates it on render).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutKind {
    /// 2×2 grid — arrow keys move spatially (up/down/left/right).
    Grid,
    /// Horizontal 4 columns (4×1) — only left/right matter, up/down ignored.
    Cols,
    /// Vertical 4 rows (1×4) — only up/down matter, left/right ignored.
    Rows,
}

/// Tracks which keys are new/changed relative to the previous fetch.
#[derive(Default)]
struct Tracker {
    sigs: HashMap<String, String>,
    initialized: bool,
}

impl Tracker {
    /// Takes the new item list and returns the set of new (previously absent) and changed
    /// (differing signature) keys.
    /// The first load only records the baseline and highlights nothing (avoids flashing everything).
    fn diff(&mut self, items: &[(String, String)]) -> HashSet<String> {
        let mut changed = HashSet::new();
        let new_map: HashMap<String, String> = items.iter().cloned().collect();

        if self.initialized {
            for (k, sig) in items {
                match self.sigs.get(k) {
                    None => {
                        changed.insert(k.clone());
                    }
                    Some(old) if old != sig => {
                        changed.insert(k.clone());
                    }
                    _ => {}
                }
            }
        } else {
            self.initialized = true;
        }

        self.sigs = new_map;
        changed
    }
}

/// Bundles one panel's data, filter, scroll state, error, and highlighted keys.
pub struct PanelData<T> {
    pub items: Vec<T>,
    /// Indices into `items` that pass the filter (all indices if no filter).
    pub filtered: Vec<usize>,
    /// Search filter string. Empty string means no filter.
    pub filter: String,
    pub highlighted: HashSet<String>,
    tracker: Tracker,
    pub state: ListState,
    pub error: Option<String>,
    pub loaded: bool,
}

impl<T: Entry> PanelData<T> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            filtered: Vec::new(),
            filter: String::new(),
            highlighted: HashSet::new(),
            tracker: Tracker::default(),
            state: ListState::default(),
            error: None,
            loaded: false,
        }
    }

    /// Updates with new data, computes new/changed keys, then re-applies the filter.
    fn update(&mut self, items: Vec<T>) {
        let pairs: Vec<(String, String)> = items.iter().map(|i| (i.key(), i.signature())).collect();
        self.highlighted = self.tracker.diff(&pairs);
        self.items = items;
        self.loaded = true;
        self.error = None;
        self.recompute_filter();
    }

    fn fail(&mut self, e: String) {
        self.error = Some(e);
        self.loaded = true;
    }

    /// Sets the search filter and recomputes the visible list.
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.recompute_filter();
    }

    /// Rebuilds the `filtered` indices to match `filter` (case-insensitive substring).
    fn recompute_filter(&mut self) {
        let q = self.filter.to_lowercase();
        self.filtered = if q.is_empty() {
            (0..self.items.len()).collect()
        } else {
            (0..self.items.len())
                .filter(|&i| self.items[i].search_text().contains(&q))
                .collect()
        };
        self.clamp_selection();
    }

    /// Clamps the selection index into the visible list range.
    fn clamp_selection(&mut self) {
        if self.filtered.is_empty() {
            self.state.select(None);
        } else {
            let last = self.filtered.len() - 1;
            let cur = self.state.selected().unwrap_or(0).min(last);
            self.state.select(Some(cur));
        }
    }

    /// Scrolls one step up/down in the focused panel (based on the visible list).
    pub fn scroll(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() - 1;
        let cur = self.state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, last as isize) as usize;
        self.state.select(Some(next));
    }

    /// Number of items marked as new/changed.
    pub fn highlight_count(&self) -> usize {
        self.highlighted.len()
    }

    /// Whether a filter is active.
    pub fn is_filtered(&self) -> bool {
        !self.filter.is_empty()
    }

    /// Number of visible (filter-passing) items.
    pub fn visible_count(&self) -> usize {
        self.filtered.len()
    }

    /// Reference to the `pos`-th item in the visible list.
    pub fn visible_item(&self, pos: usize) -> Option<&T> {
        let &idx = self.filtered.get(pos)?;
        self.items.get(idx)
    }

    /// Currently selected item (first item if none selected, None if empty).
    pub fn selected_item(&self) -> Option<&T> {
        self.visible_item(self.state.selected().unwrap_or(0))
    }
}

/// Header spinner animation frames (braille).
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Detail preview modal state.
pub struct Detail {
    pub title: String,
    pub url: String,
    /// Rendered body lines (markdown or plain).
    pub lines: Vec<Line<'static>>,
    pub loading: bool,
    /// Whether the run is in progress and being auto-refreshed (follow).
    pub following: bool,
    pub scroll: u16,
    /// For follow re-requests — which item.
    pub kind: DetailKind,
    pub follow_ticks: u32,
    /// Generation of this modal's content. Responses with a different epoch are ignored.
    pub epoch: u64,
}

impl Detail {
    fn loading(title: String, url: String, kind: DetailKind, epoch: u64) -> Self {
        Self {
            title,
            url,
            lines: Vec::new(),
            loading: true,
            following: false,
            scroll: 0,
            kind,
            follow_ticks: 0,
            epoch,
        }
    }
}

/// Converts a detail body string into plain lines (not markdown).
fn plain_lines(s: &str) -> Vec<Line<'static>> {
    s.lines().map(|l| Line::from(l.to_string())).collect()
}

/// The currently displayed overlay.
pub enum Modal {
    None,
    Detail(Detail),
}

/// Item selected in the focused panel — browser URL and detail request info.
struct Selection {
    url: String,
    title: String,
    kind: DetailKind,
}

pub struct App {
    pub repo: String,
    pub runs: PanelData<Run>,
    pub prs: PanelData<Pr>,
    pub issues: PanelData<Issue>,
    /// Bitbucket-only — active branches shown in the Issues slot on Bitbucket (Issues deprecated).
    pub branches: PanelData<Branch>,
    pub commits: PanelData<Commit>,
    pub focus: Panel,
    /// Data source backend — used to split the 4th panel into Issues/Branches.
    pub backend: Backend,
    /// Current panel layout (ui updates it on render) — for interpreting arrow moves.
    pub layout: LayoutKind,
    pub last_refresh: Option<DateTime<Local>>,
    pub poll_secs: u64,
    pub spinner: usize,
    pub should_quit: bool,
    pub modal: Modal,
    /// Color theme.
    pub theme: Theme,
    /// UI language.
    pub lang: Lang,
    /// Newer argus version available (set by the update notifier), shown in the header.
    pub update: Option<String>,
    /// Whether search input mode is active (targets the focused panel).
    pub search_active: bool,
    /// Search input buffer.
    pub search_buffer: String,
    /// Modal generation counter — incremented each time a modal opens or its content switches.
    detail_epoch: u64,
    refresh_tx: mpsc::Sender<FetchCmd>,
    detail_tx: mpsc::Sender<DetailReq>,
}

impl App {
    pub fn new(
        repo: String,
        backend: Backend,
        poll_secs: u64,
        theme: Theme,
        lang: Lang,
        refresh_tx: mpsc::Sender<FetchCmd>,
        detail_tx: mpsc::Sender<DetailReq>,
    ) -> Self {
        Self {
            repo,
            runs: PanelData::new(),
            prs: PanelData::new(),
            issues: PanelData::new(),
            branches: PanelData::new(),
            commits: PanelData::new(),
            focus: Panel::Actions,
            backend,
            layout: LayoutKind::Grid,
            last_refresh: None,
            poll_secs,
            spinner: 0,
            should_quit: false,
            modal: Modal::None,
            theme,
            lang,
            update: None,
            search_active: false,
            search_buffer: String::new(),
            detail_epoch: 0,
            refresh_tx,
            detail_tx,
        }
    }

    /// Applies a data message sent by the background fetcher.
    pub fn apply(&mut self, msg: DataMsg) {
        // Detail bodies are applied only to the modal, independent of the polling timestamp.
        // But only when the response epoch matches the current modal generation (ignore stale responses from a previous modal).
        if let DataMsg::Detail { epoch, result } = msg {
            let theme = self.theme; // Copy before borrowing the modal (disjoint field).
            let lang = self.lang;
            if let Modal::Detail(d) = &mut self.modal {
                if d.epoch != epoch {
                    return;
                }
                match result {
                    Ok(payload) => {
                        d.lines = if payload.markdown {
                            crate::markdown::render(&payload.body, &theme)
                        } else {
                            plain_lines(&payload.body)
                        };
                        d.following = payload.run_active;
                    }
                    Err(e) => {
                        d.lines = plain_lines(&i18n::load_failed(lang, e));
                        d.following = false;
                    }
                }
                d.loading = false;
            }
            return;
        }

        // Update notification — set the banner flag, don't touch refresh state.
        if let DataMsg::Update(v) = msg {
            self.update = Some(v);
            return;
        }

        match msg {
            DataMsg::Runs(Ok(v)) => self.runs.update(v),
            DataMsg::Runs(Err(e)) => self.runs.fail(e),
            DataMsg::Prs(Ok(v)) => self.prs.update(v),
            DataMsg::Prs(Err(e)) => self.prs.fail(e),
            DataMsg::Issues(Ok(v)) => self.issues.update(v),
            DataMsg::Issues(Err(e)) => self.issues.fail(e),
            DataMsg::Branches(Ok(v)) => self.branches.update(v),
            DataMsg::Branches(Err(e)) => self.branches.fail(e),
            DataMsg::Commits(Ok(v)) => self.commits.update(v),
            DataMsg::Commits(Err(e)) => self.commits.fail(e),
            DataMsg::Detail { .. } | DataMsg::Update(_) => unreachable!(),
        }
        self.last_refresh = Some(Local::now());
    }

    /// Called on every timer tick — advances the spinner + periodically re-requests follow-mode runs.
    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);

        let refetch = if let Modal::Detail(d) = &mut self.modal {
            if d.following && !d.loading {
                d.follow_ticks += 1;
                if d.follow_ticks >= FOLLOW_INTERVAL_TICKS {
                    d.follow_ticks = 0;
                    Some((d.kind.clone(), d.epoch))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some((kind, epoch)) = refetch {
            let _ = self.detail_tx.try_send(DetailReq {
                repo: self.repo.clone(),
                kind,
                epoch,
            });
        }
    }

    /// Whether any workflow run is in progress (condition for showing the header spinner).
    pub fn has_active_runs(&self) -> bool {
        // `waiting` is a manual/approval gate (an indefinite pause), not active work — exclude it so
        // a long-paused run doesn't keep the header spinner going forever.
        self.runs
            .items
            .iter()
            .any(|r| r.status != "completed" && r.status != "waiting")
    }

    /// Handles terminal events such as key presses/resize.
    pub fn handle_event(&mut self, ev: Event) {
        if let Event::Key(key) = ev {
            // Prevent duplicate Release events on Windows.
            if key.kind == KeyEventKind::Release {
                return;
            }
            self.handle_key(key);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // When a modal is open, handle only modal-specific keys.
        if let Modal::Detail(d) = &mut self.modal {
            match key.code {
                // Close with ← / Esc / q / v.
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('v') | KeyCode::Left => {
                    self.modal = Modal::None;
                }
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => d.scroll = d.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => d.scroll = d.scroll.saturating_add(15),
                KeyCode::PageUp | KeyCode::Char('b') => d.scroll = d.scroll.saturating_sub(15),
                KeyCode::Char('g') | KeyCode::Home => d.scroll = 0,
                // To the bottom — the actual upper bound is clamped to the wrapped line count at render time.
                KeyCode::Char('G') | KeyCode::End => d.scroll = u16::MAX,
                // In run detail, toggle tree ↔ full per-step logs (l = log).
                KeyCode::Char('l') => {
                    if let DetailKind::Run { id, log } = &d.kind {
                        self.detail_epoch += 1;
                        let epoch = self.detail_epoch;
                        let kind = DetailKind::Run {
                            id: *id,
                            log: !*log,
                        };
                        d.kind = kind.clone();
                        d.epoch = epoch;
                        d.loading = true;
                        d.scroll = 0;
                        let _ = self.detail_tx.try_send(DetailReq {
                            repo: self.repo.clone(),
                            kind,
                            epoch,
                        });
                    }
                }
                KeyCode::Enter | KeyCode::Char('o') => open_in_browser(&d.url),
                _ => {}
            }
            return;
        }

        // Search input mode — applies a live filter to the focused panel.
        if self.search_active {
            match key.code {
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Esc => {
                    // Cancel search: clear the filter and exit input.
                    self.search_buffer.clear();
                    self.apply_search();
                    self.search_active = false;
                }
                KeyCode::Enter => self.search_active = false, // Confirm (keep filter)
                KeyCode::Backspace => {
                    self.search_buffer.pop();
                    self.apply_search();
                }
                KeyCode::Char(c) => {
                    self.search_buffer.push(c);
                    self.apply_search();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Tab | KeyCode::Char('l') => self.focus = self.focus.next(),
            KeyCode::BackTab | KeyCode::Char('h') => self.focus = self.focus.prev(),
            // Shift+arrow — move panels according to the current layout.
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down if shift => {
                self.shift_move(key.code)
            }
            KeyCode::Down | KeyCode::Char('j') => self.focused_panel_scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.focused_panel_scroll(-1),
            KeyCode::Char('r') => self.request_refresh(),
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_interval(1),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_interval(-1),
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(),
            // Open the preview with v or →.
            KeyCode::Char('v') | KeyCode::Right => self.open_detail(),
            _ => {}
        }
    }

    /// Starts search input for the focused panel (continues editing the existing filter).
    fn start_search(&mut self) {
        self.search_buffer = self.focused_filter();
        self.search_active = true;
    }

    fn focused_filter(&self) -> String {
        match self.focus {
            Panel::Actions => self.runs.filter.clone(),
            Panel::Prs => self.prs.filter.clone(),
            // On Bitbucket the Branches panel is in the Issues slot.
            Panel::Issues if self.backend == Backend::Bitbucket => self.branches.filter.clone(),
            Panel::Issues => self.issues.filter.clone(),
            Panel::Commits => self.commits.filter.clone(),
        }
    }

    /// Applies the current input buffer as the focused panel's filter.
    fn apply_search(&mut self) {
        let q = self.search_buffer.clone();
        match self.focus {
            Panel::Actions => self.runs.set_filter(q),
            Panel::Prs => self.prs.set_filter(q),
            Panel::Issues if self.backend == Backend::Bitbucket => self.branches.set_filter(q),
            Panel::Issues => self.issues.set_filter(q),
            Panel::Commits => self.commits.set_filter(q),
        }
    }

    fn focused_panel_scroll(&mut self, delta: isize) {
        match self.focus {
            Panel::Actions => self.runs.scroll(delta),
            Panel::Prs => self.prs.scroll(delta),
            Panel::Issues if self.backend == Backend::Bitbucket => self.branches.scroll(delta),
            Panel::Issues => self.issues.scroll(delta),
            Panel::Commits => self.commits.scroll(delta),
        }
    }

    /// Interprets Shift+arrow moves according to the current layout.
    /// - Grid (2×2): spatial up/down/left/right moves
    /// - Cols (4×1, horizontal): left/right only (up/down ignored)
    /// - Rows (1×4, vertical): up/down only (left/right ignored)
    fn shift_move(&mut self, code: KeyCode) {
        use LayoutKind::{Cols, Grid, Rows};
        self.focus = match (self.layout, code) {
            (Grid, KeyCode::Left) => self.focus.left(),
            (Grid, KeyCode::Right) => self.focus.right(),
            (Grid, KeyCode::Up) => self.focus.up(),
            (Grid, KeyCode::Down) => self.focus.down(),
            (Cols, KeyCode::Left) => self.focus.prev(),
            (Cols, KeyCode::Right) => self.focus.next(),
            (Rows, KeyCode::Up) => self.focus.prev(),
            (Rows, KeyCode::Down) => self.focus.next(),
            _ => self.focus, // Cols up/down / Rows left/right are ignored
        };
    }

    /// Sends an immediate refresh signal to the fetcher (ignored if the channel is full).
    fn request_refresh(&mut self) {
        let _ = self.refresh_tx.try_send(FetchCmd::RefreshNow);
    }

    /// Adjusts the polling interval by one step (dir>0: slower/longer, dir<0: faster/shorter).
    /// On change, passes the new interval to the fetcher and triggers an immediate refresh.
    fn adjust_interval(&mut self, dir: i32) {
        let idx = INTERVAL_STEPS
            .iter()
            .position(|&v| v >= self.poll_secs)
            .unwrap_or(INTERVAL_STEPS.len() - 1);
        let new_idx = if dir > 0 {
            (idx + 1).min(INTERVAL_STEPS.len() - 1)
        } else {
            idx.saturating_sub(1)
        };
        let new_secs = INTERVAL_STEPS[new_idx];
        if new_secs != self.poll_secs {
            self.poll_secs = new_secs;
            let _ = self.refresh_tx.try_send(FetchCmd::SetInterval(new_secs));
        }
    }

    /// Extracts the URL and detail info of the item selected in the focused panel.
    fn current_selection(&self) -> Option<Selection> {
        match self.focus {
            Panel::Actions => {
                let r = self.runs.selected_item()?;
                Some(Selection {
                    url: r.url.clone(),
                    title: format!("⚙ {} · {}", r.workflow_name, r.display_title),
                    kind: DetailKind::Run {
                        id: r.database_id,
                        log: false,
                    },
                })
            }
            Panel::Prs => {
                let p = self.prs.selected_item()?;
                Some(Selection {
                    url: p.url.clone(),
                    title: format!("⑃ #{} {}", p.number, p.title),
                    kind: DetailKind::Pr { number: p.number },
                })
            }
            // On Bitbucket, Branches in the Issues slot — detail is the branch's last commit diff.
            Panel::Issues if self.backend == Backend::Bitbucket => {
                let b = self.branches.selected_item()?;
                Some(Selection {
                    url: b.url.clone(),
                    title: format!("⌥ {}", b.name),
                    kind: DetailKind::Commit {
                        sha: b.commit_sha.clone(),
                    },
                })
            }
            Panel::Issues => {
                let i = self.issues.selected_item()?;
                Some(Selection {
                    url: i.url.clone(),
                    title: format!("◎ #{} {}", i.number, i.title),
                    kind: DetailKind::Issue { number: i.number },
                })
            }
            Panel::Commits => {
                let c = self.commits.selected_item()?;
                Some(Selection {
                    url: c.html_url.clone(),
                    title: format!("⎇ {} {}", c.short_sha(), c.summary()),
                    kind: DetailKind::Commit { sha: c.sha.clone() },
                })
            }
        }
    }

    /// Opens the selected item in the default browser.
    fn open_selected(&mut self) {
        if let Some(sel) = self.current_selection() {
            open_in_browser(&sel.url);
        }
    }

    /// Opens the detail preview modal for the selected item and requests an async fetch.
    fn open_detail(&mut self) {
        if let Some(sel) = self.current_selection() {
            self.detail_epoch += 1;
            let epoch = self.detail_epoch;
            let kind = sel.kind.clone();
            self.modal = Modal::Detail(Detail::loading(sel.title, sel.url, sel.kind, epoch));
            let _ = self.detail_tx.try_send(DetailReq {
                repo: self.repo.clone(),
                kind,
                epoch,
            });
        }
    }
}

/// Opens a URL with the OS default handler (failures silently ignored — to avoid breaking the TUI).
fn open_in_browser(url: &str) {
    // Do nothing if the URL is empty because the backend gave no link (avoids running the handler with an empty arg).
    if url.is_empty() {
        return;
    }

    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    let _ = std::process::Command::new(program)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{DataMsg, DetailKind, DetailPayload, Run};

    fn make_app() -> App {
        let (tx, _r) = mpsc::channel(1);
        let (dtx, _d) = mpsc::channel(1);
        App::new(
            "o/r".into(),
            Backend::Github,
            15,
            Theme::default(),
            Lang::Ko,
            tx,
            dtx,
        )
    }

    fn detail(epoch: u64, kind: DetailKind) -> Detail {
        Detail {
            title: "t".into(),
            url: "u".into(),
            lines: vec![],
            loading: true,
            following: false,
            scroll: 0,
            kind,
            follow_ticks: 0,
            epoch,
        }
    }

    /// Between a run-log modal (epoch 5) being closed and an Issue modal being opened, a follow
    /// response from the previous generation (epoch 3) must not overwrite the current modal.
    #[test]
    fn stale_detail_response_is_ignored() {
        let mut app = make_app();
        app.modal = Modal::Detail(detail(5, DetailKind::Issue { number: 1 }));

        app.apply(DataMsg::Detail {
            epoch: 3,
            result: Ok(DetailPayload {
                body: "STALE RUN LOG".into(),
                markdown: false,
                run_active: true,
            }),
        });
        match &app.modal {
            Modal::Detail(d) => {
                assert!(d.loading, "loading must not clear on a stale response");
                assert!(
                    d.lines.is_empty(),
                    "a stale response must not fill the body"
                );
                assert!(!d.following, "a stale run_active must not be applied");
            }
            _ => panic!("modal disappeared"),
        }

        // Current-generation (epoch 5) responses are applied normally.
        app.apply(DataMsg::Detail {
            epoch: 5,
            result: Ok(DetailPayload {
                body: "line1\nline2".into(),
                markdown: false,
                run_active: false,
            }),
        });
        match &app.modal {
            Modal::Detail(d) => {
                assert!(!d.loading);
                assert_eq!(d.lines.len(), 2);
            }
            _ => panic!(),
        }
    }

    /// A detail response arriving while the modal is closed is silently discarded.
    #[test]
    fn detail_for_closed_modal_is_dropped() {
        let mut app = make_app();
        app.apply(DataMsg::Detail {
            epoch: 1,
            result: Ok(DetailPayload {
                body: "x".into(),
                markdown: false,
                run_active: false,
            }),
        });
        assert!(matches!(app.modal, Modal::None));
    }

    fn run_json(id: u64, wf: &str, title: &str, conclusion: &str) -> String {
        format!(
            r#"{{"databaseId":{id},"workflowName":"{wf}","displayTitle":"{title}","status":"completed","conclusion":"{conclusion}","headBranch":"main","event":"push","createdAt":"2026-06-12T07:54:25Z","url":"u{id}"}}"#
        )
    }

    #[test]
    fn search_filter_narrows_panel() {
        let mut app = make_app();
        let json = format!(
            "[{},{}]",
            run_json(1, "CI", "fix login", "failure"),
            run_json(2, "Deploy", "ship it", "success"),
        );
        let runs: Vec<Run> = serde_json::from_str(&json).unwrap();
        app.apply(DataMsg::Runs(Ok(runs)));
        assert_eq!(app.runs.visible_count(), 2);

        // Search by status (conclusion).
        app.runs.set_filter("failure".into());
        assert_eq!(app.runs.visible_count(), 1);
        assert_eq!(app.runs.selected_item().unwrap().database_id, 1);

        // Case-insensitive + workflow name matching.
        app.runs.set_filter("DEPLOY".into());
        assert_eq!(app.runs.visible_count(), 1);
        assert_eq!(app.runs.selected_item().unwrap().database_id, 2);

        // Empty filter → all.
        app.runs.set_filter(String::new());
        assert_eq!(app.runs.visible_count(), 2);

        // No matches → 0 items, no selection.
        app.runs.set_filter("zzz".into());
        assert_eq!(app.runs.visible_count(), 0);
        assert!(app.runs.selected_item().is_none());
    }

    #[test]
    fn interval_adjusts_through_steps() {
        let mut app = make_app(); // default 15s
        assert_eq!(app.poll_secs, 15);

        app.adjust_interval(1);
        assert_eq!(app.poll_secs, 30);
        app.adjust_interval(-1);
        assert_eq!(app.poll_secs, 15);
        app.adjust_interval(-1);
        assert_eq!(app.poll_secs, 10);

        // Does not go below the lower bound (2s).
        for _ in 0..10 {
            app.adjust_interval(-1);
        }
        assert_eq!(app.poll_secs, 2);

        // Does not go above the upper bound (300s).
        for _ in 0..20 {
            app.adjust_interval(1);
        }
        assert_eq!(app.poll_secs, 300);
    }

    #[test]
    fn filter_persists_across_refresh() {
        let mut app = make_app();
        app.runs.set_filter("ci".into());
        let json = format!(
            "[{},{},{}]",
            run_json(1, "CI", "a", "failure"),
            run_json(3, "CI", "b", "success"),
            run_json(2, "Deploy", "c", "success"),
        );
        let runs: Vec<Run> = serde_json::from_str(&json).unwrap();
        app.apply(DataMsg::Runs(Ok(runs)));
        // The filter persists across refresh, so only the 2 CI items are visible.
        assert_eq!(app.runs.visible_count(), 2);
        assert!(app.runs.is_filtered());
    }

    #[test]
    fn panel_spatial_moves() {
        // 2×2 spatial moves + stays put at the edges.
        assert_eq!(Panel::Actions.right(), Panel::Prs);
        assert_eq!(Panel::Actions.down(), Panel::Issues);
        assert_eq!(Panel::Commits.left(), Panel::Issues);
        assert_eq!(Panel::Commits.up(), Panel::Prs);
        assert_eq!(Panel::Prs.right(), Panel::Prs); // right edge → stays put
        assert_eq!(Panel::Actions.up(), Panel::Actions); // top edge → stays put
    }

    #[test]
    fn shift_move_depends_on_layout() {
        use ratatui::crossterm::event::KeyCode;
        let mut app = make_app();

        // Grid (2×2): spatial moves in the arrow direction.
        app.layout = LayoutKind::Grid;
        app.focus = Panel::Actions;
        app.shift_move(KeyCode::Right);
        assert_eq!(app.focus, Panel::Prs);
        app.shift_move(KeyCode::Down);
        assert_eq!(app.focus, Panel::Commits);

        // Cols (horizontal 4 columns): left/right only, up/down ignored.
        app.layout = LayoutKind::Cols;
        app.focus = Panel::Actions;
        app.shift_move(KeyCode::Down); // ignored
        assert_eq!(app.focus, Panel::Actions);
        app.shift_move(KeyCode::Right); // next
        assert_eq!(app.focus, Panel::Prs);

        // Rows (vertical 4 rows): up/down only, left/right ignored.
        app.layout = LayoutKind::Rows;
        app.focus = Panel::Actions;
        app.shift_move(KeyCode::Right); // ignored
        assert_eq!(app.focus, Panel::Actions);
        app.shift_move(KeyCode::Down); // next
        assert_eq!(app.focus, Panel::Prs);
    }
}
