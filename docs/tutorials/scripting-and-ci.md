# Tutorial: Scripting & CI

**Goal:** use the CLI as a building block — JSON output, `jq` recipes, and a working GitHub Actions deploy job.

## The three env vars that make CI work

The CLI needs zero setup files when these are set:

```bash
export QUOME_TOKEN=qk_...   # instead of `quome login`
export QUOME_ORG=0d9f...    # instead of `quome link`
export QUOME_APP=7c9e...    # instead of `quome link` (app-scoped commands)
```

Precedence is `flag → env → linked directory`, so env vars in CI never fight with a developer's local links. Full table: [Configuration](../configuration.md).

Use a dedicated key for CI — scoped and expiring:

```bash
quome keys create github-actions --scopes "write:app read:secret" --expires-days 90
```

## `--json` + `jq` recipes

Every command supports `--json` and prints raw API objects. Tables are for humans; this is for everything else.

```bash
# App ID by name
APP_ID=$(quome apps list --json | jq -r '.[] | select(.name=="my-api") | .id')

# Is the app running?
quome apps get --json | jq -e '.status == "running"' >/dev/null && echo up

# URL of every running app
quome apps list --json | jq -r '.[] | select(.status=="running") | .primary_url'

# Latest deployment status
quome deployments list --json | jq -r '.[0].status'

# Error lines from logs
quome logs --json | jq -r '.revisions[].logs[] | select(.severity=="ERROR") | .message'

# Audit actions by frequency
quome events -n 100 --json | jq -r '.[].action' | sort | uniq -c | sort -rn
```

## Wait for a deployment to finish

`deployments create` returns immediately. Poll until it settles:

```bash
#!/usr/bin/env bash
set -euo pipefail

DEPLOY_ID=$(quome deployments create --json | jq -r .id)
echo "Triggered deployment $DEPLOY_ID"

while true; do
  STATUS=$(quome deployments get "$DEPLOY_ID" --json | jq -r .status)
  case "$STATUS" in
    success)             echo "✓ deployed"; exit 0 ;;
    failed|cancelled)    echo "✗ deployment $STATUS"
                         quome deployments get "$DEPLOY_ID" --json | jq -r '.failure_reason // empty'
                         exit 1 ;;
    *)                   echo "  …$STATUS"; sleep 10 ;;
  esac
done
```

## GitHub Actions: deploy after tests pass

For git-sourced apps you may prefer Quome's own push-to-deploy ([Deploy from GitHub](deploy-from-github.md)). This job is for when you want CI to gate the deploy — tests first, then trigger:

```yaml
name: Deploy

on:
  push:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make test

  deploy:
    needs: test
    runs-on: ubuntu-latest
    env:
      QUOME_TOKEN: ${{ secrets.QUOME_TOKEN }}
      QUOME_ORG: ${{ vars.QUOME_ORG }}
      QUOME_APP: ${{ vars.QUOME_APP }}
    steps:
      - name: Install quome
        run: |
          curl -sL "https://github.com/quome-cloud/quome-cli/releases/latest/download/quome-darwin-arm64.tar.gz" -o quome.tar.gz
          # Linux runners: build from source until Linux release binaries ship
          # cargo install --git https://github.com/quome-cloud/quome-cli.git

      - name: Deploy and wait
        run: |
          DEPLOY_ID=$(quome deployments create --json | jq -r .id)
          for i in $(seq 1 60); do
            STATUS=$(quome deployments get "$DEPLOY_ID" --json | jq -r .status)
            [ "$STATUS" = "success" ] && exit 0
            { [ "$STATUS" = "failed" ] || [ "$STATUS" = "cancelled" ]; } && exit 1
            sleep 10
          done
          exit 1
```

> **Note:** release binaries are currently macOS-only (arm64 + x64). On Linux runners, install with `cargo install --git https://github.com/quome-cloud/quome-cli.git` (add a Rust toolchain step), or run a macOS runner.

Store `QUOME_TOKEN` as an encrypted **secret**; `QUOME_ORG`/`QUOME_APP` are fine as repository **variables** — they're UUIDs, not credentials.

## Debugging automation

- `QUOME_DEBUG=1` prints every raw API response to stderr — logs stay parseable because JSON output goes to stdout.
- Exit code is `0` on success, `1` on any error, so `set -e` and `&&` chains behave.
- Rate limited (`429`)? Back off and retry; see [Troubleshooting](../troubleshooting.md#api-errors).
