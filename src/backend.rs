//! Data source backend selection. GitHub(`gh`) / Bitbucket Cloud(`bkt`).
//!
//! argus's polling and detail layers invoke different CLIs as subprocesses depending on the backend.
//! Authentication and networking are fully delegated to the respective CLI (`gh` or `bkt`) — argus
//! never handles tokens directly. For Bitbucket, log in beforehand via `bkt auth login --web`.

use serde::Deserialize;
use tokio::process::Command;

use crate::i18n::{self, Lang};

/// Runs a CLI subprocess non-tty and returns its stdout (JSON) bytes. On failure, returns stderr
/// as the error message. Shared by the `gh`/`bkt` backends (auth/network delegated to the CLI).
pub async fn run_cli(bin: &str, args: &[&str], err_hint: &str) -> Result<Vec<u8>, String> {
    let output = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{err_hint}: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(err.trim().to_string());
    }
    Ok(output.stdout)
}

/// Parses JSON bytes into a type (error is a String since it gets shown directly in the UI).
pub fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("JSON parse failed: {e}"))
}

/// Which CLI to use as the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `gh` CLI → GitHub. repo is `owner/repo`.
    Github,
    /// `bkt` CLI → Bitbucket Cloud. repo is `workspace/repo_slug`.
    Bitbucket,
}

impl Backend {
    /// Resolves a backend name from config/args. Unspecified or unknown falls back to GitHub.
    pub fn from_name(name: Option<&str>) -> Self {
        match name.map(str::trim).map(str::to_lowercase).as_deref() {
            Some("bitbucket") | Some("bb") | Some("bitbucket-cloud") => Backend::Bitbucket,
            _ => Backend::Github,
        }
    }

    /// Human-readable backend name.
    pub fn label(self) -> &'static str {
        match self {
            Backend::Github => "GitHub",
            Backend::Bitbucket => "Bitbucket",
        }
    }

    /// CLI executable name this backend delegates to.
    pub fn cli(self) -> &'static str {
        match self {
            Backend::Github => "gh",
            Backend::Bitbucket => "bkt",
        }
    }

    /// Auth command (shared for guidance and auto-run when unauthenticated).
    pub fn auth_hint(self) -> &'static str {
        match self {
            Backend::Github => "gh auth login",
            Backend::Bitbucket => "bkt auth login https://bitbucket.org --kind cloud --web",
        }
    }

    /// Install/auth guidance shown when the CLI is not installed (language-aware).
    pub fn install_hint(self, lang: Lang) -> &'static str {
        match self {
            Backend::Github => i18n::install_hint_gh(lang),
            Backend::Bitbucket => i18n::install_hint_bkt(lang),
        }
    }
}

/// Checks whether the backend CLI is on PATH (tries `--version`, exit code ignored).
pub async fn cli_available(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok()
}

// ─── Preflight (install/auth/context checks and fixes before entering the TUI) ───────────────────
//
// Doesn't just "judge"; if something is missing it resolves it guided-style (install/login after
// consent, bkt context automatically). If everything is fine it passes through silently. Auth and
// network are still delegated to the respective CLI; here we only call that CLI's helper commands
// (install/auth/context).

/// Checks and repairs backend readiness before entering the TUI. Returns Err(guidance) if blocked.
pub async fn preflight(backend: Backend, repo: &str, lang: Lang) -> Result<(), String> {
    let bin = backend.cli();

    // 1) CLI install — if missing, install after consent (when brew exists), else guide and exit.
    if !cli_available(bin).await && !offer_install(backend, lang).await {
        return Err(i18n::cli_not_found(
            lang,
            backend.label().to_string(),
            bin.to_string(),
            backend.install_hint(lang).to_string(),
        ));
    }

    // 2) Auth — if not authenticated, run login after consent (browser OAuth).
    if !is_authenticated(backend).await {
        offer_auth(backend, lang).await;
    }

    // 3) bkt context — if authenticated but no active context, auto-create and activate (no consent, notify only).
    if backend == Backend::Bitbucket {
        ensure_bkt_context(repo, lang).await;
    }

    Ok(())
}

/// If brew exists, installs the backend CLI after consent. Returns true once installed and available.
async fn offer_install(backend: Backend, lang: Lang) -> bool {
    if !cli_available("brew").await {
        return false; // caller prints install_hint (both brew/go)
    }
    let pkg = brew_pkg(backend);
    if !prompt_yes(&i18n::install_offer(
        lang,
        backend.cli().to_string(),
        pkg.to_string(),
    )) {
        return false;
    }
    eprintln!("▶ brew install {pkg} …");
    let ok = Command::new("brew")
        .args(["install", pkg])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    ok && cli_available(backend.cli()).await
}

