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
