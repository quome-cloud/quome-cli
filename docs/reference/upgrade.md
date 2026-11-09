# Upgrade: `upgrade`

Self-update via Homebrew. Checks the tap for a newer version, shows what would change, and asks before upgrading.

```
Usage: quome upgrade
```

```console
$ quome upgrade
  Current version: 0.2.0
  Latest version:  0.2.1

? Upgrade from 0.2.0 to 0.2.1? Yes
✓ Upgraded to 0.2.1
```

Requires the CLI to have been installed with Homebrew (`brew tap quome-cloud/quome && brew install quome`). If you installed via Cargo, upgrade with:

```bash
cargo install --git https://github.com/quome-cloud/quome-cli.git --force
```

> If Homebrew refuses with an "untrusted tap" error, run `brew trust quome-cloud/quome` once — see [Troubleshooting](../troubleshooting.md#homebrew).

## Automatic updates

A Homebrew install keeps itself up to date, so you rarely need `quome upgrade`.

- At most once a day, in the background, `quome` compares itself with the latest GitHub release. This never slows a command down, and `quome mcp` counts as a command, so agent-only users stay current too.
- A release installs itself once it has been out for 6 hours.
- The update refreshes only the Quome tap and upgrades only `quome`. Nothing else in Homebrew changes. A pinned formula stays pinned.
- The new version applies the next time `quome` starts, and you see one line: `Updated quome to 0.2.32`.
- If an update fails, nothing is shown and the next day's check tries again.
- Installs that didn't come from Homebrew are never replaced. They see one line saying a new version is available.

Turn it off with `QUOME_NO_AUTO_UPDATE=1`. It never runs when `CI` is set.
