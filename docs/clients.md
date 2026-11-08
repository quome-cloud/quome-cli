# Agent clients

`quome connect` registers the `quome` MCP server in every agent client it finds
on this computer, and `quome skills sync` writes one thin skill per Quome
playbook into each of them. Each skill just tells the agent to call the
`playbook` tool with its name, so the steps always come from the server and are
never stale.

Supported clients: Claude Code, Codex, OpenCode, Antigravity.

```sh
quome connect                   # connect, then set up every installed client
quome connect --client codex    # limit to one client
quome skills sync               # refresh the skills (also runs when quome mcp starts)
```

A client that is not installed is skipped. If a client's setup fails, `quome
connect` prints the manual snippet below and still succeeds.

## What gets set up

| Client | MCP server | Skills | Invoke |
|---|---|---|---|
| Claude Code | `quome` in `~/.claude.json` (`claude mcp add -s user quome -- quome mcp --profile P`) | local plugin `~/.claude/skills/quome/` (`.claude-plugin/plugin.json`, `skills/<name>/SKILL.md`) | `/quome:add-sign-in`, or automatically from the description |
| Codex | `[mcp_servers.quome]` in `~/.codex/config.toml` (`codex mcp add quome -- quome mcp --profile P`) | `~/.agents/skills/quome-<name>/SKILL.md` | `$quome-add-sign-in`, or `/skills` |
| OpenCode | `quome` in `~/.config/opencode/opencode.json` (`opencode mcp add quome --global -- quome mcp --profile P`) | `~/.agents/skills/quome-<name>/SKILL.md` (shared with Codex) | `@quome-add-sign-in`, or the skills menu |
| Antigravity | `mcpServers.quome` in `~/.gemini/config/mcp_config.json` | `~/.gemini/config/skills/quome-<name>/SKILL.md` | `/quome-add-sign-in` |

Skills are also picked automatically when the agent decides a request matches
the description. `quome skills sync` only writes skills for playbooks the server
lists, and only removes skills it wrote itself.

## Manual setup

If automatic setup is not possible, add the server by hand. Replace `P` with
your profile name (`default` if you use one profile).

Claude Code:

```sh
claude mcp add -s user quome -- quome mcp --profile P
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.quome]
command = "quome"
args = ["mcp", "--profile", "P"]
```

OpenCode: prefer the CLI, which writes the right shape for your version:

```sh
opencode mcp add quome --global -- quome mcp --profile P
```

Antigravity (`~/.gemini/config/mcp_config.json`):

```json
{ "mcpServers": { "quome": { "command": "quome", "args": ["mcp", "--profile", "P"] } } }
```

Then run `quome skills sync`.

## Known note: OpenCode sees each playbook twice

OpenCode also reads `~/.claude/skills`, so with Claude Code installed it loads
each playbook twice: once from the Claude plugin (as `add-sign-in`) and once from
the shared skills (as `quome-add-sign-in`). Both call the same `playbook` tool,
so it is harmless.

## Checking a client

`scripts/smoke-clients.sh` runs each installed client's non-interactive mode and
checks it reaches the `playbook` path. `scripts/smoke-clients.sh --fixture` does
the same with a single fixture skill in a scratch home (your real configuration
is not touched), which proves skill discovery per client; the client still needs
its own model login. The eval suite for Claude Code lives in
[`evals/`](../evals/README.md).
