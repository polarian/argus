//! Layer that fetches GitHub repo data by invoking the `gh` CLI.
//!
//! Every fetch runs a `gh` subprocess async and parses `--json` output with serde.
//! Auth/network are fully delegated to `gh`, so no token management is needed.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::backend::{self, Backend};
use crate::bitbucket;
use crate::i18n::{self, Lang};

/// Provides metadata for change detection (new/changed) and search of a panel item.
pub trait Entry {
    /// Key that uniquely identifies the item (run id, issue number, commit sha, etc.).
    fn key(&self) -> String;
    /// Signature that detects content changes. A different value counts as "changed".
    fn signature(&self) -> String;
    /// Substring-search target text (lowercased). Concatenates all searchable fields.
    fn search_text(&self) -> String;
}

// ─── Actions (workflow runs) ────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub database_id: u64,
    pub workflow_name: String,
    pub display_title: String,
    /// queued · in_progress · completed, etc.
    pub status: String,
    /// success · failure · cancelled · skipped … (None while in progress)
    pub conclusion: Option<String>,
    pub head_branch: String,
    pub event: String,
    pub created_at: String,
    pub url: String,
}

impl Entry for Run {
    fn key(&self) -> String {
        self.database_id.to_string()
    }
    fn signature(&self) -> String {
        format!(
            "{}:{}",
            self.status,
            self.conclusion.as_deref().unwrap_or("")
        )
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.workflow_name,
            self.display_title,
            self.status,
            self.conclusion.as_deref().unwrap_or(""),
            self.head_branch,
            self.event,
        )
        .to_lowercase()
    }
}

// ─── Issues ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
    /// GitHub label color (6-digit hex without #). default since some APIs may omit it.
    #[serde(default)]
    pub color: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: Option<Author>,
    pub updated_at: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub url: String,
}

impl Entry for Issue {
    fn key(&self) -> String {
        self.number.to_string()
    }
    fn signature(&self) -> String {
        self.updated_at.clone()
    }
    fn search_text(&self) -> String {
        let author = self.author.as_ref().map(|a| a.login.as_str()).unwrap_or("");
        let labels = self
            .labels
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        format!("#{} {} {} {}", self.number, self.title, author, labels).to_lowercase()
    }
}

// ─── Pull Requests ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub author: Option<Author>,
    pub updated_at: String,
    pub is_draft: bool,
    /// APPROVED · CHANGES_REQUESTED · REVIEW_REQUIRED … (None if absent)
    pub review_decision: Option<String>,
    pub head_ref_name: String,
    pub url: String,
}

impl Entry for Pr {
    fn key(&self) -> String {
        self.number.to_string()
    }
    fn signature(&self) -> String {
        format!(
            "{}:{}:{}",
            self.updated_at,
            self.is_draft,
            self.review_decision.as_deref().unwrap_or("")
        )
    }
    fn search_text(&self) -> String {
        let author = self.author.as_ref().map(|a| a.login.as_str()).unwrap_or("");
        let draft = if self.is_draft { "draft" } else { "" };
        format!(
            "#{} {} {} {} {} {}",
            self.number,
            self.title,
            author,
            self.head_ref_name,
            self.review_decision.as_deref().unwrap_or(""),
            draft,
        )
        .to_lowercase()
    }
}

// ─── Commits (gh api) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitDetail,
    /// GitHub user (None if unmapped) — `commit.author` is the raw git author.
    pub author: Option<Author>,
    pub html_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitDetail {
    pub message: String,
    pub author: CommitAuthor,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitAuthor {
    pub name: String,
    pub date: String,
}

impl Commit {
    /// First line (subject) of the commit message only.
    pub fn summary(&self) -> &str {
        self.commit.message.lines().next().unwrap_or("")
    }
    /// 7-char short sha for display.
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
    /// GitHub login if available, otherwise git author name.
    pub fn author_name(&self) -> &str {
        self.author
            .as_ref()
            .map(|a| a.login.as_str())
            .unwrap_or(&self.commit.author.name)
    }
}

impl Entry for Commit {
    fn key(&self) -> String {
        self.sha.clone()
    }
    fn signature(&self) -> String {
        // Commits are immutable — a new sha means a new item.
        self.sha.clone()
    }
    fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.short_sha(),
            self.summary(),
            self.author_name()
        )
        .to_lowercase()
    }
}

// ─── Branches (Bitbucket only) ───────────────────────────────────────────────

/// Bitbucket branch (active = sorted by recent commit). Shown in place of the
/// Issues panel on Bitbucket, where Issues is discontinued (built by
/// `bitbucket::fetch_branches`).
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    /// Whether it's the default branch (main/master) — for display emphasis.
    pub is_default: bool,
    pub commit_message: String,
    pub author: String,
    /// Last commit time.
    pub updated_at: String,
    /// Last commit sha (detail = commit diff; used for change detection).
    pub commit_sha: String,
    pub url: String,
}

impl Branch {
    /// First line of the last commit message.
    pub fn summary(&self) -> &str {
        self.commit_message.lines().next().unwrap_or("")
    }
}

