//! Layer that fetches Bitbucket Cloud data via the `bkt` CLI.
//!
//! Every fetch calls Bitbucket REST API 2.0 through `bkt api <path>` (auth delegated to `bkt`)
//! and converts the response into the common models (`Run`/`Issue`/`Pr`/`Commit` in github.rs),
//! so the app/ui layers stay backend-agnostic. The repo argument is `workspace/repo_slug`.
//!
//! Bitbucket's status vocabulary (`COMPLETED`/`SUCCESSFUL` etc.) is normalized to the GitHub
//! vocabulary (`completed`/`success`) so ui.rs glyph/completion-rate/success-badge logic works as-is.

use serde::Deserialize;

use crate::backend;
use crate::github::{
    Author, Branch, Commit, CommitAuthor, CommitDetail, DetailKind, DetailPayload, DetailReq, Pr,
    Run,
};
use crate::i18n::{self, Lang};

/// Runs `bkt` and returns the stdout (JSON) bytes (sibling of github::run_gh, delegates to the shared helper).
/// Common bkt errors are rewritten into actionable guidance shown in the panel. The error is
/// reached from the (lang-less) fetcher path, so it reads the global resolved language.
async fn run_bkt(args: &[&str]) -> Result<Vec<u8>, String> {
    backend::run_cli(
        "bkt",
        args,
        "bkt failed to run (check install/PATH and `bkt auth login`)",
    )
    .await
    .map_err(|e| humanize_bkt_error(e, i18n::current()))
}

/// Converts a raw bkt error into friendly setup guidance (keeps the original if none matches).
fn humanize_bkt_error(err: String, lang: Lang) -> String {
    let low = err.to_lowercase();
    // Logged in but no active context (the most common first-run pitfall).
    if low.contains("no active context") {
        return i18n::bkt_no_context(lang).to_string();
    }
    // Not authenticated.
    if low.contains("not authenticated")
        || low.contains("no credentials")
        || low.contains("login required")
    {
        return i18n::bkt_not_authed(lang).to_string();
    }
    err
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    backend::parse_json(bytes)
}

/// Common page wrapper for Bitbucket REST list responses (`{ "values": [...] }`).
#[derive(Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    values: Vec<T>,
}

/// Small `{ "name": ... }` object (reused for state.result, trigger, branch, etc.).
#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Href {
    #[serde(default)]
    href: String,
}

#[derive(Deserialize)]
struct Links {
    html: Option<Href>,
}

/// Bitbucket user. There's no stable identifier matching GitHub's login, so prefer nickname→display_name.
#[derive(Deserialize)]
struct User {
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    display_name: String,
}

impl User {
    fn login(&self) -> String {
        if !self.nickname.is_empty() {
            self.nickname.clone()
        } else {
            self.display_name.clone()
        }
    }
}

/// Bitbucket user → common `Author`. Returns `None` when the identifier is empty
/// to avoid being mistaken for "has author (empty string)" (prevents empty author).
fn user_to_author(user: Option<User>) -> Option<Author> {
    user.map(|u| u.login())
        .filter(|l| !l.is_empty())
        .map(|login| Author { login })
}

// ─── State vocabulary normalization (Bitbucket → GitHub) ─────────────────────

/// pipeline `state.name` (+ in-progress `stage.name`) → GitHub run `status`.
fn map_status(name: &str, stage: Option<&str>) -> String {
    match name {
        // An in-progress pipeline halted at a manual step reports stage PAUSED/HALTED. That's a
        // manual gate (an indefinite wait, not active work) — map to GitHub's `waiting` so it shows
        // the waiting glyph instead of the running spinner and doesn't keep has_active_runs true.
        "IN_PROGRESS" | "BUILDING" | "RUNNING"
            if matches!(stage, Some("PAUSED") | Some("HALTED")) =>
        {
            "waiting".into()
        }
        "IN_PROGRESS" | "BUILDING" | "RUNNING" => "in_progress".into(),
        "PENDING" | "PAUSED" => "queued".into(),
        // COMPLETED and all other terminal/unknown states (ERROR/STOPPED/HALTED etc.) count as 'completed'.
        // Treating an unknown state as in-progress would make has_active_runs(app.rs) permanently true
        // and the header spinner would never stop, so only known progress/queued map that way; the rest are completed.
        _ => "completed".into(),
    }
}

