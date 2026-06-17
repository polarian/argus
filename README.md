# argus 🦚

**English** · [한국어](README.ko.md)

A terminal dashboard that watches a GitHub or Bitbucket repo's **Actions / Pull Requests / Issues / Commits** in real time, using the `gh` / `bkt` CLI as its backend. It polls on an interval and marks new/changed items with `●`.

## Features

- **4 live panels** — Actions · PRs · Issues · Commits, responsive 2×2 / 4×1 / 1×4
- **Change detection** — `●` marks items new or changed since the last poll
- **Detail preview** (`v`) — job/step tree with **live logs & timeline** for in-progress runs (follows until done); PR/Issue bodies with review·comment timelines; commit diffs
- **Search** (`/`) — per-panel substring filter across status, labels, and authors
- **GitHub & Bitbucket** — auto-detected from the git remote; auth fully delegated to `gh`/`bkt` (no token handling)
- **en/ko UI**, 5 color themes, and a startup update notice

## Install

```bash
# Prebuilt binary (no Rust) — macOS / Linux
curl -fsSL https://raw.githubusercontent.com/polarian/argus/master/install.sh | sh

# With Cargo
cargo binstall argus                                    # prebuilt (needs cargo-binstall)
cargo install --git https://github.com/polarian/argus   # from source
```

`cargo binstall` requires [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) (`cargo install cargo-binstall`, or via Homebrew / its own installer). Or build locally with `cargo install --path .` (or grab a tarball from [Releases](https://github.com/polarian/argus/releases)). Binaries are CLI-fetched, so **macOS doesn't quarantine them — no signing/notarization needed.**

> **Backend CLI** is installed separately — only the one you use: [`gh`](https://cli.github.com/) for GitHub, [`bkt`](https://github.com/avivsinai/bitbucket-cli) for Bitbucket. argus guides setup on first run.

**Updating:** a `⬆ vX.Y.Z available` banner appears when a newer release exists. Update with `cargo install-update -a`, `brew upgrade`, or by re-running the installer.

## Usage

```bash
arg [repo] [poll_secs] [--theme <name>] [--lang en|ko]
```

| Argument | Description |
|----------|-------------|
| `repo` | `owner/repo` (GitHub) or `workspace/repo_slug` (Bitbucket). **Omit to auto-detect from the git `origin` remote.** |
| `poll_secs` | Refresh interval in seconds (default `15`, min `2`). |
| `--theme`, `-t` | Color theme (also `ARGUS_THEME`). |
| `--lang` | UI language `en`/`ko` (also `ARGUS_LANG`; default: system locale). |

```bash
arg cli/cli          # watch cli/cli
arg cli/cli 5        # 5s interval
cd my-repo && arg    # auto-detect the current repo
```

## Key bindings

| Key | Action | Key | Action |
|-----|--------|-----|--------|
| `Tab` / `Shift+Tab` | move panel focus | `/` | search panel |
| `↑`/`↓` (`k`/`j`) | scroll | `Enter` / `o` | open in browser |
| `Shift`+arrows | move between panels | `v` / `→` | detail preview |
| `+` / `-` | polling interval | `r` | refresh now |
| `q` / `Esc` | quit | | |

In the detail modal: `↑`/`↓`·`PgUp`/`PgDn` scroll, `g`/`G` top/bottom, `l` toggle log view (runs), `o` browser, `←`/`Esc` close.

## Backends

Selected by the `backend` config key (or auto-detected from the git remote). Auth and networking are delegated entirely to the CLI.

| Backend | CLI | repo format | Auth |
|---------|-----|-------------|------|
| `github` (default) | `gh` | `owner/repo` | `gh auth login` |
| `bitbucket` | `bkt` | `workspace/repo_slug` | `bkt auth login https://bitbucket.org --kind cloud --web` |

**Usually you just run `arg`** — a startup pre-flight checks the CLI install, auth, and (for Bitbucket) the active context, fixing what it can: it offers to install/log in, and auto-creates the Bitbucket context.

Bitbucket notes:
- `bkt` needs **auth + an active context**. The context's host must be **`api.bitbucket.org`** (the pre-flight handles this automatically).
- Actions map to **Pipelines**; since Bitbucket Issues are deprecated, the Issues slot shows **active Branches** instead.
- Review status is aggregated from PR participants (no single reviewDecision).

## Configuration

Looks up `./argus.toml` → `$XDG_CONFIG_HOME/argus/config.toml` → `~/.argus.toml` (first found wins; CLI args take precedence). All keys optional:

```toml
repo = "cli/cli"            # default target (else inferred from git remote)
poll_secs = 10              # refresh interval (s)
limit = 30                  # items per panel (1–100)
theme = "catppuccin-mocha"  # default · nord · catppuccin-mocha · dracula · tokyo-night
backend = "github"          # github | bitbucket
lang = "en"                 # en | ko (default: system locale)
update_check = true         # startup release check
```

## License

[MIT](LICENSE) © Polarian