impl Entry for Branch {
    fn key(&self) -> String {
        self.name.clone()
    }
    fn signature(&self) -> String {
        // A changed last commit sha means new commits landed on the branch (changed).
        self.commit_sha.clone()
    }
    fn search_text(&self) -> String {
        format!("{} {} {}", self.name, self.summary(), self.author).to_lowercase()
    }
}

// ─── fetch layer ─────────────────────────────────────────────────────────────

/// Data message passed from the fetcher task → UI loop. Updated per panel.
#[derive(Debug)]
pub enum DataMsg {
    Runs(Result<Vec<Run>, String>),
    Issues(Result<Vec<Issue>, String>),
    Prs(Result<Vec<Pr>, String>),
    Commits(Result<Vec<Commit>, String>),
    /// Bitbucket only — active branches shown in place of Issues.
    Branches(Result<Vec<Branch>, String>),
    /// Body for the detail preview modal. `epoch` identifies which request the
    /// response belongs to (to ignore a stale-generation response that arrives
    /// after the modal changed).
    Detail {
        epoch: u64,
        result: Result<DetailPayload, String>,
    },
    /// A newer argus release is available (version string, without the `v`).
    Update(String),
}

/// Detail preview body and rendering hints.
#[derive(Debug, Clone)]
pub struct DetailPayload {
    /// Display body. Markdown source if `markdown`, otherwise plain text.
    pub body: String,
    /// Whether to render the body as markdown (Issue · PR).
    pub markdown: bool,
    /// Whether the run is still in progress and polling should continue (follow).
    pub run_active: bool,
}

/// Identifies what the detail preview should fetch.
#[derive(Debug, Clone)]
pub enum DetailKind {
    /// `log`=false: jobs/steps tree; true: full per-step logs.
    Run {
        id: u64,
        log: bool,
    },
    Issue {
        number: u64,
    },
    Pr {
        number: u64,
    },
    Commit {
        sha: String,
    },
}

/// Detail preview request (passed to the worker task).
#[derive(Debug, Clone)]
pub struct DetailReq {
    pub repo: String,
    pub kind: DetailKind,
    /// The modal generation this request belongs to. Echoed back in the response.
    pub epoch: u64,
}

/// Runs `gh` and returns stdout (JSON) bytes. On failure, stderr as the error message.
async fn run_gh(args: &[&str]) -> Result<Vec<u8>, String> {
    backend::run_cli("gh", args, "gh failed to run (check install/PATH)").await
}

/// Parse JSON bytes into a type.
fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    backend::parse_json(bytes)
}

pub async fn fetch_runs(repo: &str, limit: usize) -> Result<Vec<Run>, String> {
    let limit = limit.to_string();
    let out = run_gh(&[
        "run",
        "list",
        "--repo",
        repo,
        "--limit",
        &limit,
        "--json",
        "databaseId,workflowName,displayTitle,status,conclusion,headBranch,event,createdAt,url",
    ])
    .await?;
    parse(&out)
}

pub async fn fetch_issues(repo: &str, limit: usize) -> Result<Vec<Issue>, String> {
    let limit = limit.to_string();
    let out = run_gh(&[
        "issue",
        "list",
        "--repo",
        repo,
        "--limit",
        &limit,
        "--state",
        "open",
        "--json",
        "number,title,author,updatedAt,labels,url",
    ])
    .await?;
    parse(&out)
}

pub async fn fetch_prs(repo: &str, limit: usize) -> Result<Vec<Pr>, String> {
    let limit = limit.to_string();
    let out = run_gh(&[
        "pr",
        "list",
        "--repo",
        repo,
        "--limit",
        &limit,
        "--state",
        "open",
        "--json",
        "number,title,author,updatedAt,isDraft,reviewDecision,headRefName,url",
    ])
    .await?;
    parse(&out)
}

pub async fn fetch_commits(repo: &str, limit: usize) -> Result<Vec<Commit>, String> {
    let path = format!("repos/{repo}/commits?per_page={limit}");
    let out = run_gh(&["api", &path]).await?;
    parse(&out)
}

/// Polling loop control command (UI → fetcher).
#[derive(Debug)]
pub enum FetchCmd {
    /// Refresh immediately.
    RefreshNow,
    /// Change the polling interval (seconds) at runtime.
    SetInterval(u64),
}

