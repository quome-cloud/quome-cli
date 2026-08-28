# Host

Run a Quome sandbox host on your own computer (or a dedicated Linux box) and
join it to your organization's sandbox fleet. Sandboxes you choose to "Run on"
this device execute here, on your hardware, while everything else about them
(the terminal, the agent, secrets, git) works exactly as it does in the cloud.

`quome host` is a thin wrapper around `quome-host`, the per-environment host
tool the control plane publishes through its signed distribution channel. The
first `quome host …` downloads it into `~/.quome/bin`, verifies its sha256
against the published `SHA256SUMS`, and has the downloaded binary verify that
manifest's Ed25519 signature against the control plane's public key. From
then on `quome-host` verifies its own updates against the key compiled into
it. No API key is needed or sent — the enrollment code is the credential —
and nothing needs root (the `--native` Linux path is the exception, by design).

## Prerequisites

- macOS 13+ on Apple silicon or Intel, with [Lima](https://lima-vm.io) (`brew install lima`) — the host runs in a lightweight VM.
- Or Linux (arm64/amd64) with `--native`: provisions *this* machine as the host. Root, no isolation from the rest of the box — meant for a dedicated device, not a laptop.
- Your organization must have **on-device hosts** enabled (Settings → Security → Sandbox → On-device hosts) and you need an enrollment code from **Sandboxes → Fleet → Add device**. Codes are shown once and expire.

## `quome host up`

```bash
quome host up --enroll qh_acme_…       # first run: install, create the VM, enroll
quome host up                          # later: just start the VM again
quome host up --native --enroll qh_…   # Linux, this machine, run as root
```

| Flag | Description |
|------|-------------|
| `--enroll <CODE>` | Enrollment code from the dashboard. Joins this host to the org that minted it. |
| `--native` | Provision this machine instead of a VM (Linux, root). |
| `--refresh` | Re-download and re-verify `quome-host` first. |
| `--cpus N`, `--memory GiB`, `--disk GiB`, `--vm-type vz\|qemu`, `--dry-run` | Passed through to `quome-host up`. |

## `quome host enroll <CODE>`

Redeem an enrollment code on a host that is already running. `--native` for a Linux native host.

## `quome host status`

VM state plus the local agent's own health answer.

## `quome host logs [-f]`

The host agent's journal; `-f` follows it.

## `quome host update`

Re-provision the host, which reinstalls the verified current agent.

## `quome host down`

Stop the VM. Sandboxes running on it stop with it.

## `quome host login`

Authorize this CLI for the device you enrolled. Opens your browser to the
platform's approval page (your normal sign-in plus MFA step-up); the session
it mints is a **CLI session** — scoped to sandbox actions on *this* device
only, nothing else — and lives in `~/.quome/cli/session.json` (0600) for
7 days of inactivity.

| Flag | Description |
|------|-------------|
| `--web-url <URL>` | Override the approval-page origin (default: what the control plane advertises). |

## `quome host shell [SANDBOX] [--session NAME]`

Attach this terminal to a sandbox running on this device — the same tmux
session the browser terminal shows, over loopback, so both views share one
screen. With exactly one sandbox running the name is optional; otherwise pass
its name or an id prefix (an ambiguous call prints the candidates and exits
`1`). `--session` picks a tmux session (default `quome`, the main browser
terminal; Power Mode terminals are named after their agent, e.g. `claude`).
Detach with `Ctrl-b d`.

Exit codes: `0` detached · `1` no/ambiguous sandbox · `2` not logged in ·
`3` the org has direct terminals off · `4` could not connect.

## `quome host logout`

Revoke the CLI session server-side and remove the local file.

## `quome host install [--refresh]`

Download and verify `quome-host` without running anything — useful to pre-stage a machine or to check what the control plane publishes.

## Environment

| Variable | Effect |
|----------|--------|
| `QUOME_API_URL` | Which control plane to install from and enroll with (default: your settings' `api_url`). |
| `QUOME_HOST_INSTALL_DIR` | Where `quome-host` is installed (default `~/.quome/bin`). |
| `QUOME_HOST_VERSION` | Artifact version to download (default `latest`). |

## Exit codes

Mirrors `quome-host`: `0` success, `1` the requested action failed, `2` this machine is missing something (Lima, root).

## Without Homebrew

The same install, as a one-liner served by your control plane:

```bash
curl -fsSL https://quome.studio/api/v1/downloads/sandbox-host/host.sh | sh -s -- --enroll <CODE>
```
