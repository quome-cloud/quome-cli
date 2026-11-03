# Connect: `connect`

> **Quome Cloud only.** `quome connect` links this computer to **Quome Cloud** (`api.quome.cloud`) so Claude Code can use it. It is separate from [`quome login`](session.md), which authenticates the CLI against **Quome Enterprise** with an org API key. `login` and every existing command are unchanged and do not use the connection described here.

## `quome connect`

Gives this computer its own agent key, asks you to approve it in the browser, and registers the Quome MCP server (see [`quome mcp`](mcp.md)) with Claude Code.

```
Connect this computer to Quome Cloud

Usage: quome connect [OPTIONS] [API_URL]

Arguments:
  [API_URL]  Quome Cloud API URL [default: https://api.quome.cloud]

Options:
      --name <NAME>        Name shown for this agent in the console (default: "Claude Code on <hostname>")
      --profile <PROFILE>  Profile to save the connection under
      --console <CONSOLE>  Console URL (default: the API URL with `api.` replaced by `console.`)
      --issuer <ISSUER>    Issuer URL (default: the API URL with `api.` replaced by `auth.`)
      --no-mcp             Do not register the MCP server with Claude Code
      --json               Output as JSON
  -h, --help               Print help
```

### What happens

1. **A key is made on this machine.** The CLI generates a fresh signing key. The private half never leaves this computer.
2. **The browser opens for approval.** The CLI prints a `Key fingerprint: SHA256:abcd…wxyz` line and opens the console's connect page. The page shows the same fingerprint; **compare the two before you approve**. If they differ, do not approve.
3. **You approve; the CLI finishes.** The console hands the grant back to a short-lived listener on `127.0.0.1`. The CLI checks the connection works, saves it, and registers the MCP server.

```console
$ quome connect
Key fingerprint: SHA256:q3Zk…Hn8w
Opening https://console.quome.cloud/connect?name=Claude%20Code%20on%20laptop&…
Waiting for you to approve in the browser...
Registered the quome MCP server with Claude Code (profile cloud).
Connected to Quome Cloud as Claude Code on laptop (org acme), expires 2027-02-01
```

If no browser can be opened, the CLI says so; open the printed URL yourself.

### The agent name

`--name` is what the console lists for this agent. Control and invisible formatting characters are removed, and the name is cut to **64 characters**. The default is `Claude Code on <hostname>`.

### Where state lives

| What | Where |
|------|-------|
| Connection details (agent, org, expiry, URLs) | `~/.quome/cloud.json` |
| The private key, macOS | macOS Keychain, service `cloud.quome.agent`, account = profile name |
| The private key, Linux | `~/.quome/cloud-<profile>.key`, mode `0600` |

`~/.quome/config.json` (the Enterprise login) is not touched. Windows is not supported; `quome connect` exits with code 2 there.

### Claude Code registration

When `claude` is on your `PATH`, `quome connect` runs the equivalent of:

```bash
claude mcp add --scope user quome -- quome mcp --profile cloud
```

An existing user-scope `quome` server is replaced, so reconnecting updates it. If `claude` is not installed, or the command fails, the CLI prints the config to paste into your MCP client instead:

```json
{"mcpServers":{"quome":{"command":"quome","args":["mcp","--profile","cloud"]}}}
```

Pass `--no-mcp` to skip registration entirely.

### Profiles and reconnecting

Run `quome connect` again at any time to reconnect, for example after the connection expires or you revoke it. The new connection replaces the saved one; the old one stays valid until it expires or you revoke it on the console dashboard, and the CLI reminds you of that. `--profile <name>` saves a second connection under another name; without it the new connection becomes the default.

`--json` prints a result object (`connected`, `profile`, `agent_name`, `org_id`, `org_slug`, `expires_at`, `mcp`) on stdout and sends progress lines to stderr.

## Checking and removing the connection

See [Cloud](cloud.md) for `quome cloud status` and `quome cloud disconnect`.