/// Background polling loop. Fetches the 4 data kinds concurrently and sends them
/// over the channel, refetching every `poll_secs` or on a control command
/// (immediate refresh / interval change).
pub async fn fetcher(
    repo: String,
    backend: Backend,
    tx: mpsc::Sender<DataMsg>,
    mut cmd_rx: mpsc::Receiver<FetchCmd>,
    mut poll_secs: u64,
    limit: usize,
) {
    loop {
        // 4 data kinds concurrently. Branches by backend on the gh / bkt path.
        // 4 data kinds concurrently. GitHub uses Issues; Bitbucket has Issues
        // discontinued, so Branches takes its place.
        // If the channel is closed (UI exited), end the loop.
        let closed = match backend {
            Backend::Github => {
                let (r, i, p, c) = tokio::join!(
                    fetch_runs(&repo, limit),
                    fetch_issues(&repo, limit),
                    fetch_prs(&repo, limit),
                    fetch_commits(&repo, limit),
                );
                tx.send(DataMsg::Runs(r)).await.is_err()
                    || tx.send(DataMsg::Issues(i)).await.is_err()
                    || tx.send(DataMsg::Prs(p)).await.is_err()
                    || tx.send(DataMsg::Commits(c)).await.is_err()
            }
            Backend::Bitbucket => {
                let (r, b, p, c) = tokio::join!(
                    bitbucket::fetch_runs(&repo, limit),
                    bitbucket::fetch_branches(&repo, limit),
                    bitbucket::fetch_prs(&repo, limit),
                    bitbucket::fetch_commits(&repo, limit),
                );
                tx.send(DataMsg::Runs(r)).await.is_err()
                    || tx.send(DataMsg::Branches(b)).await.is_err()
                    || tx.send(DataMsg::Prs(p)).await.is_err()
                    || tx.send(DataMsg::Commits(c)).await.is_err()
            }
        };
        if closed {
            break;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(poll_secs)) => {}
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(FetchCmd::RefreshNow) => {}            // straight to next loop
                    Some(FetchCmd::SetInterval(s)) => poll_secs = s, // change interval + refresh now
                    None => break,                              // sender dropped → exit
                }
            }
        }
    }
}

// ─── detail preview fetch ─────────────────────────────────────────────────────

/// Single-commit API response (includes stats · files, unlike the list API).
#[derive(Debug, Deserialize)]
struct CommitFull {
    commit: CommitDetail,
    stats: Option<CommitStats>,
    #[serde(default)]
    files: Vec<CommitFile>,
}

#[derive(Debug, Deserialize)]
struct CommitStats {
    additions: i64,
    deletions: i64,
}

#[derive(Debug, Deserialize)]
struct CommitFile {
    filename: String,
    additions: i64,
    deletions: i64,
    status: String,
}

fn status_mark(status: &str) -> char {
    match status {
        "added" => 'A',
        "removed" => 'D',
        "renamed" => 'R',
        "modified" => 'M',
        _ => '·',
    }
}

fn format_commit(c: &CommitFull, sha: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("commit {sha}\n"));
    out.push_str(&format!("Author: {}\n", c.commit.author.name));
    out.push_str(&format!("Date:   {}\n\n", c.commit.author.date));
    out.push_str(c.commit.message.trim());
    out.push_str("\n\n");
    if let Some(st) = &c.stats {
        out.push_str(&format!(
            "─── {} files changed · +{} −{} ───\n",
            c.files.len(),
            st.additions,
            st.deletions
        ));
    }
    for f in &c.files {
        out.push_str(&format!(
            "  {} +{:<4} −{:<4} {}\n",
            status_mark(&f.status),
            f.additions,
            f.deletions,
            f.filename
        ));
    }
    out
}

