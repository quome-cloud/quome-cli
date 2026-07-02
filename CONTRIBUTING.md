# Contributing

Thanks for helping make the Quome CLI better. The codebase is small and approachable — a typical command is ~100 lines.

## Setup

```bash
git clone https://github.com/quome-cloud/quome-cli.git
cd quome-cli
./scripts/setup.sh    # installs the pre-commit hook (fmt + clippy)
cargo build
```

Rust 1.70+ required.

## Repo tour

```
src/
  main.rs          # clap command tree
  client.rs        # HTTP client: X-API-Key auth, error mapping
  config.rs        # ~/.quome/config.json (token, per-directory links)
  settings.rs      # api_url resolution (env → local → global → default)
  errors.rs        # QuomeError
  ui.rs            # spinners, tables, panels
  api/             # one file per API domain; models.rs has all types
  commands/        # one file per command group
```

Adding a command generally means: a method in `src/api/<domain>.rs`, request/response types in `src/api/models.rs`, a subcommand in `src/commands/<group>.rs`, and wiring in `main.rs`.

## Before you push

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

CI runs exactly these on every PR — green locally means green in CI.

## Docs

If you add or change a command, update its page under `docs/reference/` (and the table in `README.md` if it's a new group). Docs use real `--help` output and realistic example output — run the command and paste, don't invent.

## Releases

Automatic. Every push to `main` bumps the patch version, tags, builds macOS binaries, publishes a GitHub release, and updates the Homebrew formula. For a minor/major bump, run the **Release** workflow manually with the `version_bump` input.

## Conventions

- Commit messages: conventional-commit style (`feat:`, `fix:`, `docs:`, `chore:`) — they become release notes
- Errors: add variants to `QuomeError` rather than stringly-typed errors where practical
- Every new command supports `--json` and `--org`/`--app` overrides