/// pipeline `state.result.name` → GitHub run `conclusion`.
fn map_conclusion(name: &str) -> String {
    match name {
        "SUCCESSFUL" => "success".into(),
        "FAILED" => "failure".into(),
        "STOPPED" | "EXPIRED" => "cancelled".into(),
        "ERROR" => "failure".into(),
        other => other.to_lowercase(),
    }
}

// ─── Actions (pipelines) ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BbPipeline {
    #[serde(default)]
    build_number: u64,
    state: Option<PipelineState>,
    target: Option<PipelineTarget>,
    trigger: Option<Named>,
    #[serde(default)]
    created_on: String,
}

#[derive(Deserialize, Default)]
struct PipelineState {
    #[serde(default)]
    name: String,
    /// Sub-stage while IN_PROGRESS: RUNNING / PAUSED / HALTED. PAUSED/HALTED means the
    /// pipeline is halted at a manual step (an indefinite gate), not actively running.
    stage: Option<Named>,
    result: Option<Named>,
}

#[derive(Deserialize, Default)]
struct PipelineTarget {
    #[serde(default)]
    ref_name: String,
    commit: Option<PipelineCommit>,
}

/// Commit the pipeline points at. message is absent from the default response, so it's expanded via the fields param.
#[derive(Deserialize)]
struct PipelineCommit {
    #[serde(default)]
    message: String,
}

pub async fn fetch_runs(repo: &str, limit: usize) -> Result<Vec<Run>, String> {
    // bkt api convention: leading slash + queries via --param (Cloud adds the /2.0 prefix automatically).
    // commit.message is absent from the default response, so expand it via fields (get the title message without an extra call).
    let path = format!("/repositories/{repo}/pipelines/");
    let pagelen = format!("pagelen={limit}");
    let out = run_bkt(&[
        "api",
        &path,
        "--param",
        "sort=-created_on",
        "--param",
        "fields=+values.target.commit.message",
        "--param",
        &pagelen,
    ])
    .await?;
    let page: Page<BbPipeline> = parse(&out)?;
    Ok(page
        .values
        .into_iter()
        .map(|p| pipeline_to_run(repo, p))
        .collect())
}

fn pipeline_to_run(repo: &str, p: BbPipeline) -> Run {
    let BbPipeline {
        build_number,
        state,
        target,
        trigger,
        created_on,
    } = p;
    let state = state.unwrap_or_default();
    let target = target.unwrap_or_default();

    let status = map_status(&state.name, state.stage.as_ref().map(|s| s.name.as_str()));
    // Set conclusion **only on completed runs** so a leftover prior result on an in-progress run
    // isn't miscounted into the failure badge (✗N) (same as the GitHub model: conclusion only when completed).
    let conclusion = if status == "completed" {
        state.result.map(|r| map_conclusion(&r.name))
    } else {
        None
    };
    let head_branch = target.ref_name;
    // Bitbucket has no GitHub-style workflow name → show the pipeline run number (build number) up front
    // (in GitHub's workflow_name slot). The number is more useful than the selector type (default/custom).
    let workflow_name = format!("#{build_number}");
    let event = trigger.map(|t| t.name.to_lowercase()).unwrap_or_default();
    // Title is the first **non-empty** line of the commit message (equivalent to GitHub displayTitle). Falls back to the build number.
    let display_title = target
        .commit
        .and_then(|c| {
            c.message
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| format!("#{build_number}"));
    Run {
        database_id: build_number,
        workflow_name,
        display_title,
        status,
        conclusion,
        head_branch,
        event,
        created_at: created_on,
        url: format!("https://bitbucket.org/{repo}/pipelines/results/{build_number}"),
    }
}

// ─── Pull Requests ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BbPr {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    title: String,
    author: Option<User>,
    #[serde(default)]
    updated_on: String,
    #[serde(default)]
    draft: bool,
    source: Option<Endpoint>,
    links: Option<Links>,
    #[serde(default)]
    participants: Vec<Participant>,
}

#[derive(Deserialize)]
struct Endpoint {
    branch: Option<Named>,
}

/// PR participant. Bitbucket has no single reviewDecision like GitHub, so participant approval states are aggregated.
#[derive(Deserialize)]
struct Participant {
    #[serde(default)]
    approved: bool,
    state: Option<String>,
}

