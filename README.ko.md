# argus 🦚

[English](README.md) · **한국어**

[![crates.io](https://img.shields.io/crates/v/argus-tui.svg)](https://crates.io/crates/argus-tui)
[![Release](https://img.shields.io/github/v/release/polarian/argus.svg)](https://github.com/polarian/argus/releases)
[![CI](https://github.com/polarian/argus/actions/workflows/ci.yml/badge.svg)](https://github.com/polarian/argus/actions/workflows/ci.yml)
[![Stars](https://img.shields.io/github/stars/polarian/argus.svg)](https://github.com/polarian/argus/stargazers)
[![MSRV](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/crates/l/argus-tui.svg)](LICENSE)

`gh` / `bkt` CLI를 백엔드로 써서 GitHub·Bitbucket 저장소의 **Actions / Pull Requests / Issues / Commits** 를 터미널에서 실시간으로 지켜보는 대시보드. 주기적으로 폴링하며 신규·변경 항목을 `●`로 표시합니다.

## 기능

- **4개 라이브 패널** — Actions · PRs · Issues · Commits, 반응형 2×2 / 4×1 / 1×4
- **변화 감지** — 직전 폴링 대비 신규·변경 항목을 `●`로 표시
- **상세 미리보기**(`v`) — 진행 중 run은 잡/스텝 트리 + **라이브 로그·타임라인**(완료까지 follow), PR/Issue 본문 + 리뷰·코멘트 타임라인, 커밋 diff
- **검색**(`/`) — 패널별 부분 문자열 필터(상태·라벨·작성자 포함)
- **GitHub·Bitbucket** — git remote에서 자동 감지, 인증은 전적으로 `gh`/`bkt`에 위임(토큰 직접 안 다룸)
- **en/ko UI**, 5개 색 테마, 시작 시 업데이트 알림

## 설치

**Rust 불필요** — macOS / Linux용 미리 빌드 바이너리(설치 위치가 `PATH`에 없으면 설치기가 알려줍니다):

```bash
curl -fsSL https://raw.githubusercontent.com/polarian/argus/master/install.sh | sh
```

**Cargo로** (크레이트명 `argus-tui`, 명령은 `arg`):

```bash
cargo binstall argus-tui    # 미리빌드 바이너리 (cargo-binstall 필요)
cargo install argus-tui     # 소스 빌드
```

> **Rust가 아직 없다면?** [rustup](https://rustup.rs)으로 설치하세요 — `~/.cargo/bin`을 `PATH`에 자동 추가해 설치된 `arg`를 바로 찾습니다. (Homebrew의 `rust`는 이 작업을 안 해서 `cargo install`한 바이너리가 `PATH`에 안 잡힙니다.)

`cargo binstall`은 [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall)이 먼저 설치돼 있어야 합니다(`cargo install cargo-binstall`). 최신 개발본은 `cargo install --git https://github.com/polarian/argus`. 또는 [Releases](https://github.com/polarian/argus/releases)에서 tar.gz. 바이너리를 CLI로 받으므로 **macOS가 격리하지 않습니다 — 서명·공증 불필요.**

> **백엔드 CLI**는 쓰는 쪽만 별도 설치: GitHub은 [`gh`](https://cli.github.com/), Bitbucket은 [`bkt`](https://github.com/avivsinai/bitbucket-cli). 최초 실행 시 argus가 설정을 안내합니다.

**업데이트:** 새 릴리스가 있으면 `⬆ vX.Y.Z 사용 가능` 배너가 뜹니다. `cargo install-update -a`(cargo/binstall 설치) 또는 curl 설치기 재실행으로 갱신.

## 사용법

```bash
arg [repo] [poll_secs] [--theme <이름>] [--lang en|ko]
```

| 인자 | 설명 |
|------|------|
| `repo` | `owner/repo`(GitHub) 또는 `workspace/repo_slug`(Bitbucket). **생략 시 git `origin` remote에서 자동 추론.** |
| `poll_secs` | 새로고침 주기(초, 기본 `15`, 최소 `2`). |
| `--theme`, `-t` | 색 테마(환경변수 `ARGUS_THEME`도 가능). |
| `--lang` | UI 언어 `en`/`ko`(환경변수 `ARGUS_LANG`도 가능, 기본: 시스템 로케일). |

```bash
arg cli/cli          # cli/cli 감시
arg cli/cli 5        # 5초 주기
cd my-repo && arg    # 현재 repo 자동 추론
```

## 키 바인딩

| 키 | 동작 | 키 | 동작 |
|----|------|----|------|
| `Tab` / `Shift+Tab` | 패널 포커스 이동 | `/` | 패널 검색 |
| `↑`/`↓` (`k`/`j`) | 스크롤 | `Enter` / `o` | 브라우저 열기 |
| `Shift`+방향키 | 패널 간 이동 | `v` / `→` | 상세 미리보기 |
| `+` / `-` | 폴링 주기 | `r` | 즉시 새로고침 |
| `q` / `Esc` | 종료 | | |

상세 모달: `↑`/`↓`·`PgUp`/`PgDn` 스크롤, `g`/`G` 맨 위/아래, `l` 로그 뷰 토글(run), `o` 브라우저, `←`/`Esc` 닫기.

## 백엔드

설정의 `backend` 키로 선택(또는 git remote에서 자동 감지). 인증·네트워크는 전적으로 CLI에 위임합니다.

| 백엔드 | CLI | repo 형식 | 인증 |
|--------|-----|-----------|------|
| `github` (기본) | `gh` | `owner/repo` | `gh auth login` |
| `bitbucket` | `bkt` | `workspace/repo_slug` | `bkt auth login https://bitbucket.org --kind cloud --web` |

**보통은 그냥 `arg`만 실행하면 됩니다** — 시작 시 프리플라이트가 CLI 설치·인증·(Bitbucket의) 활성 context를 점검하고 가능한 건 자동 보정합니다(설치·로그인 제안, Bitbucket context 자동 생성).

Bitbucket 참고:
- `bkt`는 **인증 + 활성 context**가 필요합니다. context의 host는 반드시 **`api.bitbucket.org`**(프리플라이트가 자동 처리).
- Actions는 **Pipelines**로 매핑되고, Bitbucket Issues가 폐지돼 Issues 자리에 **활성 Branches**를 표시합니다.
- PR 리뷰 상태는 participants의 승인/변경요청을 집계합니다(단일 reviewDecision 없음).

## 설정

`./argus.toml` → `$XDG_CONFIG_HOME/argus/config.toml` → `~/.argus.toml` 순으로 탐색(첫 번째 발견 사용, CLI 인자 우선). 모든 키 선택적:

```toml
repo = "cli/cli"            # 기본 대상 (없으면 git remote 추론)
poll_secs = 10              # 새로고침 주기(초)
limit = 30                  # 패널당 항목 수 (1–100)
theme = "catppuccin-mocha"  # default · nord · catppuccin-mocha · dracula · tokyo-night
backend = "github"          # github | bitbucket
lang = "en"                 # en | ko (기본: 시스템 로케일)
update_check = true         # 시작 시 릴리스 확인
```

## 라이선스

[MIT](LICENSE) © Polarian