/// Whether the backend CLI is authenticated (success of `<cli> auth status`).
async fn is_authenticated(backend: Backend) -> bool {
    Command::new(backend.cli())
        .args(["auth", "status"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// When unauthenticated, runs the login command after consent (browser OAuth, I/O passed through).
async fn offer_auth(backend: Backend, lang: Lang) {
    if !prompt_yes(&i18n::auth_offer(lang, backend.label().to_string())) {
        eprintln!("{}", i18n::auth_skip(lang, backend.auth_hint().to_string()));
        return;
    }
    let status = match backend {
        Backend::Github => Command::new("gh").args(["auth", "login"]).status().await,
        Backend::Bitbucket => {
            Command::new("bkt")
                .args([
                    "auth",
                    "login",
                    "https://bitbucket.org",
                    "--kind",
                    "cloud",
                    "--web",
                ])
                .status()
                .await
        }
    };
    match status {
        Ok(s) if s.success() => eprintln!("{}", i18n::login_done(lang)),
        _ => eprintln!(
            "{}",
            i18n::login_failed(lang, backend.auth_hint().to_string())
        ),
    }
}

/// If bkt has no active context, auto-creates and activates one (host=api.bitbucket.org, workspace inferred from repo).
async fn ensure_bkt_context(repo: &str, lang: Lang) {
    if bkt_has_active_context().await {
        return;
    }
    let ws = workspace_of(repo);
    eprintln!("{}", i18n::ctx_auto(lang, ws.to_string()));
    let null = || std::process::Stdio::null();
    let created = Command::new("bkt")
        .args([
            "context",
            "create",
            "argus",
            "--host",
            "api.bitbucket.org",
            "--workspace",
            ws,
            "--set-active",
        ])
        .stdin(null())
        .stdout(null())
        .stderr(null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if created {
        eprintln!("{}", i18n::ctx_created(lang));
        return;
    }
    // A context with the same name may already exist, so just try to activate it.
    let used = Command::new("bkt")
        .args(["context", "use", "argus"])
        .stdin(null())
        .stdout(null())
        .stderr(null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if used {
        eprintln!("{}", i18n::ctx_reused(lang));
    } else {
        eprintln!("{}", i18n::ctx_failed(lang, ws.to_string()));
    }
}

/// True if `bkt context list` output contains an active marker (`*`).
async fn bkt_has_active_context() -> bool {
    match run_cli("bkt", &["context", "list"], "").await {
        Ok(out) => has_active_marker(&String::from_utf8_lossy(&out)),
        Err(_) => false,
    }
}

/// `owner/repo` → `owner` (workspace). If there's no slash, returns the input as-is.
fn workspace_of(repo: &str) -> &str {
    repo.split('/').next().unwrap_or(repo)
}

/// Whether the context list output has an active marker (`*`) (indentation ignored).
fn has_active_marker(list: &str) -> bool {
    list.lines().any(|l| l.trim_start().starts_with('*'))
}

/// Per-backend brew package (formula/tap).
fn brew_pkg(backend: Backend) -> &'static str {
    match backend {
        Backend::Github => "gh",
        Backend::Bitbucket => "avivsinai/tap/bitbucket-cli",
    }
}

/// Asks a y/N question on the terminal; true on consent (default N). Non-interactive (EOF)/input error → N.
fn prompt_yes(msg: &str) -> bool {
    use std::io::Write;
    eprint!("{msg} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Infers (backend, `owner/repo`) from the current directory's git `origin` remote.
/// Parses the remote URL directly without relying on gh, so it detects both GitHub and Bitbucket at once.
pub async fn detect_remote() -> Result<(Backend, String), String> {
    let out = run_cli(
        "git",
        &["remote", "get-url", "origin"],
        "git failed to run (not a git repo?)",
    )
    .await?;
    let url = String::from_utf8_lossy(&out);
    parse_remote_url(url.trim()).ok_or_else(|| {
        format!(
            "could not recognize a GitHub/Bitbucket repo from the git remote: {}",
            url.trim()
        )
    })
}

/// git remote URL (or a user-supplied URL) → (backend, `owner/repo`). None if unrecognized.
/// Supports: scp form (`[user@]host:owner/repo.git`) and `https`/`http`/`ssh`/`git` scheme forms.
/// Ignores userinfo (`user@`) and port (`:NNNN`) before the host — handles `https://user@host/…` forms.
pub fn parse_remote_url(url: &str) -> Option<(Backend, String)> {
    let url = url.trim();
    // Strip the scheme. If there's no scheme, treat it as scp form (`[user@]host:owner/repo.git`).
    let no_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .or_else(|| url.strip_prefix("git://"));
    let (host, path) = match no_scheme {
        Some(rest) => rest.split_once('/')?, // [user@]host[:port]/owner/repo.git
        None => url.split_once(':')?,        // scp: [user@]host:owner/repo.git
    };
    // Strip userinfo (user@) and port (:NNNN), leaving the bare host.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    // Substring match so SSH config Host aliases (e.g. `github.com-work`, common with
    // multiple accounts) and Enterprise/Server hosts (`github.mycorp.com`) still resolve.
    // Auth is delegated to gh/bkt, so a loose backend guess here is safe.
    let host_l = host.to_lowercase();
    let backend = if host_l.contains("github") {
        Backend::Github
    } else if host_l.contains("bitbucket") {
        Backend::Bitbucket
    } else {
        return None;
    };
    // Clean the path: strip leading/trailing '/' and '.git', keeping just the owner/repo segments.
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let repo = repo.split('/').next().unwrap_or(repo);
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((backend, format!("{owner}/{repo}")))
}

#[cfg(test)]
mod tests {
    use super::Backend;
    use crate::i18n::Lang;

    #[test]
    fn cli_names_and_hints() {
        assert_eq!(Backend::Github.cli(), "gh");
        assert_eq!(Backend::Bitbucket.cli(), "bkt");
        assert_eq!(Backend::Github.label(), "GitHub");
        // The install guidance contains the auth command (both languages).
        assert!(
            Backend::Github
                .install_hint(Lang::En)
                .contains("gh auth login")
        );
        assert!(
            Backend::Github
                .install_hint(Lang::Ko)
                .contains("gh auth login")
        );
        assert!(
            Backend::Bitbucket
                .install_hint(Lang::En)
                .contains("bkt auth login")
        );
        // Auth guidance (auth_hint) — a command, language-neutral.
        assert_eq!(Backend::Github.auth_hint(), "gh auth login");
        assert!(
            Backend::Bitbucket
                .auth_hint()
                .contains("--kind cloud --web")
        );
    }

    #[test]
    fn preflight_pure_helpers() {
        use super::{brew_pkg, has_active_marker, workspace_of};
        // repo → workspace.
        assert_eq!(workspace_of("acme/web-app"), "acme");
        assert_eq!(workspace_of("noslash"), "noslash");
        // Detect the active marker (*) in context list output (indentation ignored).
        assert!(has_active_marker(
            "* argus (host: api.bitbucket.org)\n  other"
        ));
        assert!(has_active_marker("  * indented"));
        assert!(!has_active_marker("  argus (host: ...)\n  other"));
        assert!(!has_active_marker(""));
        // brew package.
        assert_eq!(brew_pkg(Backend::Github), "gh");
        assert_eq!(brew_pkg(Backend::Bitbucket), "avivsinai/tap/bitbucket-cli");
    }

    #[test]
    fn resolves_names() {
        assert_eq!(Backend::from_name(Some("bitbucket")), Backend::Bitbucket);
        assert_eq!(Backend::from_name(Some("BB")), Backend::Bitbucket);
        assert_eq!(
            Backend::from_name(Some(" Bitbucket-Cloud ")),
            Backend::Bitbucket
        );
        assert_eq!(Backend::from_name(Some("github")), Backend::Github);
        assert_eq!(Backend::from_name(None), Backend::Github);
        // Unknown names safely fall back to github.
        assert_eq!(Backend::from_name(Some("gitlab")), Backend::Github);
    }

    #[test]
    fn parses_remote_urls() {
        use super::parse_remote_url as p;
        // GitHub — scp form / https form (with/without .git)
        assert_eq!(
            p("git@github.com:cli/cli.git"),
            Some((Backend::Github, "cli/cli".into()))
        );
        assert_eq!(
            p("https://github.com/cli/cli.git"),
            Some((Backend::Github, "cli/cli".into()))
        );
        assert_eq!(
            p("https://github.com/cli/cli"),
            Some((Backend::Github, "cli/cli".into()))
        );
        // Bitbucket — scp form / https form / ssh form
        assert_eq!(
            p("git@bitbucket.org:acme/widget.git"),
            Some((Backend::Bitbucket, "acme/widget".into()))
        );
        assert_eq!(
            p("https://bitbucket.org/acme/widget"),
            Some((Backend::Bitbucket, "acme/widget".into()))
        );
        assert_eq!(
            p("ssh://git@bitbucket.org/acme/widget.git"),
            Some((Backend::Bitbucket, "acme/widget".into()))
        );
        // https form with embedded userinfo (user@) — Bitbucket commonly generates these (also checks hyphenated repo).
        assert_eq!(
            p("https://alice@bitbucket.org/acme/web-app.git"),
            Some((Backend::Bitbucket, "acme/web-app".into()))
        );
        assert_eq!(
            p("https://user@github.com/cli/cli.git"),
            Some((Backend::Github, "cli/cli".into()))
        );
        // ssh form with a port still recognizes only the host.
        assert_eq!(
            p("ssh://git@github.com:22/cli/cli.git"),
            Some((Backend::Github, "cli/cli".into()))
        );
        // SSH config Host aliases (multiple accounts) and Enterprise/Server hosts resolve too.
        assert_eq!(
            p("git@github.com-polarian:polarian/argus.git"),
            Some((Backend::Github, "polarian/argus".into()))
        );
        assert_eq!(
            p("git@bitbucket.org-work:team/repo.git"),
            Some((Backend::Bitbucket, "team/repo".into()))
        );
        assert_eq!(
            p("https://github.mycorp.com/org/app.git"),
            Some((Backend::Github, "org/app".into()))
        );
        // Unsupported host / form → None
        assert_eq!(p("git@gitlab.com:foo/bar.git"), None);
        assert_eq!(p("not a url"), None);
        assert_eq!(p("https://github.com/"), None);
    }
}
