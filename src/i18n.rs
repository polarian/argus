//! UI text localization (en/ko).
//!
//! A compile-time message catalog: each message lists its English and Korean
//! variants side by side via the `messages!` macro. No-arg messages return
//! `&'static str`; parameterized ones return `String` (format! with inline named
//! captures bound to the function params). Selecting the language is just passing
//! a `Lang` to each accessor. Low-level/developer errors stay English and are not
//! routed through this module (see CLAUDE.md).

/// UI language.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    En,
    Ko,
}

/// Process-wide resolved language. The primary mechanism is passing `Lang`
/// explicitly; this global mirror exists only for deep error paths (e.g.
/// `bitbucket::run_bkt`) where threading `lang` through every caller would be
/// disproportionately invasive. Set once in `main` right after resolution.
static CURRENT: std::sync::OnceLock<Lang> = std::sync::OnceLock::new();

/// Record the resolved language (call once at startup).
pub fn set_lang(lang: Lang) {
    let _ = CURRENT.set(lang);
}

/// The resolved language (falls back to English if `set_lang` wasn't called).
pub fn current() -> Lang {
    CURRENT.get().copied().unwrap_or(Lang::En)
}

impl Lang {
    /// Resolve the language: `--lang` > `ARGUS_LANG` > config `lang` > system
    /// locale (`LC_ALL`/`LC_MESSAGES`/`LANG`) > English.
    pub fn resolve(cli: Option<&str>, cfg: Option<&str>) -> Lang {
        fn env(k: &str) -> Option<Lang> {
            std::env::var(k).ok().as_deref().and_then(Lang::from_name)
        }
        cli.and_then(Lang::from_name)
            .or_else(|| env("ARGUS_LANG"))
            .or_else(|| cfg.and_then(Lang::from_name))
            .or_else(|| {
                env("LC_ALL")
                    .or_else(|| env("LC_MESSAGES"))
                    .or_else(|| env("LANG"))
            })
            .unwrap_or(Lang::En)
    }

    /// Loose name/locale parse: `ko*`/`korean` → Ko, `en*`/`english` → En, else None.
    /// Handles locale forms like `ko_KR.UTF-8`.
    fn from_name(s: &str) -> Option<Lang> {
        let s = s.trim().to_lowercase();
        if s.starts_with("ko") {
            Some(Lang::Ko)
        } else if s.starts_with("en") {
            Some(Lang::En)
        } else {
            None
        }
    }
}

/// Defines a message accessor. No-arg form returns `&'static str`; the
/// parameterized form returns `String` (the en/ko literals reference the params
/// via inline named captures, e.g. `{total}`).
macro_rules! messages {
    ($name:ident => { en: $en:expr, ko: $ko:expr $(,)? }) => {
        pub fn $name(lang: Lang) -> &'static str {
            match lang { Lang::En => $en, Lang::Ko => $ko }
        }
    };
    ($name:ident ($($a:ident : $t:ty),* $(,)?) => { en: $en:expr, ko: $ko:expr $(,)? }) => {
        pub fn $name(lang: Lang, $($a: $t),*) -> String {
            match lang { Lang::En => format!($en), Lang::Ko => format!($ko) }
        }
    };
}

// ─── Panels / empty / loading ────────────────────────────────────────────────
messages! { empty => { en: "no items", ko: "항목 없음" } }
messages! { filtered_empty => { en: "🔍 no matching items", ko: "🔍 일치하는 항목 없음" } }
messages! { loading => { en: "loading…", ko: "불러오는 중…" } }
messages! { loading_short => { en: "loading…", ko: "로딩 중…" } }

// ─── Header / footer ─────────────────────────────────────────────────────────
messages! { poll_suffix => { en: " poll ", ko: " 주기 " } }
messages! { search_confirm_hint => { en: "   Enter confirm · Esc cancel ", ko: "   Enter 확정 · Esc 취소 " } }
messages! { key_panels => { en: "panels", ko: "패널" } }
messages! { key_scroll => { en: "scroll", ko: "스크롤" } }
messages! { key_search => { en: "search", ko: "검색" } }
messages! { key_open => { en: "open", ko: "열기" } }
messages! { key_preview => { en: "preview", ko: "미리보기" } }
messages! { key_refresh => { en: "refresh", ko: "새로고침" } }
messages! { key_interval => { en: "interval", ko: "주기" } }
messages! { key_quit => { en: "quit", ko: "종료" } }
messages! { changed(total: usize) => { en: "   ● {total} changed ", ko: "   ● {total} 변경 " } }
messages! { update_available(ver: String) => { en: "⬆ v{ver} available", ko: "⬆ v{ver} 사용 가능" } }

