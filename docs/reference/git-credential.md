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

Run these three lines once. The [`git_clone_url`](mcp.md#git) tool prints the helper line without `--add`; run the empty line first and then the `--add` form shown here, because the plain form would replace the empty entry.

```bash
git config --global credential.https://git.quome.cloud.helper ''
git config --global --add credential.https://git.quome.cloud.helper '!quome git-credential'
git config --global credential.https://git.quome.cloud.useHttpPath true
```

The first line is empty on purpose. It clears any helper git would otherwise use for that host, such as the macOS keychain or a credential manager. Those helpers would keep the token, but it expires after 5 minutes, so the next `git push` would fail with a stale password. With only this helper left, git asks for a fresh token each time.

The third line lets git send the repo path along with the host. Every org on one platform shares the same git host, so the path (it starts with the org) is what tells the helper which org's token to give. It is needed as soon as one computer is connected to several orgs (several profiles), and harmless with one.

Without it, git sends only the host. When several connections serve the host and git sends no path, the helper uses the default profile only if it is one of them; otherwise it gives no credential and names this `useHttpPath` setting on stderr.

Then use git as usual:

```bash
git clone https://git.quome.cloud/acme/api.git
```

### Repos outside the org (the egress host)

The helper also answers for the org's egress host, `egress.<apps domain>`, which reaches repos on other Git hosts through the org's Credentials. Set it up the same way, with the egress host in place of the git host:

```bash
git config --global credential.https://egress.quome.cloud.helper ''
git config --global --add credential.https://egress.quome.cloud.helper '!quome git-credential'
git config --global credential.https://egress.quome.cloud.useHttpPath true
```

Egress URLs start with the org, then `git`, the host, and the repo: `https://egress.quome.cloud/acme/git/github.com/octo/app.git`. As on the git host, the first path segment (the org) is how the helper picks the connection, so `useHttpPath` is needed once several connections serve the host. Here the helper asks for Credentials access (`credentials_use`) instead of Git access. If the connection lacks it, nothing is printed on stdout and stderr says: Run `quome connect` again and allow Credentials access.

## What it answers

For `get`, git sends the repo it wants on stdin and the helper prints the credential:

```console
$ printf 'protocol=https\nhost=git.quome.cloud\npath=acme/api.git\n\n' | quome git-credential get
username=quome
password=<token>
password_expiry_utc=1767225900
```

`password` is a token that works for 5 minutes. `password_expiry_utc` is when it stops working, in Unix seconds; recent versions of git use it to ask again instead of reusing an expired token.

The helper answers only for `https` URLs whose host is exactly `git.<apps domain>` or `egress.<apps domain>` of one of your connections and whose org matches. Look-alike hosts get nothing. For anything else it prints nothing and exits successfully, so git moves on to its next helper. `--profile <name>` only narrows which connection is considered; the host, `https` and the org in the path must still match.

`store` and `erase` do nothing and print nothing: there is no password to remember or forget, because each `get` asks for a new token.

## Output and errors

There is no `--json` flag: git reads the output, so it is always the three `key=value` lines above.

Errors, such as a connection that was revoked or a computer that was never connected, go to stderr and the command exits with code 1. Git then falls back to asking for a password. Run `quome connect` again, then retry.
