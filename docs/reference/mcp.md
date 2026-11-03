# MCP: `mcp`

> **Quome Cloud only.** `quome mcp` serves Quome Cloud tools to an MCP client such as Claude Code, using the connection made by [`quome connect`](connect.md). It does not use your Quome Enterprise login ([`quome login`](session.md)).

## `quome mcp`

Runs an MCP server over stdio. You normally do not run it yourself: `quome connect` registers it with Claude Code, which starts it on demand.

```
Run the Quome Cloud MCP server (stdio) for Claude Code

Usage: quome mcp [OPTIONS]

Options:
      --profile <PROFILE>  Profile to use (default: the default profile)
  -h, --help               Print help
```

## Tools

Sandbox and app arguments marked "name or id" accept either.

| Tool | Arguments | What it does |
|------|-----------|--------------|
| `guide` | none | Reads the Quome Cloud agent guide: how sandboxes, previews and app deploys work. Claude is told to call it first. |
| `create_sandbox` | `name?`, `memory_gb?`, `runtime?` | Creates a sandbox (a Linux workspace with a coding-agent runtime) and waits until it is running (up to 180 s). `name`: 2-31 chars, a lowercase letter then lowercase letters, digits or hyphens; default `agent-<6 hex>`. `memory_gb`: 0.5, 1, 2, 4, 8, 16, 32 or 64; default 4. `runtime`: `claude`, `codex`, `opencode` or `antigravity`; default `claude`. |
| `list_sandboxes` | none | Lists this organization's sandboxes. |
| `stop_sandbox` | `sandbox` | Stops a sandbox (name or id). |
| `exec` | `sandbox`, `command`, `timeout_s?` | Runs a shell command in the sandbox's `/workspace`. Returns `exit_code`, `stdout`, `stderr`, `truncated`. `timeout_s` is clamped to 1-25; default 25. |
| `write_file` | `sandbox`, `path`, `content?`, `content_base64?` | Writes a file. `path` must be absolute, under `/workspace/`, with no `..` segment. Give exactly one of `content` (text) or `content_base64` (binary). |
| `read_file` | `sandbox`, `path` | Reads a text file, up to 1 MiB. For larger or binary files use `exec` (for example `base64` or `head`). |
| `list_files` | `sandbox`, `path?` | Lists a directory; default `/workspace`. |
| `preview` | `sandbox`, `port` | Returns a public preview URL for a port the sandbox is serving. |
| `deploy_app` | `name`, `folder`, `memory_gb?` | Deploys a local folder containing a Dockerfile as a Quome Cloud app. `memory_gb` as above; default 1. |
| `app_status` | `name` | Shows an app's state and URL by name. |

### Long-running commands

`exec` returns after at most **25 seconds**. For anything longer (installs, builds, servers), start the job in the background and poll:

```bash
nohup npm run dev > /tmp/dev.log 2>&1 &     # start, returns immediately
tail -n 20 /tmp/dev.log                      # a later exec call: check on it
```

### Previews

`preview` exposes a port the sandbox is listening on and returns its public URL. If the port is already exposed, the existing URL is returned instead of creating a new one. Start the server first (in the background), then ask for the preview.

### Deploying

`deploy_app` packs the folder on this computer, creates the app if it does not exist, uploads the source, builds it, and waits up to **10 minutes**. On success it returns the app URL. If the build fails it returns the build error. If the build is still running after 10 minutes it returns `status: "building"` with a note; check later with `app_status`.

## Messages

When the connection is no longer valid (revoked on the dashboard, or expired), every tool returns:

```
This agent was disconnected from Quome Cloud — run `quome connect` again
```

If you just reconnected and still see it, a wrong system clock is the usual cause, and the message adds `If you just reconnected, check this computer's clock is correct.`

If you start the flow but have not yet approved in the browser, tools return:

```
Finish approving in the browser, then try again
```

If this computer was never connected, `quome mcp` cannot authenticate; run [`quome connect`](connect.md) first.
