# Git credential: `git-credential`

> **Quome Cloud only.** This command signs plain `git` in to the org's Git repos using the connection made by [`quome connect`](connect.md). It does not use your Quome Enterprise login ([`quome login`](session.md)).

## `quome git-credential`

A [git credential helper](https://git-scm.com/docs/gitcredentials). You do not run it yourself: git runs it whenever it needs a username and password for a repo, and it answers with a short-lived token for this connection's org. With it, `git clone`, `git fetch` and `git push` work on the org's repos without a password or a stored token.

```
Git credential helper for the org's Git repos (Quome Cloud)

Usage: quome git-credential [OPTIONS] <ACTION>

Arguments:
  <ACTION>  get, store or erase (git passes this)

Options:
      --profile <PROFILE>  Profile to use instead of matching the host (default: match by host)
  -h, --help               Print help
```

## Setup

Run these two lines once. The [`git_clone_url`](mcp.md#git) tool prints the helper line without `--add`; run the empty line first and then the `--add` form shown here, because the plain form would replace the empty entry.

```bash
git config --global credential.https://git.quome.cloud.helper ''
git config --global --add credential.https://git.quome.cloud.helper '!quome git-credential'
```

The first line is empty on purpose. It clears any helper git would otherwise use for that host, such as the macOS keychain or a credential manager. Those helpers would keep the token, but it expires after 5 minutes, so the next `git push` would fail with a stale password. With only this helper left, git asks for a fresh token each time.

If one computer is connected to several orgs (several profiles), also let git send the repo path, so the helper can tell which org's token to give:

```bash
git config --global credential.https://git.quome.cloud.useHttpPath true
```

Without it, git sends only the host. When several connections serve the host and git sends no path, the helper uses the default profile only if it is one of them; otherwise it gives no credential and names this `useHttpPath` setting on stderr.

Then use git as usual:

```bash
git clone https://git.quome.cloud/acme/api.git
```

## What it answers

For `get`, git sends the repo it wants on stdin and the helper prints the credential:

```console
$ printf 'protocol=https\nhost=git.quome.cloud\npath=acme/api.git\n\n' | quome git-credential get
username=quome
password=<token>
password_expiry_utc=1767225900
```

`password` is a token that works for 5 minutes. `password_expiry_utc` is when it stops working, in Unix seconds; recent versions of git use it to ask again instead of reusing an expired token.

The helper answers only for `https` URLs whose host and org match one of your connections. For anything else it prints nothing and exits successfully, so git moves on to its next helper. `--profile <name>` only narrows which connection is considered; the host, `https` and the org in the path must still match.

`store` and `erase` do nothing and print nothing: there is no password to remember or forget, because each `get` asks for a new token.

## Output and errors

There is no `--json` flag: git reads the output, so it is always the three `key=value` lines above.

Errors, such as a connection that was revoked or a computer that was never connected, go to stderr and the command exits with code 1. Git then falls back to asking for a password. Run `quome connect` again, then retry.
