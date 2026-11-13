# Models: `models`

> **Quome Cloud only.** `quome models` lets this computer serve its local models (Ollama, vLLM, llama.cpp, MLX or Quexo) to your organization through the platform's gateway. macOS only in packaged releases; Linux via `cargo install`.

**Trust boundary.** Whoever controls this machine can read the prompts sent to it.

```
quome models connect   # pair this computer, approve in the browser
quome models serve --install   # keep serving in the background, starting at login
quome models status
quome models disconnect
```

## `quome models`

```
Serve this computer's local models to your organization

Usage: quome models <COMMAND>

Commands:
  connect     Pair this computer with an organization to serve its local models
  status      Show this machine's connection, settings and what the platform sees
  disconnect  Revoke this machine and forget its connection
  serve       Serve this machine's models to the organization
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

## `quome models connect`

Pairs this computer with an organization. It opens the platform's approval page in your browser; an organization admin approves it. The approval grants this machine one capability, `models_serve`, and nothing else. A first connection never becomes the default profile of [`quome connect`](connect.md); it is stored as its own profile, `models`.

```
Pair this computer with an organization to serve its local models

Usage: quome models connect [OPTIONS] [API_URL]

Arguments:
  [API_URL]  Quome Cloud API URL [default: https://api.quome.cloud]

Options:
      --org <ORG>
          Organization (slug) this machine serves
      --name <NAME>
          Name shown for this machine (default: this computer's short hostname)
      --server <SERVER>
          Local server: ollama, vllm, llama.cpp, mlx or quexo
      --url <URL>
          URL of the local server (plain http only to this computer)
      --max-concurrency <MAX_CONCURRENCY>
          Most requests served at once (1 to 64)
      --console <CONSOLE>
          Console URL (default: the API URL with `api.` replaced by `console.`)
      --issuer <ISSUER>
          Issuer URL (default: the API URL with `api.` replaced by `auth.`)
      --replace
          Disconnect an existing machine connection first
      --json
          Output as JSON
  -h, --help
          Print help
```

Flags override `models.toml` for this connection and are saved to it. `--url` must be `https`, or plain `http` only to this computer. With an existing machine connection, `connect` refuses unless you pass `--replace`.

## `quome models status`

Shows the connection (organization, approval expiry), the settings from `models.toml`, whether the local server answers and which models it lists, whether the background service is installed, and what the platform sees. It warns when the approval is close to expiring. Without `--json` it exits 1 when this machine is not connected; with `--json` it always exits 0 and reports `connected`.

```
Show this machine's connection, settings and what the platform sees

Usage: quome models status [OPTIONS]

Options:
      --json  Output as JSON
  -h, --help  Print help
```

## `quome models disconnect`

Revokes this machine on the platform (a failure is reported, not fatal), removes the background service if it is installed, deletes the machine key and removes the `models` profile. Your other profiles are left alone.

```
Revoke this machine and forget its connection

Usage: quome models disconnect [OPTIONS]

Options:
      --json  Output as JSON
  -h, --help  Print help
```

## `quome models serve`

Opens one outbound tunnel to the gateway and answers the requests it sends to your local server. It runs in the foreground; transient failures back off and retry inside the process. Prompts, completions and tokens are never logged: log lines carry a request id, the path, the status and the time taken.

`--install` writes a background service and starts it; `--uninstall` stops and removes it. `--install` refuses when this machine is not connected, and never starts a second foreground `serve`.

| Platform | Service | File |
|----------|---------|------|
| macOS | launchd user agent `cloud.quome.models` | `~/Library/LaunchAgents/cloud.quome.models.plist` (log: `~/Library/Logs/quome/models.log`) |
| Linux | systemd user unit `quome-models.service` | `~/.config/systemd/user/quome-models.service` |

The service runs `quome models serve` with `QUOME_NO_AUTO_UPDATE=1`, using the `quome` found on your `PATH` when it is this binary (a Homebrew symlink survives upgrades), otherwise this binary's own path. On Linux, run ``loginctl enable-linger $USER`` to keep it running after you log out.

```
Serve this machine's models to the organization

Usage: quome models serve [OPTIONS]

Options:
      --install    Install it as a background service that starts at login
      --uninstall  Remove the background service
      --json-logs  Log as JSON lines
  -h, --help       Print help
```

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | Stopped for good: not connected, revoked or disconnected, or a stop signal |
| 3 | The binary was upgraded; the service manager starts the new one |

The service restarts only after a failure (launchd `KeepAlive` with `SuccessfulExit` false; systemd `Restart=on-failure`), so exit 0 stays stopped and exit 3 restarts into the upgraded binary.

## Files

| File | What |
|------|------|
| `~/.quome/cloud-models.key` | The machine's key, mode 0600 |
| `~/.quome/cloud.json`, profile `models` | The connection: organization, approval expiry. The default profile is never touched |
| `$XDG_CONFIG_HOME/quome/models.toml`, else `~/.config/quome/models.toml` | Settings (on macOS too) |

### `models.toml`

| Key | Default | Meaning |
|-----|---------|---------|
| `server` | `ollama` | `ollama`, `vllm`, `llama.cpp`, `mlx` or `quexo` |
| `url` | per kind: ollama `http://127.0.0.1:11434`, vllm `http://127.0.0.1:8000`, llama.cpp and mlx `http://127.0.0.1:8080`, quexo `http://127.0.0.1:52415` | The local server |
| `max_concurrency` | `4` | Requests served at once, 1 to 64 |
| `api_key_file` | none | A file holding a key for the local server, read at start and sent to the local server only |

### Why the key is a file, not the Keychain

The machine key is a 0600 file on every OS, including macOS. A background service cannot answer the Keychain prompt that an unsigned upgrade raises, so a Keychain key would stop the service after an upgrade. A file is protected the same way as on Linux: only your user account can read it. If you lose the computer or want to cut it off, revoke the machine from the console's Models, Machines page, or run `quome models disconnect`.

## Packaged releases

macOS only in packaged releases; Linux via `cargo install`. The macOS release assets are `quome-darwin-arm64.tar.gz` and its checksum `quome-darwin-arm64.tar.gz.sha256`.