/// participants → GitHub reviewDecision vocabulary. Any changes-requested takes priority, then approved.
/// None if there's no action (ui shows the default glyph `◍`).
fn review_decision(participants: &[Participant]) -> Option<String> {
    if participants
        .iter()
        .any(|p| p.state.as_deref() == Some("changes_requested"))
    {
        Some("CHANGES_REQUESTED".into())
    } else if participants
        .iter()
        .any(|p| p.approved || p.state.as_deref() == Some("approved"))
    {
        Some("APPROVED".into())
    } else {
        None
    }
}

pub async fn fetch_prs(repo: &str, limit: usize) -> Result<Vec<Pr>, String> {
    let path = format!("/repositories/{repo}/pullrequests");
    let pagelen = format!("pagelen={limit}");
    // Sort by most recently updated (consistent with the GitHub path). participants is absent from the default
    // list response, so expand it via fields (for aggregating review approvals/change-requests — without an extra call).
    let out = run_bkt(&[
        "api",
        &path,
        "--param",
        "state=OPEN",
        "--param",
        "sort=-updated_on",
        "--param",
        "fields=+values.participants.approved,+values.participants.state",
        "--param",
        &pagelen,
    ])
    .await?;
    let page: Page<BbPr> = parse(&out)?;
    Ok(page.values.into_iter().map(pr_to_pr).collect())
}

fn pr_to_pr(p: BbPr) -> Pr {
    let review_decision = review_decision(&p.participants);
    Pr {
        number: p.id,
        title: p.title,
        author: user_to_author(p.author),
        updated_at: p.updated_on,
        is_draft: p.draft,
        review_decision,
        head_ref_name: p
            .source
            .and_then(|e| e.branch)
            .map(|b| b.name)
            .unwrap_or_default(),
        url: html_url(p.links),
    }
}

// (Bitbucket Issues is being deprecated, so there's no list fetch — fetch_branches fills the Issues slot.)

// ─── Commits ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BbCommit {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    message: String,
    author: Option<CommitAuthorRaw>,
    links: Option<Links>,
}

#[derive(Deserialize)]
struct CommitAuthorRaw {
    /// Raw "Name <email>" (fallback when user mapping fails).
    #[serde(default)]
    raw: String,
    user: Option<User>,
}

pub async fn fetch_commits(repo: &str, limit: usize) -> Result<Vec<Commit>, String> {
    let path = format!("/repositories/{repo}/commits");
    let pagelen = format!("pagelen={limit}");
    let out = run_bkt(&["api", &path, "--param", &pagelen]).await?;
    let page: Page<BbCommit> = parse(&out)?;
    Ok(page.values.into_iter().map(commit_to_commit).collect())
}

fn commit_to_commit(c: BbCommit) -> Commit {
    // git author display name: prefer user.display_name, else just the name from "Name <email>".
    let (name, login) = match c.author {
        Some(a) => {
            // login is None when empty (prevents empty author) → author_name() falls back to the raw name.
            let login = a.user.as_ref().map(|u| u.login()).filter(|l| !l.is_empty());
            let name = a
                .user
                .map(|u| u.display_name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| author_name_from_raw(&a.raw));
            (name, login)
        }
        None => (String::new(), None),
    };
    Commit {
        sha: c.hash,
        commit: CommitDetail {
            message: c.message,
            author: CommitAuthor { name, date: c.date },
        },
        author: login.map(|login| Author { login }),
        html_url: html_url(c.links),
    }
}

/// Extracts just the name part from a raw "Name <email>".
fn author_name_from_raw(raw: &str) -> String {
    raw.split('<').next().unwrap_or(raw).trim().to_string()
}

/// Commit author display name: prefer the mapped user (login), else the name from "Name <email>".
fn commit_author_name(a: &CommitAuthorRaw) -> String {
    a.user
        .as_ref()
        .map(|u| u.login())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| author_name_from_raw(&a.raw))
}

fn html_url(links: Option<Links>) -> String {
    links
        .and_then(|l| l.html)
        .map(|h| h.href)
        .unwrap_or_default()
}

// ─── Branches (Bitbucket-only, in the Issues slot) ───────────────────────────

#[derive(Deserialize)]
struct BbBranch {
    #[serde(default)]
    name: String,
    target: Option<BranchTarget>,
    links: Option<Links>,
}

#[derive(Deserialize, Default)]
struct BranchTarget {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    message: String,
    author: Option<CommitAuthorRaw>,
}