// ── Run detail (jobs/steps tree, polling for follow) ──

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunDetail {
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    jobs: Vec<JobInfo>,
    display_title: String,
    workflow_name: String,
    head_branch: String,
    event: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobInfo {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    database_id: u64,
    #[serde(default)]
    steps: Vec<StepInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepInfo {
    name: String,
    status: String,
    conclusion: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

/// Job/step status as a single-char glyph.
/// Whether conclusion is in the failure family (failure/timed_out/startup_failure). Shared by glyph and aggregation.
pub(crate) fn is_failure(conclusion: Option<&str>) -> bool {
    matches!(
        conclusion,
        Some("failure") | Some("timed_out") | Some("startup_failure")
    )
}

fn job_glyph(status: &str, conclusion: Option<&str>) -> char {
    match status {
        "completed" => match conclusion {
            Some("success") => '✓',
            c if is_failure(c) => '✗',
            Some("cancelled") => '⊘',
            Some("skipped") => '⊝',
            _ => '•',
        },
        "in_progress" => '▸',
        "queued" | "waiting" | "pending" | "requested" => '·',
        _ => '•',
    }
}

/// Parse an ISO8601 timestamp. None for pre-epoch values (e.g. `0001-01-01` of an unstarted step).
fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>()
        .ok()
        .filter(|t| t.timestamp() > 0)
}

/// Format seconds as a short human-readable elapsed string (`6s` · `1m23s` · `1h2m`).
fn fmt_dur(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    }
}

/// Step elapsed (seconds). completed = completed-started, in_progress = now-started. None if unstarted (queued).
fn step_elapsed(s: &StepInfo, now: DateTime<Utc>) -> Option<i64> {
    let start = s.started_at.as_deref().and_then(parse_iso)?;
    let end = match s.status.as_str() {
        "completed" => s.completed_at.as_deref().and_then(parse_iso).unwrap_or(now),
        "in_progress" => now,
        _ => return None, // queued/pending/waiting: not started yet
    };
    Some((end - start).num_seconds().max(0))
}

/// One-line job result summary for a completed run (omits zero counts).
fn job_summary(jobs: &[JobInfo], lang: Lang) -> String {
    let count = |c: &str| {
        jobs.iter()
            .filter(|j| j.conclusion.as_deref() == Some(c))
            .count()
    };
    let success = count("success");
    let skipped = count("skipped");
    let cancelled = count("cancelled");
    let failure = jobs
        .iter()
        .filter(|j| is_failure(j.conclusion.as_deref()))
        .count();

    let mut parts = vec![i18n::jobs_count(lang, jobs.len())];
    if success > 0 {
        parts.push(i18n::n_success(lang, success));
    }
    if failure > 0 {
        parts.push(i18n::n_failure(lang, failure));
    }
    if skipped > 0 {
        parts.push(i18n::n_skipped(lang, skipped));
    }
    if cancelled > 0 {
        parts.push(i18n::n_cancelled(lang, cancelled));
    }
    parts.join(" · ")
}

fn format_run(rd: &RunDetail, lang: Lang) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} · {}\n", rd.workflow_name, rd.display_title));
    let concl = match rd.conclusion.as_deref() {
        Some(c) => format!(" ({c})"),
        None => String::new(),
    };
    out.push_str(&format!(
        "{} · {} · {}{}\n",
        rd.event, rd.head_branch, rd.status, concl
    ));
    // Completed run shows a job result summary instead of a gauge (skips · failures at a glance).
    if rd.status == "completed" && !rd.jobs.is_empty() {
        out.push_str(&format!("{}\n", job_summary(&rd.jobs, lang)));
    }
    out.push_str(i18n::log_view_hint(lang));
    out.push_str("\n\n");
    if rd.status != "completed" {
        out.push_str(i18n::in_progress_auto(lang));
        out.push('\n');
        // GitHub creates in-progress jobs/steps dynamically, so the total step count keeps changing.
        // Compute progress per **job** (more stable) instead of the volatile step count, and
        // show job status as counts (steps as an absolute count without a denominator — so growth doesn't mislead).
        let total = rd.jobs.len();
        let done = rd.jobs.iter().filter(|j| j.status == "completed").count();
        let active = rd.jobs.iter().filter(|j| j.status == "in_progress").count();
        let queued = rd
            .jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.status.as_str(),
                    "queued" | "waiting" | "pending" | "requested"
                )
            })
            .count();
        let done_steps = rd
            .jobs
            .iter()
            .flat_map(|j| &j.steps)
            .filter(|s| s.status == "completed")
            .count();
        if total > 0 {
            const W: usize = 16;
            let filled = (done * W).checked_div(total).unwrap_or(0);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(W - filled));
            out.push_str(&i18n::progress_line(lang, bar, done, total, done_steps));
            out.push('\n');
            out.push_str(&i18n::job_counts(lang, done, active, queued));
            out.push('\n');
        }
        out.push('\n');
    }
    for j in &rd.jobs {
        out.push_str(&format!(
            "{} {}\n",
            job_glyph(&j.status, j.conclusion.as_deref()),
            j.name
        ));
        for s in &j.steps {
            out.push_str(&format!(
                "    {} {}\n",
                job_glyph(&s.status, s.conclusion.as_deref()),
                s.name
            ));
        }
    }
    out
}

/// Max lines kept per job when fetching per-job logs (runaway guard, tail).
const JOB_LOG_TAIL: usize = 500;

async fn fetch_run_detail(
    repo: &str,
    id: u64,
    log: bool,
    lang: Lang,
) -> Result<DetailPayload, String> {
    let id_s = id.to_string();
    let out = run_gh(&[
        "run",
        "view",
        &id_s,
        "--repo",
        repo,
        "--json",
        "status,conclusion,jobs,displayTitle,workflowName,headBranch,event",
    ])
    .await?;
    let rd: RunDetail = parse(&out)?;
    let active = rd.status != "completed";

    // Log mode: full per-job logs (parsing step groups). With follow, in-progress logs refresh.
    if log {
        let body = fetch_run_logs(repo, &rd, lang).await;
        return Ok(DetailPayload {
            body,
            markdown: false,
            run_active: active,
        });
    }

    // Tree mode: job/step status.
    let mut body = format_run(&rd, lang);
    let failed = rd.conclusion.as_deref() == Some("failure")
        || rd
            .jobs
            .iter()
            .any(|j| j.conclusion.as_deref() == Some("failure"));
    if !active
        && failed
        && let Ok(bytes) = run_gh(&["run", "view", &id_s, "--repo", repo, "--log-failed"]).await
    {
        let log = String::from_utf8_lossy(&bytes);
        let log = log.trim();
        if !log.is_empty() {
            body.push('\n');
            body.push_str(i18n::failure_log_header(lang));
            body.push('\n');
            body.push_str(log);
            body.push('\n');
        }
    }
    Ok(DetailPayload {
        body,
        markdown: false,
        run_active: active,
    })
}

