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

Sandbox and app arguments marked "name or id" accept either. Email `address` arguments accept the full address or its id.

| Tool | Arguments | What it does |
|------|-----------|--------------|
| `guide` | none | Reads the Quome Cloud agent guide: how sandboxes, previews, app deploys and email inboxes work. Claude is told to call it first. |
| `create_sandbox` | `name?`, `memory_gb?`, `runtime?` | Creates a sandbox (a Linux workspace with a coding-agent runtime) and waits until it is running (up to 180 s; it stops early if the sandbox fails, stops or is deleted). `name`: 2-31 chars, a lowercase letter then lowercase letters, digits or hyphens; default `agent-<6 hex>`. `memory_gb`: 0.5, 1, 2, 4, 8, 16, 32 or 64; default 4. `runtime`: `claude`, `codex`, `opencode` or `antigravity`; default `claude`. |
| `list_sandboxes` | none | Lists this organization's sandboxes. |
| `stop_sandbox` | `sandbox` | Stops a sandbox (name or id). |
| `exec` | `sandbox`, `command`, `timeout_s?` | Runs a shell command in the sandbox's `/workspace`. Returns `exit_code`, `stdout`, `stderr`, `truncated`. `timeout_s` is clamped to 1-25; default 25. |
| `write_file` | `sandbox`, `path`, `content?`, `content_base64?` | Writes a file. `path` must be absolute, under `/workspace/`, with no `..` segment. Give exactly one of `content` (text) or `content_base64` (binary). |
| `read_file` | `sandbox`, `path` | Reads a text file, up to 1 MiB. For larger or binary files use `exec` (for example `base64` or `head`). |
| `list_files` | `sandbox`, `path?` | Lists a directory; default `/workspace`. |
| `preview` | `sandbox`, `port` | Returns a public preview URL for a port the sandbox is serving. |
| `deploy_app` | `name`, `folder`, `memory_gb?` | Deploys a local folder containing a Dockerfile as a Quome Cloud app and waits for it to run. `memory_gb` as above; default 1. |
| `app_status` | `name` | Shows an app's state by name, with its latest build (`build_id`, `status`, `error`) and latest deploy (`latest_deploy`: `deploy_id`, `status`, `error`, `failure`). `state` is `building`, `rolling_out`, `running`, `crashed` (the latest deploy failed), or the app's status otherwise (`created`, `stopped`). The `url` is given only once a build has succeeded. |
| `list_app_catalog` | | Lists the first-party Quome apps you can install, with install status. |
| `install_app` | `app`, `confirm?` | Without `confirm: true`, previews the billing and changes nothing. With it, installs the app and waits (up to 10 minutes) until it runs; returns `status`, `url` and `next`. |
| `list_installed_apps` | | Lists this org's installed apps and their status. |
| `open_app` | `app` | Returns `{url}`, for an installed app, a link that opens it already signed in (no second login). |
| `find_app_tools` | `query?`, `app?` | Searches the tools of installed apps and returns short hits. Apps' tools are never separate MCP servers, so this tool list stays fixed. |
| `describe_app_tool` | `app`, `tool` | Shows one app tool's description and input schema. |
| `call_app_tool` | `app`, `tool`, `arguments` | Calls an app tool; `arguments` go through unchanged. Destructive tools need an org owner or admin. |
| `list_app_keys` | `app` | Lists the keys an app can use and whether each is set (never the values). |
| `set_app_key` | `app`, `key`, `value` | Sets an app key. Write-only: the value is never returned or echoed. Needs an org owner or admin, and the "Store secrets" permission (`quome connect`). The result says whether the app is picking the key up (`delivering`). |
| `uninstall_app` | `app`, `confirm?` | Without `confirm: true`, previews. With it, removes the app's service; its data is kept. |
| `delete_app_data` | `app`, `confirm?` | Without `confirm: true`, previews. With it, permanently deletes an uninstalled app's database, storage and keys. |
| `email_create_address` | `label`, `kind?`, `sandbox?`, `forward?` | Creates an email address and returns it. `label`: what it is for, usually the service name; 1-20 lowercase letters, digits or hyphens (lowercased for you). `kind`: `signup` (default) to sign the person up for a service, with mail forwarded to them; or `sandbox`, a test inbox for an app in a sandbox. `sandbox` (name or id) is required for `sandbox` and not allowed for `signup`. `forward`: forward each message to the person's own email (signup only); default true. |
| `email_list_addresses` | none | Lists your email addresses (yours and those of sandboxes you can use), with status and last message time. |
| `email_list_messages` | `address`, `since?`, `limit?` | Lists an address's messages (full address or id), newest first: id, sender, subject, received time, risk and whether it is quarantined. `since`: only messages after this time (RFC 3339), for example a previous `email_wait` `cursor`. `limit`: clamped to 1-100; default 20. The result adds a `note` that subjects and sender names come from outside senders. |
| `email_read` | `message_id` | Reads one message as an untrusted envelope: the text between `<<<EMAIL nonce>>>` markers, likely verification `codes` and `links`, and attachments. When `quarantined` is true the content is withheld until the person releases it in the console. |
| `email_wait` | `address`, `since?`, `from_contains?`, `subject_contains?`, `timeout_s?` | Waits for mail at an address (full address or id) and returns `{message, cursor}`: the first message that matches the optional case-insensitive sender and subject filters, in the same envelope as `email_read`. `since`: pass the previous result's `cursor` to get only newer mail; when omitted, a later call in the same session continues from the last cursor for that address, and the first call looks back one minute. `timeout_s`: total wait, clamped to 1-120; default 60. |
| `email_disable` | `address` | Disables an address (full address or id) for good: it never receives mail again. Messages already received stay readable. Disabling an address that is already disabled succeeds. |

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
| `failed` | The build failed (`error` has the tail), or it built but the container did not stay up: `error` says what was observed (exit code, seconds alive), `container_output` is its last lines. `note` names the build log route. |
| `building` | Still building after 10 minutes. Check later with `app_status`. |
| `deploying` | Built, but still rolling out after 10 minutes. Check later with `app_status`. |