// ─── Detail modal hints ──────────────────────────────────────────────────────
messages! { modal_hint_run => {
    en: " ↑/↓ scroll · L logs · O browser · ← close ",
    ko: " ↑/↓ 스크롤 · L 로그 · O 브라우저 · ← 닫기 ",
}}
messages! { modal_hint_other => {
    en: " ↑/↓·PgUp/PgDn scroll · O browser · ← close ",
    ko: " ↑/↓·PgUp/PgDn 스크롤 · O 브라우저 · ← 닫기 ",
}}

// ─── Detail body (Actions tree / log / timeline) ─────────────────────────────
messages! { jobs_count(n: usize) => { en: "{n} jobs", ko: "잡 {n}" } }
messages! { n_success(n: usize) => { en: "✓{n} success", ko: "✓{n} 성공" } }
messages! { n_failure(n: usize) => { en: "✗{n} failure", ko: "✗{n} 실패" } }
messages! { n_skipped(n: usize) => { en: "⊝{n} skipped", ko: "⊝{n} 스킵" } }
messages! { n_cancelled(n: usize) => { en: "⊘{n} cancelled", ko: "⊘{n} 취소" } }
messages! { log_view_hint => { en: "L: full per-step log view", ko: "L: 스텝별 전체 로그 보기" } }
messages! { tree_view_hint => { en: "L: switch to tree view", ko: "L: 트리 보기로 전환" } }
messages! { in_progress_auto => { en: "● in progress — auto-refreshing…", ko: "● 진행 중 — 자동 갱신 중…" } }
messages! { in_progress_log_auto => { en: "● in progress — log auto-refreshing…", ko: "● 진행 중 — 로그 자동 갱신 중…" } }
messages! { progress_line(bar: String, done: usize, total: usize, steps: usize) => {
    en: "progress {bar} jobs {done}/{total} · {steps} steps done",
    ko: "진행률 {bar} 잡 {done}/{total} · 스텝 {steps} 완료",
}}
messages! { job_counts(done: usize, active: usize, queued: usize) => {
    en: "✓{done} done · ▸{active} running · ◴{queued} queued",
    ko: "✓{done} 완료 · ▸{active} 진행 · ◴{queued} 대기",
}}
messages! { failure_log_header => { en: "──────── failure log ────────", ko: "──────── 실패 로그 ────────" } }
messages! { no_log => { en: "  (no log)", ko: "  (로그 없음)" } }
messages! { timeline_in_progress => {
    en: "  ● in progress — step timeline (switches to full log on completion)",
    ko: "  ● 진행 중 — 스텝 타임라인(완료 시 전체 로그로 전환)",
}}
messages! { timeline_waiting => { en: "  (waiting for step info…)", ko: "  (스텝 정보 대기 중…)" } }
messages! { step_waiting => { en: "waiting", ko: "대기" } }
messages! { log_not_found => {
    en: "log not found (still preparing or expired)",
    ko: "로그를 찾을 수 없습니다(아직 준비 중이거나 만료됨)",
}}
messages! { log_fetch_failed(first: String) => { en: "failed to fetch log: {first}", ko: "로그를 가져오지 못함: {first}" } }
messages! { lines_omitted(n: usize) => { en: "  … ({n} earlier lines omitted) …", ko: "  … (앞부분 {n}줄 생략) …" } }
messages! { pipeline_header(build: u64) => {
    en: "Pipeline #{build}\nL: switch to tree view\n\n",
    ko: "Pipeline #{build}\nL: 트리 보기로 전환\n\n",
}}

// ─── Detail body (Issues / PRs / Commits) ────────────────────────────────────
messages! { body_none => { en: "_(no body)_", ko: "_(본문 없음)_" } }
messages! { review_approved => { en: "✓ approved", ko: "✓ 승인" } }
messages! { review_changes => { en: "✗ changes requested", ko: "✗ 변경요청" } }
messages! { review_commented => { en: "💬 comment", ko: "💬 코멘트" } }
messages! { review_dismissed => { en: "⊘ dismissed", ko: "⊘ 기각" } }
messages! { review_other => { en: "● review", ko: "● 리뷰" } }
messages! { comments_header(n: usize) => { en: "## 💬 Comments ({n})", ko: "## 💬 코멘트 ({n})" } }
messages! { comments_sep(n: usize) => { en: "--- comments {n} ---", ko: "--- 코멘트 {n} ---" } }
messages! { timeline_header => { en: "## 💬 Timeline", ko: "## 💬 타임라인" } }
messages! { path_general => { en: "(general)", ko: "(일반)" } }
messages! { code_review_header => { en: "## 📄 Code review threads", ko: "## 📄 코드 리뷰 스레드" } }

