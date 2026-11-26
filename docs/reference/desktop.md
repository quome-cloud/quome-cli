# Desktop: `quome desktop`

Link a dedicated Mac to your organization as a computer its agents can use. An agent sees the screen and uses the mouse and keyboard through the desktop app, which ships separately as Quome Desktop.

This works on Apple silicon Macs only. On any other computer every `quome desktop` command says so and exits with code 69. It needs macOS 14 or later; on an older Mac the commands say so and exit with code 69.

## What a dedicated computer is

Use a Mac that agents can have to themselves, with its own standard account (not an administrator). Set the account up so an agent never meets a lock:

- It is a **standard account**, for example one named "Agent Desktop".
- It **signs in automatically** at startup (System Settings > Users & Groups).
- Its **screen does not lock** and the Mac does not sleep.
- It is **signed in to the apps the agent needs**, such as a browser or mail.

`quome desktop doctor` checks each of these and says what to change.

**FileVault.** With FileVault on, a Mac cannot sign in by itself after a restart. A person must unlock it once. Until then agents cannot use it. `doctor` reports this as a note, not a failure. Turn FileVault off only if your policy allows it.

Two things to know before you link a computer:

- An agent driving this computer can use every app and file the account can. Stop and Pause end its desktop tools, not programs it already started.
- This computer is your organization's: whoever controls it sees everything an agent does there.

## Commands

Run these in the dedicated account, from Terminal.

### `quome desktop connect`

```
Link this Mac to an organization as a desktop its agents can use

Usage: quome desktop connect [OPTIONS] [API_URL]

Arguments:
  [API_URL]  Quome Cloud API URL (default: the saved profile's API URL when reconnecting, else https://api.quome.cloud)

Options:
      --org <ORG>     Organization (slug) this computer serves
      --name <NAME>   Name shown for this computer (default: the computer's name)
      --small-frames  Give agents the smaller screen size of this display's shape
      --replace       Unlink an existing desktop link first
      --json          Output as JSON
  -h, --help          Print help
```

Opens an approval page in your browser. A person approves one switch there. Then `connect` installs the desktop app, starts the two background agents and opens the macOS permission panes the app needs (Screen Recording and Accessibility). Turn both on for Quome Desktop.

If this Mac is already linked but set-up did not finish, `connect` resumes it. If it is linked and set up, `connect` stops. Use `--replace` to unlink first and link again.

If the link is over on the platform (the computer was removed in the console, its approval was revoked or expired, or its approver is no longer an Owner or Admin), `connect` says so, forgets the old link on this Mac and asks for a new approval. Running the install command again does the same. The macOS permissions are not asked for again.

### `quome desktop serve`

```
Run the link between this computer and the platform (started by launchd)

Usage: quome desktop serve [OPTIONS]

Options:
      --install    Install and start the background agents, then exit
      --uninstall  Stop and remove the background agents, then exit
  -h, --help       Print help
```

You do not run this yourself. launchd starts it. `--install` and `--uninstall` put the background agents in or take them out without linking or unlinking.

### `quome desktop status`

```
Show whether this computer is linked and what the platform and the app say

Usage: quome desktop status [OPTIONS]

Options:
      --json  Output as JSON
  -h, --help  Print help
```

### `quome desktop doctor`

```
Check this Mac for what would stop agents using it

Usage: quome desktop doctor [OPTIONS]

Options:
      --json  Output as JSON
  -h, --help  Print help
```

Checks that the account is a standard account, signs in automatically, has no screen lock and does not sleep, that the app is installed and signed, that it has its permissions, and that the platform sees this computer online. FileVault is reported as a note.

### `quome desktop disconnect`

```
Unlink this computer and stop its background agents

Usage: quome desktop disconnect [OPTIONS]

Options:
      --json  Output as JSON
  -h, --help  Print help
```

## Files it writes

| What | Where |
|------|-------|
| Settings (platform URL, organization, screen size) | `~/.config/quome/desktop.toml` |
| The `desktop` profile | `~/.quome/cloud.json` |
| The computer's key | macOS Keychain, service `cloud.quome.agent`, account `desktop` |
| The desktop app | `~/Applications/Quome Desktop.app` |
| Runtime files (`session.json`, `state.json`) | `~/Library/Application Support/Quome Desktop/run` |
| Background agent that keeps the link | `~/Library/LaunchAgents/cloud.quome.desktop.serve.plist` |
| Background agent that runs the app | `~/Library/LaunchAgents/cloud.quome.desktop.app.plist` |
| Logs | `~/Library/Logs/quome/desktop.log` and `desktop-app.log` |

The `desktop` profile is separate from the default `quome` profile. Linking a desktop does not change how your other commands sign in.

## Unlinking

```bash
quome desktop disconnect
```

This removes the computer from your organization, stops and removes both background agents, removes the `session.json` file, the `desktop` profile, its Keychain item and `desktop.toml`. Quome Desktop stays in `~/Applications`; drag it to the Trash to remove it. The computer disappears from the console within a few seconds. The default `quome` profile keeps working.
