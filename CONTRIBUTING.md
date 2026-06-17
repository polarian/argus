# Contributing

Thanks for your interest in argus! Bug reports, feature ideas, and PRs are welcome.

## Development

```bash
cargo build --release     # build (binary: ./target/release/arg)
cargo test                # unit tests (no network, no gh/bkt calls)
cargo run -- cli/cli       # run against a repo (needs gh auth)
cargo run -- --help
```

The TUI needs an interactive terminal; the render path is covered by `TestBackend`
smoke tests instead.

## Before opening a PR

CI runs formatting, lints, and tests. Please make sure these pass locally:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

## Conventions

- **Code comments are in English.** User-facing UI strings go through the i18n
  catalog (`src/i18n.rs`) with **both English and Korean** — never hardcode UI text.
  Low-level/developer errors stay English.
- **Commit messages in English.**
- **Verify any new `gh`/`bkt` JSON fields against real command output** before
  modeling them (e.g. `gh pr view N --json …`, `bkt api /repositories/…`). No guessing.
- Colors go through the `Theme` palette — no hardcoded colors.

A new user-facing feature usually touches the fetch layer (`github.rs`/`bitbucket.rs`),
state (`app.rs`), render (`ui.rs`), and tests together.