// ─── App ─────────────────────────────────────────────────────────────────────
messages! { load_failed(e: String) => { en: "load failed:\n\n{e}", ko: "불러오기 실패:\n\n{e}" } }

// ─── Preflight (backend.rs) ──────────────────────────────────────────────────
messages! { install_offer(cli: String, pkg: String) => {
    en: "`{cli}` is not installed — run `brew install {pkg}` now?",
    ko: "`{cli}` 미설치 — `brew install {pkg}` 를 지금 실행할까요?",
}}
messages! { auth_offer(label: String) => {
    en: "{label} is not authenticated — log in now?",
    ko: "{label} 인증이 안 돼 있습니다 — 지금 로그인할까요?",
}}
messages! { auth_skip(cmd: String) => { en: "  (skipped) manual auth: {cmd}", ko: "  (건너뜀) 수동 인증: {cmd}" } }
messages! { login_done => { en: "✓ logged in", ko: "✓ 로그인 완료" } }
messages! { login_failed(cmd: String) => {
    en: "⚠ login did not complete — manual: {cmd}",
    ko: "⚠ 로그인이 완료되지 않았습니다 — 수동: {cmd}",
}}
messages! { ctx_auto(ws: String) => {
    en: "▸ no active bkt context; auto-configuring (host: api.bitbucket.org · workspace: {ws}).",
    ko: "▸ bkt 활성 context가 없어 자동 설정합니다 (host: api.bitbucket.org · workspace: {ws}).",
}}
messages! { ctx_created => { en: "✓ context 'argus' created and activated", ko: "✓ context 'argus' 생성·활성화" } }
messages! { ctx_reused => { en: "✓ activated existing context 'argus'", ko: "✓ 기존 context 'argus' 활성화" } }
messages! { ctx_failed(ws: String) => {
    en: "⚠ context auto-setup failed — manual: bkt context create cloud --host api.bitbucket.org --workspace {ws} --set-active",
    ko: "⚠ context 자동 설정 실패 — 수동: bkt context create cloud --host api.bitbucket.org --workspace {ws} --set-active",
}}
messages! { cli_not_found(label: String, cli: String, hint: String) => {
    en: "Cannot find the `{cli}` CLI required by the {label} backend.\n\n{hint}",
    ko: "{label} 백엔드에 필요한 `{cli}` CLI를 찾을 수 없습니다.\n\n{hint}",
}}
messages! { install_hint_gh => {
    en: concat!(
        "  install: brew install gh   (or https://cli.github.com)\n",
        "  auth:    gh auth login",
    ),
    ko: concat!(
        "  설치: brew install gh   (또는 https://cli.github.com)\n",
        "  인증: gh auth login",
    ),
}}
messages! { install_hint_bkt => {
    en: concat!(
        "  install: brew install avivsinai/tap/bitbucket-cli\n",
        "           or go install github.com/avivsinai/bitbucket-cli/cmd/bkt@latest\n",
        "  auth:    bkt auth login https://bitbucket.org --kind cloud --web",
    ),
    ko: concat!(
        "  설치: brew install avivsinai/tap/bitbucket-cli\n",
        "        또는 go install github.com/avivsinai/bitbucket-cli/cmd/bkt@latest\n",
        "  인증: bkt auth login https://bitbucket.org --kind cloud --web",
    ),
}}

// ─── bkt friendly errors (bitbucket.rs::humanize_bkt_error) ──────────────────
messages! { bkt_no_context => {
    en: concat!(
        "bkt has no active context (one-time setup needed).\n",
        "  setup: bkt context create cloud --host api.bitbucket.org --workspace <workspace> --set-active\n",
        "  (if it exists) bkt context use <name>\n",
        "  check: bkt context list   (host is api.bitbucket.org)",
    ),
    ko: concat!(
        "bkt 활성 context가 없습니다(최초 1회 설정 필요).\n",
        "  설정: bkt context create cloud --host api.bitbucket.org --workspace <워크스페이스> --set-active\n",
        "  (이미 있으면) bkt context use <이름>\n",
        "  확인: bkt context list   (host는 api.bitbucket.org)",
    ),
}}
messages! { bkt_not_authed => {
    en: concat!(
        "bkt is not authenticated.\n",
        "  auth: bkt auth login https://bitbucket.org --kind cloud --web",
    ),
    ko: concat!(
        "bkt 인증이 필요합니다.\n",
        "  인증: bkt auth login https://bitbucket.org --kind cloud --web",
    ),
}}