#[derive(Deserialize)]
struct MainBranch {
    mainbranch: Option<Named>,
}

pub async fn fetch_branches(repo: &str, limit: usize) -> Result<Vec<Branch>, String> {
    let pagelen = format!("pagelen={limit}");
    let main_path = format!("/repositories/{repo}");
    let list_path = format!("/repositories/{repo}/refs/branches");
    // The arrays/strings must be bound with let so both join! futures can borrow them during await.
    let main_args = ["api", main_path.as_str()];
    let list_args = [
        "api",
        list_path.as_str(),
        "--param",
        "sort=-target.date",
        "--param",
        pagelen.as_str(),
    ];
    // Fetch the default branch name + the active (most-recent-commit-first) branch list concurrently.
    let (main_res, list_res) = tokio::join!(run_bkt(&main_args), run_bkt(&list_args));
    let default_name = main_res
        .ok()
        .and_then(|b| parse::<MainBranch>(&b).ok())
        .and_then(|m| m.mainbranch)
        .map(|n| n.name)
        .unwrap_or_default();
    let page: Page<BbBranch> = parse(&list_res?)?;
    Ok(page
        .values
        .into_iter()
        .map(|b| branch_to_branch(b, &default_name))
        .collect())
}

fn branch_to_branch(b: BbBranch, default_name: &str) -> Branch {
    let target = b.target.unwrap_or_default();
    let author = target
        .author
        .as_ref()
        .map(commit_author_name)
        .unwrap_or_default();
    Branch {
        is_default: !default_name.is_empty() && b.name == default_name,
        name: b.name,
        commit_message: target.message,
        author,
        updated_at: target.date,
        commit_sha: target.hash,
        url: html_url(b.links),
    }
}

// ─── Detail preview ──────────────────────────────────────────────────────────

/// Fetches the detail body for one item (the Bitbucket delegation target of github::fetch_detail).
pub async fn fetch_detail(req: &DetailReq, lang: Lang) -> Result<DetailPayload, String> {
    match &req.kind {
        DetailKind::Run { id, log } => pipeline_detail(&req.repo, *id, *log, lang).await,
        DetailKind::Pr { number } => pr_detail(&req.repo, *number, lang).await,
        DetailKind::Issue { number } => issue_detail(&req.repo, *number, lang).await,
        DetailKind::Commit { sha } => commit_detail(&req.repo, sha).await,
    }
}

/// Keeps only the last `n` lines of the text (prevents log flooding).
fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ── Pipeline detail (step tree / logs, for follow) ──

#[derive(Deserialize)]
struct StepInfo {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    name: String,
    state: Option<PipelineState>,
}

/// Step state → glyph (same vocabulary as github job_glyph).
fn step_glyph(state: &PipelineState) -> char {
    match state.name.as_str() {
        "IN_PROGRESS" | "BUILDING" | "RUNNING" => '▸',
        "PENDING" | "PAUSED" => '·',
        // Others (COMPLETED etc.) are decided by result.
        _ => match state.result.as_ref().map(|r| r.name.as_str()) {
            Some("SUCCESSFUL") => '✓',
            Some("FAILED") | Some("ERROR") => '✗',
            Some("STOPPED") | Some("EXPIRED") => '⊘',
            _ => '•',
        },
    }
}

async fn pipeline_detail(
    repo: &str,
    build: u64,
    log: bool,
    lang: Lang,
) -> Result<DetailPayload, String> {
    let pipe: BbPipeline =
        parse(&run_bkt(&["api", &format!("/repositories/{repo}/pipelines/{build}")]).await?)?;
    let state = pipe.state.unwrap_or_default();
    let status = map_status(&state.name, state.stage.as_ref().map(|s| s.name.as_str()));
    // `waiting` (halted at a manual step) is paused, not running — don't follow/auto-refresh it.
    let active = status == "in_progress";
    let steps: Page<StepInfo> = parse(
        &run_bkt(&[
            "api",
            &format!("/repositories/{repo}/pipelines/{build}/steps"),
        ])
        .await?,
    )?;

    if log {
        let body = pipeline_logs(repo, build, &steps.values, lang).await;
        return Ok(DetailPayload {
            body,
            markdown: false,
            run_active: active,
        });
    }

    let mut out = String::new();
    let result = state.result.as_ref().map(|r| r.name.as_str()).unwrap_or("");
    let stage = state.stage.as_ref().map(|s| s.name.as_str()).unwrap_or("");
    out.push_str(&format!("Pipeline #{build}\n"));
    if !result.is_empty() {
        out.push_str(&format!("{} ({result})\n", state.name));
    } else if !stage.is_empty() {
        // e.g. IN_PROGRESS · PAUSED — clarifies it's halted at a manual step, not actively running.
        out.push_str(&format!("{} · {stage}\n", state.name));
    } else {
        out.push_str(&format!("{}\n", state.name));
    }
    out.push_str(i18n::log_view_hint(lang));
    out.push_str("\n\n");
    if active {
        out.push_str(i18n::in_progress_auto(lang));
        out.push_str("\n\n");
    }
    for s in &steps.values {
        let g = s.state.as_ref().map(step_glyph).unwrap_or('•');
        out.push_str(&format!("{g} {}\n", s.name));
    }
    Ok(DetailPayload {
        body: out,
        markdown: false,
        run_active: active,
    })
}

