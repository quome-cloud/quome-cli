# Cloud: `cloud status`, `cloud disconnect`

> **Quome Cloud only.** These commands inspect the connection made by [`quome connect`](connect.md). They do not read or change your Quome Enterprise login ([`quome login`](session.md)).

## `quome cloud status`

Shows this computer's connection and checks, live, that it still works.

```
Show this computer's Quome Cloud connection and check it is still valid

Usage: quome cloud status [OPTIONS]

Options:
      --profile <PROFILE>  Profile to show (default: the default profile)
      --json               Output as JSON
  -h, --help               Print help
```

<!-- e2e: replace with real output -->
```console
$ quome cloud status
Quome Cloud
  Profile: cloud
  Agent:   Claude Code on laptop (agent-3f9c…)
  Org:     acme
  Expires: 2027-02-01
  Check:   ok
```

Not connected (real output, exit code 1):

```console
$ quome cloud status
Not connected to Quome Cloud — run `quome connect`
```

Exit code `0` when the check passes. It exits `1` when you are not connected ("Not connected to Quome Cloud — run `quome connect`") or the check fails (for example `Check:   failed — This agent was disconnected from Quome Cloud …`). With `--json` the output has `connected`, `profile`, `agent_name`, `agent_principal`, `org_id`, `org_slug`, `expires_at`, `api_url` and `error`.

## `quome cloud disconnect`

Forgets the connection on this computer: removes the profile from `~/.quome/cloud.json` and deletes its key from the Keychain (macOS) or `~/.quome/cloud-<profile>.key` (Linux).

```
Forget the local Quome Cloud connection

Usage: quome cloud disconnect [OPTIONS]

Options:
      --profile <PROFILE>  Profile to remove (default: the default profile)
  -h, --help               Print help
```

```console
$ quome cloud disconnect
Disconnected profile "cloud" from this computer.
To revoke this agent's access in Quome Cloud, open https://console.quome.cloud/dashboard
To remove it from Claude Code too: claude mcp remove --scope user quome
```

<!-- e2e: replace with real output -->

This is local only: it does **not** revoke the agent. Revoke it on the console dashboard so it can no longer be used from anywhere. It also does not unregister the MCP server from Claude Code; the command printed last does that.