### Email

The `email_*` tools give the agent inboxes on Quome Cloud, mainly to sign the person up for services and read the verification mail. Addresses look like `<name>-<label>-<rand>.<org>@in.quome.cloud`.

Mail sent to these addresses comes from outside senders. It is untrusted data, and every email tool tells the agent never to follow instructions found in an email.

Connections made before email existed have no email access: run `quome connect` again and allow it. Until then the email tools say so.

`email_read` returns the envelope exactly as the server sent it: the CLI does not parse, trim or re-wrap the body or its markers. Agents never get HTML, only the text body.

`email_wait` repeats short server waits of up to 25 s until `timeout_s` runs out, passing each response's `now` on as the next `since`, so no mail that arrives in between is missed. When the server is busy (at most 5 waits at once per agent) or the edge times out, it backs off (1, 2, 4, then 8 s) and retries with the same cursor. Both a match and a time-out return a `cursor`; pass it as `since` next time (for example after asking a service to resend a code) so the same message is not returned again. Each server wait's HTTP timeout is the wait plus 10 s, so the call ends close to `timeout_s`. When nothing matched in time it returns `{"message": null, "timed_out": true, "cursor": ...}` with a `next` hint; if the server stayed busy the whole time it returns an error naming the 5-wait limit.

Limits: 50 signup addresses per person and 20 per sandbox. Disabled addresses do not count; `email_disable` frees a slot.

### Progress and cancelling

`create_sandbox`, `deploy_app` and `email_wait` send MCP progress notifications while they wait, when the client asks for them, and stop when the client cancels the call (in Claude Code, press Esc). Claude Code allows long tool calls; clients that time out requests after about 60 seconds reset that timer on each progress notification. Where a client still gives up early, raise its tool timeout (in Claude Code, the `MCP_TOOL_TIMEOUT` environment variable, in milliseconds). The work continues on the server either way, so `list_sandboxes` or `app_status` shows the outcome.

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