async fn pipeline_logs(repo: &str, build: u64, steps: &[StepInfo], lang: Lang) -> String {
    let mut out = i18n::pipeline_header(lang, build);
    for s in steps {
        let g = s.state.as_ref().map(step_glyph).unwrap_or('•');
        out.push_str(&format!("══════ {g} {} ══════\n", s.name));
        if s.uuid.is_empty() {
            out.push_str(i18n::no_log(lang));
            out.push_str("\n\n");
            continue;
        }
        // Bitbucket step logs are text/plain, so bkt's default JSON Accept yields 406 → relax Accept.
        let path = format!(
            "/repositories/{repo}/pipelines/{build}/steps/{}/log",
            s.uuid
        );
        match run_bkt(&["api", &path, "--header", "Accept: */*"]).await {
            Ok(b) => {
                out.push_str(&tail_lines(String::from_utf8_lossy(&b).trim(), 500));
                out.push('\n');
            }
            Err(e) => out.push_str(&format!(
                "  ({})\n",
                i18n::log_fetch_failed(lang, e.lines().next().unwrap_or("").to_string())
            )),
        }
        out.push('\n');
    }
    out
}

// ── PR detail ──

#[derive(Deserialize)]
struct BbPrDetail {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    state: String,
    author: Option<User>,
    source: Option<Endpoint>,
    destination: Option<Endpoint>,
}

#[derive(Deserialize)]
struct RenderedContent {
    #[serde(default)]
    raw: String,
}

#[derive(Deserialize)]
struct BbComment {
    content: Option<RenderedContent>,
    user: Option<User>,
    inline: Option<Inline>,
}

#[derive(Deserialize)]
struct Inline {
    #[serde(default)]
    path: String,
    /// Line number (in the new file) the comment is attached to.
    to: Option<u64>,
}

async fn fetch_comments(path: String) -> Vec<BbComment> {
    run_bkt(&["api", &path])
        .await
        .ok()
        .and_then(|b| parse::<Page<BbComment>>(&b).ok())
        .map(|p| p.values)
        .unwrap_or_default()
}

fn branch_name(e: &Option<Endpoint>) -> String {
    e.as_ref()
        .and_then(|e| e.branch.as_ref())
        .map(|b| b.name.clone())
        .unwrap_or_default()
}

async fn pr_detail(repo: &str, number: u64, lang: Lang) -> Result<DetailPayload, String> {
    let pr: BbPrDetail = parse(
        &run_bkt(&[
            "api",
            &format!("/repositories/{repo}/pullrequests/{number}"),
        ])
        .await?,
    )?;
    let comments = fetch_comments(format!(
        "/repositories/{repo}/pullrequests/{number}/comments"
    ))
    .await;

    let mut out = format!("#{} {}\n", pr.id, pr.title);
    let author = pr.author.map(|u| u.login()).unwrap_or_default();
    out.push_str(&format!(
        "{} → {} · {} · @{author}\n\n",
        branch_name(&pr.source),
        branch_name(&pr.destination),
        pr.state,
    ));
    if !pr.description.trim().is_empty() {
        out.push_str(pr.description.trim());
        out.push_str("\n\n");
    }
    append_comments(&mut out, &comments, lang);
    Ok(DetailPayload {
        body: out,
        markdown: true,
        run_active: false,
    })
}