// ─── Usage / help (main.rs) ──────────────────────────────────────────────────
messages! { usage(themes: String) => {
    en: "argus — real-time GitHub/Bitbucket repo monitoring TUI (command: arg)\n\n\
         Usage:\n  \
         arg [<repo>] [poll_secs] [--theme <name>] [--lang en|ko]\n\n\
         Arguments:\n  \
         <repo>       repository (GitHub owner/repo · Bitbucket workspace/repo_slug). Auto-detected from the git remote if omitted\n  \
         poll_secs    auto-refresh interval (seconds, default 15, min 2)\n  \
         --theme,-t   color theme (or the ARGUS_THEME env var). Takes precedence over the config file\n  \
         --lang       UI language: en | ko (or ARGUS_LANG; default: auto-detect from locale)\n\n\
         Keys:\n  \
         Tab/Shift+Tab  move panel      ↑/↓(J/K)  scroll       / search\n  \
         Enter/O        open browser    V         detail preview  +/- interval\n  \
         R              refresh now      Q/Esc     quit\n\n\
         Config file (first found wins, CLI args take precedence):\n  \
         ./argus.toml · ~/.config/argus/config.toml · ~/.argus.toml\n  \
         keys: repo, poll_secs, limit, theme, backend, lang\n  \
         backend: github(gh·default) · bitbucket(bkt, repo is workspace/repo_slug)\n  \
         themes: {themes}\n\n\
         Layout:\n  \
         auto-switches among 2×2 · 1×4 · 4×1 based on terminal size\n",
    ko: "argus — GitHub·Bitbucket repo 실시간 모니터링 TUI (명령: arg)\n\n\
         사용법:\n  \
         arg [<repo>] [poll_secs] [--theme <이름>] [--lang en|ko]\n\n\
         인자:\n  \
         <repo>       저장소(GitHub owner/repo · Bitbucket workspace/repo_slug). 생략 시 git remote에서 자동 추론\n  \
         poll_secs    자동 새로고침 주기(초, 기본 15, 최소 2)\n  \
         --theme,-t   색 테마 (또는 환경변수 ARGUS_THEME). 설정 파일보다 우선\n  \
         --lang       UI 언어: en | ko (또는 ARGUS_LANG. 기본: 로케일 자동 감지)\n\n\
         키:\n  \
         Tab/Shift+Tab  패널 이동      ↑/↓(J/K)  스크롤       / 검색\n  \
         Enter/O        브라우저 열기   V         상세 미리보기  +/- 주기\n  \
         R              즉시 새로고침    Q/Esc     종료\n\n\
         설정 파일(첫 번째 발견 사용, CLI 인자가 우선):\n  \
         ./argus.toml · ~/.config/argus/config.toml · ~/.argus.toml\n  \
         키: repo, poll_secs, limit, theme, backend, lang\n  \
         backend: github(gh·기본) · bitbucket(bkt, repo는 workspace/repo_slug)\n  \
         테마: {themes}\n\n\
         레이아웃:\n  \
         터미널 크기에 따라 2×2 · 1×4 · 4×1 자동 전환\n",
}}
messages! { err_no_repo(cause: String) => {
    en: "no repo argument and auto-detection failed.\n  usage: arg <owner/repo>\n  cause: {cause}",
    ko: "repo 인자가 없고 자동 추론도 실패했습니다.\n  사용: arg <owner/repo>\n  원인: {cause}",
}}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lang_names() {
        assert_eq!(Lang::from_name("ko"), Some(Lang::Ko));
        assert_eq!(Lang::from_name("Korean"), Some(Lang::Ko));
        assert_eq!(Lang::from_name("ko_KR.UTF-8"), Some(Lang::Ko)); // locale form
        assert_eq!(Lang::from_name("en"), Some(Lang::En));
        assert_eq!(Lang::from_name("en_US"), Some(Lang::En));
        assert_eq!(Lang::from_name("fr"), None);
        assert_eq!(Lang::from_name("C"), None);
    }

    #[test]
    fn explicit_lang_wins() {
        // An explicit --lang arg short-circuits before env/config/locale (deterministic).
        assert_eq!(Lang::resolve(Some("ko"), None), Lang::Ko);
        assert_eq!(Lang::resolve(Some("en"), None), Lang::En);
    }

    #[test]
    fn messages_localize_and_interpolate() {
        assert_eq!(empty(Lang::En), "no items");
        assert_eq!(empty(Lang::Ko), "항목 없음");
        assert!(changed(Lang::En, 3).contains("3 changed"));
        assert!(changed(Lang::Ko, 3).contains("3 변경"));
        assert!(job_counts(Lang::En, 1, 2, 3).contains("running"));
        assert!(job_counts(Lang::Ko, 1, 2, 3).contains("진행"));
    }
}