/// Fetch each job's logs via the `actions/jobs/{id}/logs` API and organize them by step.
async fn fetch_run_logs(repo: &str, rd: &RunDetail, lang: Lang) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} · {}\n", rd.workflow_name, rd.display_title));
    out.push_str(i18n::tree_view_hint(lang));
    out.push('\n');
    if rd.status != "completed" {
        out.push_str(i18n::in_progress_log_auto(lang));
        out.push('\n');
    }
    out.push('\n');

    for j in &rd.jobs {
        out.push_str(&format!(
            "══════ {} {} ══════\n",
            job_glyph(&j.status, j.conclusion.as_deref()),
            j.name
        ));
        if j.database_id == 0 {
            out.push_str(i18n::no_log(lang));
            out.push_str("\n\n");
            continue;
        }
        // GitHub doesn't build a log archive for in-progress jobs, returning 404 (provided after completion).
        // Instead show a live step timeline (status · elapsed) — follow re-requests every ~2s,
        // so elapsed time updates, and once the job finishes it automatically switches to the real logs.
        if j.status != "completed" {
            out.push_str(i18n::timeline_in_progress(lang));
            out.push('\n');
            if j.steps.is_empty() {
                out.push_str(i18n::timeline_waiting(lang));
                out.push_str("\n\n");
                continue;
            }
            let now = Utc::now();
            for s in &j.steps {
                let dur = match step_elapsed(s, now) {
                    Some(secs) => fmt_dur(secs),
                    None => i18n::step_waiting(lang).to_string(),
                };
                out.push_str(&format!(
                    "  {} {} ({})\n",
                    job_glyph(&s.status, s.conclusion.as_deref()),
                    s.name,
                    dur
                ));
            }
            out.push('\n');
            continue;
        }
        let path = format!("repos/{}/actions/jobs/{}/logs", repo, j.database_id);
        match run_gh(&["api", &path]).await {
            Ok(bytes) => out.push_str(&clean_log(
                &String::from_utf8_lossy(&bytes),
                JOB_LOG_TAIL,
                lang,
            )),
            Err(e) => {
                let first = e.lines().next().unwrap_or("");
                // Right after completion the archive may lag briefly, yielding 404 (expiry/permission share the code).
                let msg = if first.contains("404") {
                    i18n::log_not_found(lang).to_string()
                } else {
                    i18n::log_fetch_failed(lang, first.to_string())
                };
                out.push_str(&format!("  ({msg})\n"));
            }
        }
        out.push('\n');
    }
    out
}

/// Strip the leading ISO timestamp from a single Actions log line.
fn strip_ts(line: &str) -> &str {
    if let Some(sp) = line.find(' ') {
        let head = &line[..sp];
        if head.len() >= 20 && head.contains('T') && head.ends_with('Z') {
            return &line[sp + 1..];
        }
    }
    line
}

/// Clean up raw Actions logs for readability (strip timestamps, `##[group]`→step header, etc.).
/// For job logs exceeding `tail`, keep only the last `tail` lines.
fn clean_log(raw: &str, tail: usize, lang: Lang) -> String {
    let cleaned: Vec<String> = raw
        .lines()
        .map(|l| {
            let c = strip_ts(l.trim_start_matches('\u{feff}'));
            if let Some(s) = c.strip_prefix("##[group]") {
                format!("  ▸ {s}")
            } else if c.starts_with("##[endgroup]") {
                String::new()
            } else if let Some(s) = c.strip_prefix("##[error]") {
                format!("  ✗ {s}")
            } else if let Some(s) = c.strip_prefix("##[warning]") {
                format!("  ! {s}")
            } else if let Some(s) = c.strip_prefix("##[section]") {
                format!("  {s}")
            } else if let Some(s) = c.strip_prefix("##[command]") {
                format!("  $ {s}")
            } else if let Some(s) = c.strip_prefix("##[debug]") {
                format!("    {s}")
            } else {
                format!("  {c}")
            }
        })
        .filter(|l| !l.is_empty())
        .collect();

    let slice: &[String] = if cleaned.len() > tail {
        &cleaned[cleaned.len() - tail..]
    } else {
        &cleaned
    };
    let mut s = String::new();
    if cleaned.len() > tail {
        s.push_str(&i18n::lines_omitted(lang, cleaned.len() - tail));
        s.push('\n');
    }
    s.push_str(&slice.join("\n"));
    if !slice.is_empty() {
        s.push('\n');
    }
    s
}

// ── Issue / PR detail (markdown assembly) ──

/// Plain comment below an issue/PR body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    author: Option<Author>,
    #[serde(default)]
    body: String,
    created_at: String,
}

/// PR review (approval/changes-requested, etc.).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Review {
    author: Option<Author>,
    #[serde(default)]
    body: String,
    state: String,
    submitted_at: Option<String>,
}