/// Appends the comment timeline to the markdown body (inline comments note the file path).
fn append_comments(out: &mut String, comments: &[BbComment], lang: Lang) {
    if comments.is_empty() {
        return;
    }
    out.push_str(&format!("{}\n\n", i18n::comments_sep(lang, comments.len())));
    for c in comments {
        let user = c.user.as_ref().map(|u| u.login()).unwrap_or_default();
        let text = c.content.as_ref().map(|r| r.raw.as_str()).unwrap_or("");
        // Inline (code review) comments note the file and line.
        let inline = c
            .inline
            .as_ref()
            .filter(|i| !i.path.is_empty())
            .map(|i| match i.to {
                Some(line) => format!(" `{}:{line}`", i.path),
                None => format!(" `{}`", i.path),
            })
            .unwrap_or_default();
        out.push_str(&format!("**@{user}**{inline}\n\n{text}\n\n"));
    }
}

// ── Issue detail ──

#[derive(Deserialize)]
struct BbIssueDetail {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    title: String,
    content: Option<RenderedContent>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    kind: String,
    reporter: Option<User>,
}

async fn issue_detail(repo: &str, number: u64, lang: Lang) -> Result<DetailPayload, String> {
    let issue: BbIssueDetail =
        parse(&run_bkt(&["api", &format!("/repositories/{repo}/issues/{number}")]).await?)?;
    let comments = fetch_comments(format!("/repositories/{repo}/issues/{number}/comments")).await;

    let mut out = format!("#{} {}\n", issue.id, issue.title);
    let reporter = issue.reporter.map(|u| u.login()).unwrap_or_default();
    out.push_str(&format!(
        "{} · {} · @{reporter}\n\n",
        issue.state, issue.kind
    ));
    if let Some(body) = issue.content.as_ref().filter(|c| !c.raw.trim().is_empty()) {
        out.push_str(body.raw.trim());
        out.push_str("\n\n");
    }
    append_comments(&mut out, &comments, lang);
    Ok(DetailPayload {
        body: out,
        markdown: true,
        run_active: false,
    })
}

// ── Commit detail ──

#[derive(Deserialize)]
struct BbDiffStat {
    #[serde(default)]
    status: String,
    #[serde(default)]
    lines_added: i64,
    #[serde(default)]
    lines_removed: i64,
    new: Option<DiffPath>,
    old: Option<DiffPath>,
}

#[derive(Deserialize)]
struct DiffPath {
    #[serde(default)]
    path: String,
}

/// Bitbucket diffstat status → single-char mark (same notation as github status_mark).
fn status_mark(status: &str) -> char {
    match status {
        "added" => 'A',
        "removed" => 'D',
        "renamed" => 'R',
        "modified" => 'M',
        _ => '·',
    }
}

