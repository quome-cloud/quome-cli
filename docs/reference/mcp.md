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
| `create_sandbox` | `name?`, `memory_gb?`, `runtime?` | Creates a sandbox (a Linux workspace with a coding-agent runtime) and waits until it is running (up to 180 s; it stops early if the sandbox fails, stops or is deleted). `name`: 2-31 chars, a lowercase letter then lowercase letters, digits or hyphens; default `agent-<6 hex>`. `memory_gb`: 0.5, 1, 2, 4, 8, 16, 32 or 64; default 4. `runtime`: `claude`, `codex`, `opencode` or `antigravity`; default `claude`. |
| `list_sandboxes` | none | Lists this organization's sandboxes. |
| `stop_sandbox` | `sandbox` | Stops a sandbox (name or id). |
| `exec` | `sandbox`, `command`, `timeout_s?` | Runs a shell command in the sandbox's `/workspace`. Returns `exit_code`, `stdout`, `stderr`, `truncated`. `timeout_s` is clamped to 1-25; default 25. |
| `write_file` | `sandbox`, `path`, `content?`, `content_base64?` | Writes a file. `path` must be absolute, under `/workspace/`, with no `..` segment. Give exactly one of `content` (text) or `content_base64` (binary). |
| `read_file` | `sandbox`, `path` | Reads a text file, up to 1 MiB. For larger or binary files use `exec` (for example `base64` or `head`). |
| `list_files` | `sandbox`, `path?` | Lists a directory; default `/workspace`. |
| `preview` | `sandbox`, `port` | Returns a public preview URL for a port the sandbox is serving. |
| `deploy_app` | `name`, `folder`, `memory_gb?` | Deploys a local folder containing a Dockerfile as a Quome Cloud app and waits for it to run. `memory_gb` as above; default 1. |
| `app_status` | `name` | Shows an app's state by name, with its latest build (`build_id`, `status`, `error`). The `url` is given only once a build has succeeded. |
| `list_app_catalog` | | Lists the first-party Quome apps you can install, with install status. |
| `install_app` | `app`, `confirm?` | Without `confirm: true`, previews the billing and changes nothing. With it, installs the app and waits (up to 10 minutes) until it runs; returns `status`, `url` and `next`. |
| `list_installed_apps` | | Lists this org's installed apps and their status. |
| `open_app` | `app` | Returns `{url}`, a link that opens the app already signed in (no second login). |
| `find_app_tools` | `query?`, `app?` | Searches the tools of installed apps and returns short hits. Apps' tools are never separate MCP servers, so this tool list stays fixed. |
| `describe_app_tool` | `app`, `tool` | Shows one app tool's description and input schema. |
| `call_app_tool` | `app`, `tool`, `arguments` | Calls an app tool; `arguments` go through unchanged. Destructive tools need an org owner or admin. |
| `list_app_keys` | `app` | Lists the keys an app can use and whether each is set (never the values). |
| `set_app_key` | `app`, `key`, `value` | Sets an app key. Write-only: the value is never returned or echoed. Needs the "Store secrets" permission (`quome connect`). |
| `uninstall_app` | `app`, `confirm?` | Without `confirm: true`, previews. With it, removes the app's service; its data is kept. |
| `delete_app_data` | `app`, `confirm?` | Without `confirm: true`, previews. With it, permanently deletes an uninstalled app's database, storage and keys. |

### Long-running commands

`exec` returns after at most **25 seconds**. For anything longer (installs, builds, servers), start the job in the background and poll:

```bash
nohup npm run dev > /tmp/dev.log 2>&1 &     # start, returns immediately
tail -n 20 /tmp/dev.log                      # a later exec call: check on it
```

### Previews

`preview` exposes a port the sandbox is listening on and returns its public URL. If the port is already exposed, the existing URL is returned instead of creating a new one. Start the server first (in the background), then ask for the preview.

### Deploying

`deploy_app` packs the folder on this computer (it must contain a `Dockerfile`; `.dockerignore` is honoured), creates the app if it does not exist (a deleted app of the same name is ignored), uploads the source and builds it. After the build succeeds it keeps waiting until the new revision is running. The whole wait is at most **10 minutes**. The result's `status` is one of:

| `status` | Meaning |
|----------|---------|
| `running` | Built and live. `url` is the app URL. |
| `failed` | The build failed (`error` has the tail), or it built but the new revision did not start (`service_status`). `note` names the build log route; check that the container listens on `$PORT`. |
| `building` | Still building after 10 minutes. Check later with `app_status`. |
| `deploying` | Built, but still rolling out after 10 minutes. Check later with `app_status`. |

### Progress and cancelling

`create_sandbox` and `deploy_app` send MCP progress notifications while they wait, when the client asks for them, and stop when the client cancels the call (in Claude Code, press Esc). Claude Code allows long tool calls; clients that time out requests after about 60 seconds reset that timer on each progress notification. Where a client still gives up early, raise its tool timeout (in Claude Code, the `MCP_TOOL_TIMEOUT` environment variable, in milliseconds). The work continues on the server either way, so `list_sandboxes` or `app_status` shows the outcome.

## Messages

When the connection is no longer valid (revoked on the dashboard, or expired), every tool returns:

```
This agent was disconnected from Quome Cloud — run `quome connect` again
```

Run `quome connect`; the next tool call reloads the saved connection and carries on, with no restart. Only if the saved connection has not changed does the message continue with `, then restart the MCP server (or Claude Code) if this keeps happening`.

If you just reconnected and still see it, a wrong system clock is the usual cause, and the message adds `If you just reconnected, check this computer's clock is correct.`

If you start the flow but have not yet approved in the browser, tools return:

```
Finish approving in the browser, then try again.
```

If this computer was never connected, `quome mcp` cannot authenticate; run [`quome connect`](connect.md) first.