/// Inline per-line code review comment (REST API, snake_case original).
#[derive(Debug, Default, Deserialize)]
struct ReviewComment {
    path: Option<String>,
    line: Option<u64>,
    user: Option<Author>,
    #[serde(default)]
    body: String,
    created_at: String,
    in_reply_to_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueDetail {
    number: u64,
    title: String,
    state: String,
    author: Option<Author>,
    #[serde(default)]
    labels: Vec<Label>,
    body: String,
    created_at: String,
    #[serde(default)]
    comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrDetail {
    number: u64,
    title: String,
    state: String,
    author: Option<Author>,
    #[serde(default)]
    labels: Vec<Label>,
    body: String,
    is_draft: bool,
    additions: i64,
    deletions: i64,
    head_ref_name: String,
    base_ref_name: String,
    review_decision: Option<String>,
    #[serde(default)]
    comments: Vec<Comment>,
    #[serde(default)]
    reviews: Vec<Review>,
}

fn join_labels(labels: &[Label]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
    format!(" · 🏷 {}", names.join(", "))
}

fn body_or_placeholder(body: &str, lang: Lang) -> &str {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        i18n::body_none(lang)
    } else {
        trimmed
    }
}

fn login_of(a: &Option<Author>) -> &str {
    a.as_ref().map(|x| x.login.as_str()).unwrap_or("?")
}

/// "2026-06-10T22:12:14Z" → "2026-06-10 22:12"
fn short_time(iso: &str) -> String {
    iso[..iso.len().min(16)].replace('T', " ")
}

fn review_icon(state: &str, lang: Lang) -> &'static str {
    match state {
        "APPROVED" => i18n::review_approved(lang),
        "CHANGES_REQUESTED" => i18n::review_changes(lang),
        "COMMENTED" => i18n::review_commented(lang),
        "DISMISSED" => i18n::review_dismissed(lang),
        _ => i18n::review_other(lang),
    }
}

fn format_issue(d: &IssueDetail, lang: Lang) -> String {
    let date = &d.created_at[..d.created_at.len().min(10)];
    let mut s = format!(
        "# #{} {}\n\n`{}` · @{}{} · {}\n\n---\n\n{}\n",
        d.number,
        d.title,
        d.state,
        login_of(&d.author),
        join_labels(&d.labels),
        date,
        body_or_placeholder(&d.body, lang),
    );
    if !d.comments.is_empty() {
        s.push_str(&format!(
            "\n---\n\n{}\n\n",
            i18n::comments_header(lang, d.comments.len())
        ));
        for c in &d.comments {
            s.push_str(&format!(
                "### @{} · {}\n\n{}\n\n",
                login_of(&c.author),
                short_time(&c.created_at),
                body_or_placeholder(&c.body, lang),
            ));
        }
    }
    s
}

fn format_pr(d: &PrDetail, threads: &[ReviewComment], lang: Lang) -> String {
    let state = if d.is_draft {
        format!("{} · draft", d.state)
    } else {
        d.state.clone()
    };
    let review = d
        .review_decision
        .as_deref()
        .map(|r| format!(" · review: {r}"))
        .unwrap_or_default();
    let mut s = format!(
        "# #{} {}\n\n`{}` · @{}{} · {} ← {} · +{} −{}{}\n\n---\n\n{}\n",
        d.number,
        d.title,
        state,
        login_of(&d.author),
        join_labels(&d.labels),
        d.base_ref_name,
        d.head_ref_name,
        d.additions,
        d.deletions,
        review,
        body_or_placeholder(&d.body, lang),
    );

    // Timeline merging reviews + plain comments in chronological order.
    let mut events: Vec<(String, String)> = Vec::new();
    for r in &d.reviews {
        let t = r.submitted_at.clone().unwrap_or_default();
        let mut block = format!(
            "### 🔍 {} · @{} · {}\n",
            review_icon(&r.state, lang),
            login_of(&r.author),
            short_time(&t),
        );
        if !r.body.trim().is_empty() {
            block.push_str(&format!("\n{}\n", r.body.trim()));
        }
        events.push((t, block));
    }
    for c in &d.comments {
        events.push((
            c.created_at.clone(),
            format!(
                "### 💬 @{} · {}\n\n{}\n",
                login_of(&c.author),
                short_time(&c.created_at),
                body_or_placeholder(&c.body, lang),
            ),
        ));
    }
    if !events.is_empty() {
        events.sort_by(|a, b| a.0.cmp(&b.0));
        s.push_str(&format!("\n---\n\n{}\n\n", i18n::timeline_header(lang)));
        for (_, block) in events {
            s.push_str(&block);
            s.push('\n');
        }
    }

    // Code review threads: grouped by file, each thread chronological (replies as ↳).
    if !threads.is_empty() {
        use std::collections::BTreeMap;
        let mut by_path: BTreeMap<&str, Vec<&ReviewComment>> = BTreeMap::new();
        for c in threads {
            by_path
                .entry(
                    c.path
                        .as_deref()
                        .unwrap_or_else(|| i18n::path_general(lang)),
                )
                .or_default()
                .push(c);
        }
        s.push_str(&format!("\n---\n\n{}\n\n", i18n::code_review_header(lang)));
        for (path, mut comments) in by_path {
            comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            let loc = match comments.iter().find_map(|c| c.line) {
                Some(l) => format!("{path}:{l}"),
                None => path.to_string(),
            };
            s.push_str(&format!("### 📄 {loc}\n\n"));
            for c in comments {
                let prefix = if c.in_reply_to_id.is_some() {
                    "↳ "
                } else {
                    ""
                };
                s.push_str(&format!(
                    "{}**@{}** · {}\n\n{}\n\n",
                    prefix,
                    login_of(&c.user),
                    short_time(&c.created_at),
                    body_or_placeholder(&c.body, lang),
                ));
            }
        }
    }

    s
}