async fn commit_detail(repo: &str, sha: &str) -> Result<DetailPayload, String> {
    let c: BbCommit =
        parse(&run_bkt(&["api", &format!("/repositories/{repo}/commit/{sha}")]).await?)?;
    let files: Vec<BbDiffStat> = run_bkt(&["api", &format!("/repositories/{repo}/diffstat/{sha}")])
        .await
        .ok()
        .and_then(|b| parse::<Page<BbDiffStat>>(&b).ok())
        .map(|p| p.values)
        .unwrap_or_default();

    let name = c
        .author
        .as_ref()
        .map(commit_author_name)
        .unwrap_or_default();

    let mut out = format!("commit {}\nAuthor: {name}\nDate:   {}\n\n", c.hash, c.date);
    out.push_str(c.message.trim());
    out.push_str("\n\n");
    if !files.is_empty() {
        let add: i64 = files.iter().map(|f| f.lines_added).sum();
        let del: i64 = files.iter().map(|f| f.lines_removed).sum();
        out.push_str(&format!("─── {} files · +{add} −{del} ───\n", files.len()));
        for f in &files {
            let path = f
                .new
                .as_ref()
                .or(f.old.as_ref())
                .map(|p| p.path.as_str())
                .unwrap_or("?");
            out.push_str(&format!(
                "  {} +{:<4} −{:<4} {path}\n",
                status_mark(&f.status),
                f.lines_added,
                f.lines_removed
            ));
        }
    }
    Ok(DetailPayload {
        body: out,
        markdown: false,
        run_active: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_common_bkt_errors() {
        // No active context → setup command guidance (includes context create/use).
        let h = humanize_bkt_error(
            "Error: no active context; run `bkt context use <name>`".into(),
            Lang::Ko,
        );
        assert!(h.contains("context create"));
        assert!(h.contains("--set-active"));
        assert!(h.contains("context use"));
        // Not authenticated → auth login guidance.
        let a = humanize_bkt_error("not authenticated for host".into(), Lang::En);
        assert!(a.contains("bkt auth login"));
        // Other errors keep the original.
        let raw = "some other failure".to_string();
        assert_eq!(humanize_bkt_error(raw.clone(), Lang::En), raw);
    }

    #[test]
    fn maps_state_vocabulary_to_github() {
        assert_eq!(map_status("COMPLETED", None), "completed");
        assert_eq!(map_status("IN_PROGRESS", None), "in_progress");
        assert_eq!(map_status("IN_PROGRESS", Some("RUNNING")), "in_progress");
        assert_eq!(map_status("PENDING", None), "queued");
        // In-progress halted at a manual step (stage PAUSED/HALTED) → waiting, not running.
        assert_eq!(map_status("IN_PROGRESS", Some("PAUSED")), "waiting");
        assert_eq!(map_status("IN_PROGRESS", Some("HALTED")), "waiting");
        // Unknown/terminal states fall back to completed (prevents permanent active).
        assert_eq!(map_status("ERROR", None), "completed");
        assert_eq!(map_status("HALTED", None), "completed");
        assert_eq!(map_conclusion("SUCCESSFUL"), "success");
        assert_eq!(map_conclusion("FAILED"), "failure");
        assert_eq!(map_conclusion("STOPPED"), "cancelled");
    }

    #[test]
    fn pipeline_json_maps_to_run() {
        let json = br#"{
            "build_number": 21,
            "state": { "name": "COMPLETED", "result": { "name": "FAILED" } },
            "target": {
                "ref_name": "master",
                "selector": { "type": "default" },
                "commit": { "message": "fix(slack): add scope\n\nbody line" }
            },
            "trigger": { "name": "PUSH" },
            "created_on": "2018-12-05T09:37:53.276Z"
        }"#;
        let p: BbPipeline = serde_json::from_slice(json).unwrap();
        let r = pipeline_to_run("ws/repo", p);
        assert_eq!(r.database_id, 21);
        assert_eq!(r.status, "completed");
        assert_eq!(r.conclusion.as_deref(), Some("failure"));
        assert_eq!(r.head_branch, "master");
        assert_eq!(r.event, "push");
        // build number (#21) in the workflow_name slot.
        assert_eq!(r.workflow_name, "#21");
        // Title is the first line of the commit message.
        assert_eq!(r.display_title, "fix(slack): add scope");
        assert!(r.url.contains("/pipelines/results/21"));

        // Falls back to the build number when there's no commit message. Also, if in-progress, conclusion is None even with a result.
        let json2 = br#"{"build_number":9,"state":{"name":"IN_PROGRESS","result":{"name":"FAILED"}},"target":{"ref_name":"dev"},"trigger":{"name":"MANUAL"},"created_on":"2026-01-01T00:00:00Z"}"#;
        let p2: BbPipeline = serde_json::from_slice(json2).unwrap();
        let r2 = pipeline_to_run("ws/repo", p2);
        assert_eq!(r2.display_title, "#9");
        assert_eq!(r2.status, "in_progress");
        assert_eq!(r2.conclusion, None); // conclusion unset even with a result when in-progress
        assert_eq!(r2.workflow_name, "#9"); // build number

        // Even if the message starts with blank lines, use the first non-empty line as the title.
        let json3 = br#"{"build_number":3,"state":{"name":"COMPLETED","result":{"name":"SUCCESSFUL"}},"target":{"commit":{"message":"\n\n  real subject\nbody"}},"trigger":{"name":"PUSH"},"created_on":"2026-01-01T00:00:00Z"}"#;
        let r3 = pipeline_to_run("ws/repo", serde_json::from_slice(json3).unwrap());
        assert_eq!(r3.display_title, "real subject");
        assert_eq!(r3.conclusion.as_deref(), Some("success"));

        // In-progress but halted at a manual step (stage PAUSED) → waiting, not in_progress.
        let json4 = br#"{"build_number":583,"state":{"name":"IN_PROGRESS","type":"pipeline_state_in_progress","stage":{"name":"PAUSED","type":"pipeline_state_in_progress_paused"}},"target":{"ref_name":"master"},"trigger":{"name":"PUSH"},"created_on":"2026-01-01T00:00:00Z"}"#;
        let r4 = pipeline_to_run("ws/repo", serde_json::from_slice(json4).unwrap());
        assert_eq!(r4.status, "waiting");
        assert_eq!(r4.conclusion, None);
    }

    #[test]
    fn commit_json_maps_with_raw_fallback() {
        // With no user mapping, extract just the name from raw; login is None.
        let json = br#"{
            "hash": "c3a491a6b71686cd1832354715eb112ceeac0cc8",
            "date": "2018-12-06T06:37:40+00:00",
            "message": "first line\n\nbody",
            "author": { "raw": "Jane Doe <jane@example.com>" },
            "links": { "html": { "href": "https://bitbucket.org/x/y/commits/c3a491a" } }
        }"#;
        let c: BbCommit = serde_json::from_slice(json).unwrap();
        let m = commit_to_commit(c);
        assert_eq!(m.summary(), "first line");
        assert_eq!(m.author_name(), "Jane Doe");
        assert!(m.author.is_none());
        assert!(m.html_url.contains("bitbucket.org"));

        // Even with a user object, an empty identifier means author is None and falls back to the raw name.
        let json2 = br#"{"hash":"abc1234","date":"2026-01-01T00:00:00Z","message":"msg","author":{"raw":"Real Name <e@x>","user":{}}}"#;
        let m2 = commit_to_commit(serde_json::from_slice(json2).unwrap());
        assert!(m2.author.is_none());
        assert_eq!(m2.author_name(), "Real Name");
    }

    #[test]
    fn pr_json_maps_branch_and_author() {
        let json = br#"{
            "id": 7, "title": "feat", "draft": true,
            "author": { "nickname": "alice" },
            "updated_on": "2026-06-01T00:00:00Z",
            "source": { "branch": { "name": "feature/x" } },
            "participants": [{"approved": true, "state": "approved"}],
            "links": { "html": { "href": "https://bitbucket.org/x/y/pull-requests/7" } }
        }"#;
        let p: BbPr = serde_json::from_slice(json).unwrap();
        let pr = pr_to_pr(p);
        assert_eq!(pr.number, 7);
        assert!(pr.is_draft);
        assert_eq!(pr.author.unwrap().login, "alice");
        assert_eq!(pr.head_ref_name, "feature/x");
        assert_eq!(pr.review_decision.as_deref(), Some("APPROVED"));
    }

    #[test]
    fn branch_json_maps_with_default_flag() {
        let json = br#"{
            "name": "master",
            "target": {
                "hash": "abc12345",
                "date": "2026-06-05T09:16:41+00:00",
                "message": "fix: something\n\nbody",
                "author": { "user": { "nickname": "Charles" }, "raw": "Charles <c@x>" }
            },
            "links": { "html": { "href": "https://bitbucket.org/x/y/branch/master" } }
        }"#;
        let br = branch_to_branch(serde_json::from_slice(json).unwrap(), "master");
        assert_eq!(br.name, "master");
        assert!(br.is_default);
        assert_eq!(br.summary(), "fix: something");
        assert_eq!(br.author, "Charles");
        assert_eq!(br.commit_sha, "abc12345");
        assert!(br.url.contains("/branch/master"));
        // is_default=false when it's not the default branch.
        let b2 = branch_to_branch(serde_json::from_slice(json).unwrap(), "main");
        assert!(!b2.is_default);
    }

    #[test]
    fn pr_review_decision_aggregates_participants() {
        let mk = |approved: bool, state: Option<&str>| Participant {
            approved,
            state: state.map(String::from),
        };
        // Any changes-requested takes priority over approval.
        assert_eq!(
            review_decision(&[
                mk(true, Some("approved")),
                mk(false, Some("changes_requested"))
            ])
            .as_deref(),
            Some("CHANGES_REQUESTED")
        );
        // Approval is recognized via the approved flag or state.
        assert_eq!(
            review_decision(&[mk(true, None)]).as_deref(),
            Some("APPROVED")
        );
        assert_eq!(
            review_decision(&[mk(false, Some("approved"))]).as_deref(),
            Some("APPROVED")
        );
        // None when there's no action.
        assert_eq!(review_decision(&[mk(false, None)]), None);
        assert_eq!(review_decision(&[]), None);
    }
}
