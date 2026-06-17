//! argus — real-time GitHub/Bitbucket repo monitoring TUI dashboard backed by the gh/bkt CLI.
//! (The executable command is `arg`.)
//!
//! Usage:
//!   arg [<repo>] [poll_secs]
//!
//!   <repo>      GitHub owner/repo or Bitbucket workspace/repo_slug.
//!               If omitted, infers backend/repo from the current directory's git remote
//!   poll_secs   Auto-refresh interval (seconds). Default 15

mod app;
mod backend;
mod bitbucket;
mod config;
mod github;
mod i18n;
mod markdown;
mod theme;
mod ui;
mod update;

use std::time::Duration;

use app::App;
use backend::Backend;
use config::Config;
use i18n::Lang;
use ratatui::crossterm::event::{self, Event};
use theme::Theme;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("arg: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    // Argument parsing: positional (repo, poll_secs) + `--theme <name>` / `--lang <code>` flags.
    let mut repo_arg = None;
    let mut poll_arg = None;
    let mut theme_arg = None;
    let mut lang_arg = None;
    let mut show_help = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            // Help text is language-aware, so resolve the language first (below) before printing.
            "-h" | "--help" => show_help = true,
            "--theme" | "-t" => theme_arg = args.next(),
            s if s.starts_with("--theme=") => {
                theme_arg = Some(s.trim_start_matches("--theme=").to_string());
            }
            "--lang" => lang_arg = args.next(),
            s if s.starts_with("--lang=") => {
                lang_arg = Some(s.trim_start_matches("--lang=").to_string());
            }
            _ if repo_arg.is_none() => repo_arg = Some(a),
            _ if poll_arg.is_none() => poll_arg = Some(a),
            _ => {}
        }
    }

    // Load config file (CLI args always take precedence).
    let config = Config::load();

    // lang: --lang > ARGUS_LANG > config lang > system locale > en.
    let lang = Lang::resolve(lang_arg.as_deref(), config.lang.as_deref());
    i18n::set_lang(lang); // mirror for deep error paths (e.g. bitbucket::run_bkt)

    if show_help {
        print_usage(lang);
        return Ok(());
    }

    // backend: config file's backend key (github=gh, bitbucket/bb=bkt). Always wins when specified.
    let cfg_backend = config
        .backend
        .as_deref()
        .map(|s| Backend::from_name(Some(s)));

    // repo/backend: if a CLI arg/config repo exists, use it as-is (backend from config or github);
    // otherwise auto-infer both backend and repo from the current directory's git remote.
    let (backend, repo) = match repo_arg.or(config.repo) {
        // If the repo arg is URL-shaped, parse backend/repo from it (config backend wins if present).
        Some(r) => match backend::parse_remote_url(&r) {
            Some((detected, repo)) => (cfg_backend.unwrap_or(detected), repo),
            None => (cfg_backend.unwrap_or(Backend::Github), r),
        },
        None => {
            let (detected, repo) = backend::detect_remote()
                .await
                .map_err(|e| i18n::err_no_repo(lang, e))?;
            // Honor backend if specified in config, otherwise use the backend detected from the remote.
            (cfg_backend.unwrap_or(detected), repo)
        }
    };

    // Preflight (before entering the TUI): checks CLI install/auth/bkt context and, if something is
    // missing, repairs it guided-style (install/login after consent, bkt context automatically). Exits if not installed and unable to proceed.
    backend::preflight(backend, &repo, lang).await?;
    // poll_secs: CLI arg > config file > default 15 (minimum 2)
    let poll_secs: u64 = poll_arg
        .and_then(|s| s.parse().ok())
        .or(config.poll_secs)
        .unwrap_or(15)
        .max(2);
    // limit: config file > default 20 (1..=100)
    let limit: usize = config.limit.unwrap_or(20).clamp(1, 100);
    // theme: CLI --theme > env var ARGUS_THEME > config file > default
    let theme_name = theme_arg
        .or_else(|| std::env::var("ARGUS_THEME").ok())
        .or(config.theme)
        .unwrap_or_else(|| "default".into());
    let theme = Theme::from_name(&theme_name);

    // ── Channel setup ──
    let (data_tx, mut data_rx) = mpsc::channel::<github::DataMsg>(32);
    let (refresh_tx, refresh_rx) = mpsc::channel::<github::FetchCmd>(4);
    let (detail_tx, detail_rx) = mpsc::channel::<github::DetailReq>(4);
    let (input_tx, mut input_rx) = mpsc::channel::<Event>(64);

    // Dedicated input OS thread: crossterm read is blocking, so read it on a separate thread and forward to a channel.
    std::thread::spawn(move || {
        // On read error (EOF etc.) the while let ends naturally. On send failure (UI exited) break.
        while let Ok(ev) = event::read() {
            if input_tx.blocking_send(ev).is_err() {
                break;
            }
        }
    });

    // Detail preview worker + background polling task.
    tokio::spawn(github::detail_worker(
        detail_rx,
        data_tx.clone(),
        backend,
        lang,
    ));
    // Update notifier (opt out via ARGUS_NO_UPDATE_CHECK or `update_check = false`).
    if std::env::var_os("ARGUS_NO_UPDATE_CHECK").is_none() && config.update_check != Some(false) {
        tokio::spawn(update::check(data_tx.clone()));
    }
    tokio::spawn(github::fetcher(
        repo.clone(),
        backend,
        data_tx,
        refresh_rx,
        poll_secs,
        limit,
    ));

    let mut app = App::new(repo, backend, poll_secs, theme, lang, refresh_tx, detail_tx);

    // Enter terminal (alternate screen + raw mode). Use ratatui::init so it auto-restores on panic.
    let mut terminal = ratatui::init();
    let mut ticker = tokio::time::interval(Duration::from_millis(120));

    let result = event_loop(
        &mut terminal,
        &mut app,
        &mut input_rx,
        &mut data_rx,
        &mut ticker,
    )
    .await;

    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    input_rx: &mut mpsc::Receiver<Event>,
    data_rx: &mut mpsc::Receiver<github::DataMsg>,
    ticker: &mut tokio::time::Interval,
) -> Result<(), String> {
    loop {
        terminal
            .draw(|f| ui::render(f, app))
            .map_err(|e| format!("render failed: {e}"))?;

        if app.should_quit {
            return Ok(());
        }

        tokio::select! {
            maybe_ev = input_rx.recv() => {
                match maybe_ev {
                    Some(ev) => app.handle_event(ev),
                    None => return Ok(()), // input thread ended
                }
            }
            maybe_msg = data_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    app.apply(msg);
                }
            }
            _ = ticker.tick() => {
                app.tick();
            }
        }
    }
}

fn print_usage(lang: Lang) {
    println!("{}", i18n::usage(lang, theme::NAMES.join(" · ")));
}