/// Fetch the detail body of a single item.
pub async fn fetch_detail(
    req: &DetailReq,
    backend: Backend,
    lang: Lang,
) -> Result<DetailPayload, String> {
    // Bitbucket delegates to its own detail path (bitbucket.rs).
    if backend == Backend::Bitbucket {
        return bitbucket::fetch_detail(req, lang).await;
    }
    match &req.kind {
        DetailKind::Run { id, log } => fetch_run_detail(&req.repo, *id, *log, lang).await,
        DetailKind::Issue { number } => {
            let n = number.to_string();
            let out = run_gh(&[
                "issue",
                "view",
                &n,
                "--repo",
                &req.repo,
                "--json",
                "number,title,state,author,labels,body,createdAt,comments",
            ])
            .await?;
            let d: IssueDetail = parse(&out)?;
            Ok(DetailPayload {
                body: format_issue(&d, lang),
                markdown: true,
                run_active: false,
            })
        }
        DetailKind::Pr { number } => {
            let n = number.to_string();
            let out = run_gh(&[
                "pr", "view", &n, "--repo", &req.repo,
                "--json",
                "number,title,state,author,labels,body,isDraft,additions,deletions,headRefName,baseRefName,reviewDecision,comments,reviews",
            ])
            .await?;
            let d: PrDetail = parse(&out)?;
            // Inline code review threads (body still shown on failure).
            let thread_path = format!("repos/{}/pulls/{}/comments?per_page=100", &req.repo, number);
            let threads: Vec<ReviewComment> = match run_gh(&["api", &thread_path]).await {
                Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            Ok(DetailPayload {
                body: format_pr(&d, &threads, lang),
                markdown: true,
                run_active: false,
            })
        }
        DetailKind::Commit { sha } => {
            let path = format!("repos/{}/commits/{}", req.repo, sha);
            let out = run_gh(&["api", &path]).await?;
            let full: CommitFull = parse(&out)?;
            Ok(DetailPayload {
                body: format_commit(&full, sha),
                markdown: false,
                run_active: false,
            })
        }
    }
}

/// Worker task that receives detail requests, fetches, and sends the result to the UI.
pub async fn detail_worker(
    mut rx: mpsc::Receiver<DetailReq>,
    tx: mpsc::Sender<DataMsg>,
    backend: Backend,
    lang: Lang,
) {
    while let Some(req) = rx.recv().await {
        let result = fetch_detail(&req, backend, lang).await;
        let msg = DataMsg::Detail {
            epoch: req.epoch,
            result,
        };
        if tx.send(msg).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_glyphs() {
        assert_eq!(job_glyph("completed", Some("success")), '✓');
        assert_eq!(job_glyph("completed", Some("failure")), '✗');
        assert_eq!(job_glyph("completed", Some("skipped")), '⊝');
        assert_eq!(job_glyph("in_progress", None), '▸');
        assert_eq!(job_glyph("queued", None), '·');
    }

    #[test]
    fn formats_run_tree() {
        let rd = RunDetail {
            status: "in_progress".into(),
            conclusion: None,
            jobs: vec![JobInfo {
                name: "build".into(),
                status: "in_progress".into(),
                conclusion: None,
                database_id: 0,
                steps: vec![
                    StepInfo {
                        name: "checkout".into(),
                        status: "completed".into(),
                        conclusion: Some("success".into()),
                        started_at: None,
                        completed_at: None,
                    },
                    StepInfo {
                        name: "compile".into(),
                        status: "in_progress".into(),
                        conclusion: None,
                        started_at: None,
                        completed_at: None,
                    },
                ],
            }],
            display_title: "fix".into(),
            workflow_name: "CI".into(),
            head_branch: "main".into(),
            event: "push".into(),
        };
        let out = format_run(&rd, Lang::Ko);
        assert!(out.contains("CI · fix"));
        assert!(out.contains("진행 중"));
        assert!(out.contains("▸ build"));
        assert!(out.contains("✓ checkout"));
        assert!(out.contains("▸ compile"));
        // Progress per job (0/1), steps as an absolute count (1 completed), job status counts.
        assert!(out.contains("잡 0/1"), "got:\n{out}");
        assert!(out.contains("스텝 1 완료"));
        assert!(out.contains("▸1 진행"));
    }

    #[test]
    fn run_summary_for_completed() {
        let job = |name: &str, concl: &str| JobInfo {
            name: name.into(),
            status: "completed".into(),
            conclusion: Some(concl.into()),
            database_id: 0,
            steps: vec![],
        };
        // docs change: only detect succeeds, the rest skipped.
        let rd = RunDetail {
            status: "completed".into(),
            conclusion: Some("success".into()),
            jobs: vec![
                job("detect", "success"),
                job("install", "skipped"),
                job("test", "skipped"),
                job("lint", "skipped"),
            ],
            display_title: "docs".into(),
            workflow_name: "Build & Test".into(),
            head_branch: "main".into(),
            event: "push".into(),
        };
        let out = format_run(&rd, Lang::Ko);
        assert!(out.contains("잡 4 · ✓1 성공 · ⊝3 스킵"), "got:\n{out}");
        assert!(!out.contains("실패")); // failure 0 → omitted
        assert!(!out.contains("진행률")); // completed run has no gauge
    }

    #[test]
    fn formats_issue_with_comments() {
        let d = IssueDetail {
            number: 7,
            title: "bug".into(),
            state: "OPEN".into(),
            author: Some(Author {
                login: "alice".into(),
            }),
            labels: vec![Label {
                name: "bug".into(),
                color: "d73a4a".into(),
            }],
            body: "   ".into(),
            created_at: "2026-06-10T13:27:08Z".into(),
            comments: vec![Comment {
                author: Some(Author {
                    login: "bob".into(),
                }),
                body: "I can repro".into(),
                created_at: "2026-06-11T09:00:00Z".into(),
            }],
        };
        let md = format_issue(&d, Lang::Ko);
        assert!(md.starts_with("# #7 bug"));
        assert!(md.contains("@alice"));
        assert!(md.contains("본문 없음")); // empty-body placeholder
        assert!(md.contains("## 💬 코멘트 (1)"));
        assert!(md.contains("@bob"));
        assert!(md.contains("I can repro"));
    }

    #[test]
    fn formats_pr_timeline_and_threads() {
        let d = PrDetail {
            number: 3,
            title: "feat".into(),
            state: "OPEN".into(),
            author: Some(Author {
                login: "alice".into(),
            }),
            labels: vec![],
            body: "do a thing".into(),
            is_draft: false,
            additions: 10,
            deletions: 2,
            head_ref_name: "feat-x".into(),
            base_ref_name: "main".into(),
            review_decision: Some("CHANGES_REQUESTED".into()),
            comments: vec![Comment {
                author: Some(Author {
                    login: "carol".into(),
                }),
                body: "nit".into(),
                created_at: "2026-06-11T10:00:00Z".into(),
            }],
            reviews: vec![Review {
                author: Some(Author {
                    login: "dave".into(),
                }),
                body: "please fix".into(),
                state: "CHANGES_REQUESTED".into(),
                submitted_at: Some("2026-06-11T09:30:00Z".into()),
            }],
        };
        let threads = vec![ReviewComment {
            path: Some("src/main.rs".into()),
            line: Some(42),
            user: Some(Author {
                login: "dave".into(),
            }),
            body: "rename this".into(),
            created_at: "2026-06-11T09:31:00Z".into(),
            in_reply_to_id: None,
        }];
        let md = format_pr(&d, &threads, Lang::Ko);
        assert!(md.contains("main ← feat-x"));
        assert!(md.contains("## 💬 타임라인"));
        assert!(md.contains("✗ 변경요청"));
        assert!(md.contains("please fix"));
        assert!(md.contains("@carol"));
        assert!(md.contains("## 📄 코드 리뷰 스레드"));
        assert!(md.contains("src/main.rs:42"));
        assert!(md.contains("rename this"));
    }

    #[test]
    fn cleans_actions_log() {
        let raw = "2026-06-12T09:37:45.7269406Z ##[group]Set up job\n\
                   2026-06-12T09:37:45.7300000Z Hello\n\
                   2026-06-12T09:37:45.7400000Z ##[endgroup]\n\
                   2026-06-12T09:37:45.7500000Z ##[error]boom\n";
        let out = clean_log(raw, 100, Lang::Ko);
        assert!(out.contains("▸ Set up job"));
        assert!(out.contains("  Hello"));
        assert!(!out.contains("##[endgroup]"));
        assert!(out.contains("✗ boom"));
        assert!(!out.contains("2026-06-12T")); // timestamp stripped
    }

    #[test]
    fn log_tail_truncates() {
        let raw: String = (0..50)
            .map(|i| format!("2026-06-12T09:37:45.0000000Z line{i}\n"))
            .collect();
        let out = clean_log(&raw, 10, Lang::Ko);
        assert!(out.contains("앞부분 40줄 생략"));
        assert!(out.contains("line49"));
        assert!(!out.contains("line0\n"));
    }

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_dur(0), "0s");
        assert_eq!(fmt_dur(45), "45s");
        assert_eq!(fmt_dur(83), "1m23s");
        assert_eq!(fmt_dur(3725), "1h2m");
        assert_eq!(fmt_dur(-5), "0s"); // negatives clamp to 0
    }

    #[test]
    fn step_elapsed_by_status() {
        let now: DateTime<Utc> = "2026-06-16T00:01:00Z".parse().unwrap();
        let step = |status: &str, started: Option<&str>, completed: Option<&str>| StepInfo {
            name: "s".into(),
            status: status.into(),
            conclusion: None,
            started_at: started.map(str::to_string),
            completed_at: completed.map(str::to_string),
        };
        // completed: completed-started.
        assert_eq!(
            step_elapsed(
                &step(
                    "completed",
                    Some("2026-06-16T00:00:00Z"),
                    Some("2026-06-16T00:00:30Z")
                ),
                now
            ),
            Some(30)
        );
        // in_progress: now-started.
        assert_eq!(
            step_elapsed(
                &step("in_progress", Some("2026-06-16T00:00:00Z"), None),
                now
            ),
            Some(60)
        );
        // queued (unstarted) · pre-epoch start value → None.
        assert_eq!(step_elapsed(&step("queued", None, None), now), None);
        assert_eq!(
            step_elapsed(
                &step("in_progress", Some("0001-01-01T00:00:00Z"), None),
                now
            ),
            None
        );
    }
}
