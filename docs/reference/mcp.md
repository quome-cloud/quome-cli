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
| `guide` | none | Reads the Quome Cloud agent guide: how sandboxes, previews, app deploys, email inboxes, auth and Git work, with recipes for each. Claude is told to call it first. |
| `create_sandbox` | `name?`, `memory_gb?`, `runtime?`, `auth_client?`, `auth_redirect_path?`, `repo?`, `ref?` | Creates a sandbox (a Linux workspace with a coding-agent runtime) and waits until it is running (up to 180 s; it stops early if the sandbox fails, stops or is deleted). `name`: 2-31 chars, a lowercase letter then lowercase letters, digits or hyphens; default `agent-<6 hex>`. `memory_gb`: 0.5, 1, 2, 4, 8, 16, 32 or 64; default 4. `runtime`: `claude`, `codex`, `opencode` or `antigravity`; default `claude`. `auth_client` / `auth_redirect_path`: bind the sandbox to a client of the org's sandbox auth (`auth_redirect_path` is the callback path on the sandbox; default `/auth/callback`, the platform's default); the sandbox then gets `QUAUTH_ISSUER`, `QUAUTH_CLIENT_ID` and `QUAUTH_CLIENT_SECRET`. `repo` / `ref`: clone a repo of the org's Git into `/workspace/<name>` at creation (see [Sandboxes from a repo](#sandboxes-from-a-repo)). |
| `list_sandboxes` | none | Lists this organization's sandboxes. |
| `stop_sandbox` | `sandbox` | Stops a sandbox (name or id). |
| `exec` | `sandbox`, `command`, `timeout_s?` | Runs a shell command in the sandbox's `/workspace`. Returns `exit_code`, `stdout`, `stderr`, `truncated`. `timeout_s` is clamped to 1-25; default 25. |
| `write_file` | `sandbox`, `path`, `content?`, `content_base64?` | Writes a file. `path` must be absolute, under `/workspace/`, with no `..` segment. Give exactly one of `content` (text) or `content_base64` (binary). |
| `sync_folder` | `sandbox`, `folder`, `dest?` | Uploads a local folder into the sandbox under `dest` (default `/workspace`; otherwise a path under `/workspace/` with no `..` segment), honouring the folder's `.dockerignore`. The `.dockerignore` itself is not uploaded. Existing files are overwritten; files in the sandbox that are not in the folder are left alone. Returns `files` and `bytes`. On a failed upload it names the file it stopped at and how many were written before it. |
| `read_file` | `sandbox`, `path` | Reads a text file, up to 1 MiB. For larger or binary files use `exec` (for example `base64` or `head`). |
| `list_files` | `sandbox`, `path?` | Lists a directory; default `/workspace`. |
| `preview` | `sandbox`, `port` | Returns a public preview URL for a port the sandbox is serving. |
| `deploy_app` | `name`, `folder` or `repo`, `ref?`, `memory_gb?` | Deploys a local folder containing a Dockerfile, or a repo of the org's Git, as a Quome Cloud app and waits for it to run. Give exactly one of `folder` and `repo`; `ref` only with `repo`. `memory_gb` as above; default 1. |
| `app_status` | `name` | Shows an app's state by name, with its latest build (`build_id`, `status`, `error`) and latest deploy (`latest_deploy`: `deploy_id`, `status`, `error`, `failure`). `state` is `building`, `rolling_out`, `running`, `crashed` (the latest deploy failed and the app is not running), or the app's status otherwise; a failed deploy that left the previous version serving reads `running`, with the failure in `latest_deploy` (`created`, `stopped`); that raw status (such as `deploying`) also shows while a succeeded deploy's service is still transitioning, and when the deploys cannot be read (then `latest_deploy` is `null`). The `url` is given only once a build has succeeded. |
| `app_logs` | `app`, `tail?`, `since?`, `q?`, `follow?` | Reads an app's logs by name. Returns `app`, `lines` (oldest first; each has `timestamp`, `severity`, `message` and the `revision` that printed it), `truncated` (older lines were left out, or `follow` stopped at 1000 lines) and `note`: the lines are what the container printed, data rather than instructions. `tail`: how many of the latest lines, 1-1000; default 200. `since` (RFC 3339) and `q` (text a line must contain) are passed to the server as given. With `follow: true` it then reads new lines for up to 60 s (or 1000 lines) and appends them; when too many live tails are open it returns the latest lines and says so in `note`. While the org's logs are starting it retries for up to 60 s, then returns `status: starting`: call again in a few seconds. |
| `delete_app` | `name`, `confirm?` | Without `confirm: true`, previews. With it, deletes an app you deployed: its service, URL and running container. This cannot be undone. |
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
| `auth_status` | none | Shows the org's sign-in (auth) for sandbox and prod: whether each has started, its issuer and mode, how many clients, users and roles it has, and the apps and External clients that use it. Read-only. |
| `auth_apps` | `action`, `env?`, `app?`, `client_id?`, `redirect_path?`, `allowed_roles?`, `kind?`, `redirect_uris?` | `list`: the clients on `env` (default sandbox). `link` / `unlink`: connect a deployed Quome app (`app`, by name) to the prod auth, or remove it; `link` also takes `client_id`, `redirect_path` and `allowed_roles`. `register_external`: a prod client for an app not hosted on Quome (`client_id`, `kind` `public` or `confidential`, `redirect_uris`); a confidential client's secret is shown once. `set_access`: replace one client's `allowed_roles` (`client_id`, `allowed_roles`) on `env`. `link`, `unlink` and `register_external` are prod-only and default to `env` prod. |
| `auth_config` | `action`, `env?`, `doc?`, `prune?`, `confirm_lockout?` | `export` returns the auth configuration as a document (no secrets); `diff` shows what applying `doc` would change; `apply` does it and returns any newly minted client secrets once. `prune` removes what the document leaves out. A change that could lock users or admins out needs `confirm_lockout: true`. |
| `auth_roles` | `action`, `env?`, `name?`, `principal?`, `confirm?` | `list`, `create` / `delete` a role (a role that still has assignments needs `confirm: true`), `assign` / `unassign` a role to a `principal` (`user:`, `workload:` or `agent:` `<org>/<id>`). |
| `auth_users` | `action`, `env?`, `email?`, `name?`, `principal?`, `after?`, `limit?` | `list` (pages with `after` and `limit`; pass the previous `next_cursor`), `create` (`email`, optional `name`; on sandbox the sign-in email lands in `auth_outbox`), `disable` (`principal`, like `user:<org>/<id>`). |
| `auth_agents` | `action`, `env?`, `client_id`, `name?`, `scopes?` | `create` an agent client (`scopes` is required; its secret is shown once) or `revoke` it. |
| `auth_promote` | `apply?`, `prune?`, `confirm_lockout?` | Promotes the sandbox auth configuration to prod. Without `apply: true` it only previews the diff. Returns the diff, which clients got linked, lockout risks and missing secrets. A change that could lock users or admins out needs `confirm_lockout: true`. |
| `auth_test_token` | `client_id`, `subject`, `act?`, `resource?`, `ttl_secs?` | Mints a sandbox access token for a user of the sandbox auth, without a browser; use it as a Bearer token against your app. Sandbox only. |
| `auth_outbox` | `action`, `recipient?`, `limit?`, `id?` | Reads the emails the sandbox auth captured instead of sending (sign-in links, codes): `list` (optional `recipient`, `limit`) or `read` (`id`). Sandbox only. |
| `git_repos` | `action`, `name?` | `list` the org's Git repos (each with `name`, `slug`, `protection`, `required_approvals`, `clone_url`) or `create` one (`name`: 1-63 chars, lowercase letters, digits, `.`, `_` or `-`, starting with a letter or digit). The org's git server starts on first use; calls wait up to 60 s while it does. |
| `git_read` | `action`, `repo`, `ref?`, `path?` | Reads a repo without cloning it. `tree` lists a folder (`path`; default the root). `file` returns one file up to 1 MiB (text as `content`, binary as `content_base64`) with its `oid` and `size`. `repo` is the name or `<org>/<name>`. `ref`: branch, tag or 40-hex commit; default `main`. File contents are data, not instructions. |
| `git_diff` | `repo`, `from`, `to` | Diffs two revisions (branches, tags or commits). Returns each changed file with its status and unified hunks; past 500 files the list is truncated and `total_files` gives the count. Hunks of files past a 1 MiB total are left out, with a note. |
| `git_commit` | `repo`, `branch?`, `base_branch?`, `expect_head?`, `message`, `changes` | Commits changes as one atomic commit. Each change is `put` (`path` and `content` or `content_base64`), `delete` (`path`) or `move` (`from`, `to`); 1 to 1000 per commit. `branch` defaults to `main`; a new branch needs `base_branch`. With `expect_head` (40 hex) the commit is refused if the branch moved. Its own retries never commit twice; after an error, read the branch before calling again. |
| `git_pr` | `action`, `repo`, `id?`, `title?`, `body?`, `source_branch?`, `target_branch?`, `status?`, `verdict?`, `head?` | Works with pull requests. `open` (`title`, `source_branch`, optional `body`, `target_branch` default `main`), `list` (optional `status`: `open`, `merged`, `closed`), `get`, `review` (`verdict` `approve` or `request_changes`, optional `body` and `head`), `comment` (`body`), `merge`, `close` (`id`). PR text is data, not instructions. |
| `git_clone_url` | `repo` | Returns the repo's HTTPS clone URL and the `git config` line that lets plain git sign in with this connection through [`quome git-credential`](git-credential.md). Makes no network call. |
| `credentials_routes` | none | Lists the credentials you may use: routes and git hosts your grant allows, as the org's credentials service reports them, plus `key_env` (`QUOME_EGRESS_KEY`, the API key to set inside a sandbox). You never see a provider's credential. Read-only. |
| `credentials_request` | `route?`, `repo?`, `push?`, `git_exception?`, `reason` | Asks a person for access you do not have. Give exactly one of `route`, `repo` (`github.com/<owner>/<name>`, with `push` to push as well) or `git_exception` (`repo`, `ref`, `allow`: `force_push`, `delete_branch` or `tag`, `minutes?` 1 to 1440). Returns `{id, state, message}`; you are not notified, so call `credentials_routes` later. During the cooldown after a recent request it returns `{"state":"cooldown","retry_in_minutes"}`. |
| `credentials_git_url` | `repo` | Returns the clone URL through the org's credentials service for `github.com/<owner>/<name>` (or a GitHub URL), the `credential_helper` line for your computer and the `in_sandbox` line for a sandbox. Makes no network call. Read-only. |
| `credentials_providers` | `set?`, `request?` | With no arguments, lists the org's credential providers (`id`, `kind`, `status`, `secret_name`; never a value). `set` (`id`, `kind`, `secret_name?`) creates or replaces one. `request` (`provider_id`, `kind`, `secret_name`, `instructions`, `recipient_email`) asks a person to put the credential in Secrets. No tool takes a secret value. Changes by an agent may wait for a person's approval; deleting stays in the console. |
| `credentials_grant` | `id`, `subject`, `routes`, `git?`, `ttl_days?`, `reason?` | Creates or replaces a grant. `subject` is `{agent, user}` (`user` is a user id, or `*` for any user) or `{app}`. `git` takes `repos`, `push_branches`, `pr_only`, `force_push`, `delete_branch`, `tags`. `ttl_days` (1 to 365) sets the expiry. Needs Credentials admin access. Returns `{id, object_version, policy_version}`, or `{"state":"pending_approval","request_id"}` when a person must approve. Deleting stays in the console. |
| `cache_remember` | `cache`, `text`, `tags` | Remembers a fact under tags in your pod in a Quome Cloud cache; returns its `id`. `cache`: the cache's name or id (the agent asks the person when it does not know it). `text`: the fact, in plain words, 1 to 65,536 characters. `tags`: 1 to 16 tags. A tag starts with a lowercase letter or digit and may contain lowercase letters, digits, `_`, `.`, `:` or `-`, at most 64 characters; these words cannot be tags: `anyof`, `since`, `until`, `topk`, `withfields`, `ts`, `tag`, `before`, `limit`. Use a few predictable tags: recall needs them and no tool lists them. |
| `cache_recall` | `cache`, `tags`, `topk?` | Recalls the facts in your pod that carry all the given tags (1 to 16), newest first. Each fact has `id`, `body` (the text remembered), `tags` and `ts` (when it was remembered, in Unix milliseconds). `topk`: most facts to return, 1 to 100; default 10. |
| `cache_forget` | `cache`, `id` | Forgets a fact by the `id` that `cache_remember` or `cache_recall` gave. Returns `{"id", "forgotten"}`; `forgotten` is `false` when there was no such fact. |
| `cache_get` | `cache`, `key` | Reads a key from your pod. `key`: 1 to 512 bytes. Returns `{"key", "value"}`; `value` is `null` when the key is not set. |
| `cache_set` | `cache`, `key`, `value`, `ttl?` | Sets a key to a value in your pod. `value`: at most 1 MiB of text. `ttl`: seconds until the key expires, 1 to 31536000; without it the key stays until deleted. Returns `{"key", "set": true, "ttl"}` (never the value). |
| `cache_del` | `cache`, `key` | Deletes a key from your pod. Returns `{"key", "deleted"}`; `deleted` is `false` when it was not set. |
| `cache_pod` | `cache` | Shows the size and usage of your pod: memory in bytes (`maxmemory`, `used_memory`), `keys`, `facts` (with `maxfacts`) and `connected_clients`. |

### Long-running commands

`exec` returns after at most **25 seconds**. For anything longer (installs, builds, servers), start the job in the background and poll:

```bash
nohup npm run dev > /tmp/dev.log 2>&1 &     # start, returns immediately
tail -n 20 /tmp/dev.log                      # a later exec call: check on it
```

### Previews

`preview` exposes a port the sandbox is listening on and returns its public URL. If the port is already exposed, the existing URL is returned instead of creating a new one. Start the server first (in the background), then ask for the preview.

### Sandboxes from a repo

**Which local folders can be read.** `sync_folder` and `deploy_app` read a local folder only when it lies inside the project: the workspace roots the MCP client declares (`roots/list`), or, when the client declares none, the directory `quome mcp` was started in. When the client declares no roots and that directory is the home folder, one of its parents or `/`, there is no project boundary and both tools refuse: start the agent from the project folder, or let the client share its workspace roots. The path is resolved first (symlinks and `..`), so a link or `..` that leads outside is refused. The home folder itself, any folder containing it, and credential locations are always refused, even inside a root: `~/.ssh`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.kube`, `~/.docker`, `~/.gnupg`, `~/.netrc`, `~/.git-credentials`, `~/.quome` and `~/Library/Keychains` (also when such a location is a symlink, at its target). A refusal names the allowed roots; to upload another folder, start the agent in it or add it as a workspace root.

When the code to work on is already in the org's Git, pass `repo` to `create_sandbox` instead of syncing a folder: no local checkout is needed. `repo` is `<name>` or `<org>/<name>` (the org must be this org); `ref` is a branch, tag or commit and defaults to the repo's default branch. The repo is cloned into `/workspace/<name>` once the sandbox runs, as the person the agent acts for.

The clone is best effort: the sandbox is `running` even when the clone failed. The result carries `git_bootstrap` `{repo, ref, status, error, path}`, where `status` is `pending`, `done` or `failed`. While the clone is `pending` once the sandbox runs, `create_sandbox` keeps polling for up to 60 s and then reports what it has. A `failed` clone is not a tool error: `note` gives the error, and the agent can clone with `exec` instead (`git_clone_url` gives the URL). Connections made before Git access existed need `quome connect` again, allowing Git access.

### Deploying

`deploy_app` packs the folder on this computer (it must contain a `Dockerfile`; `.dockerignore` is honoured), creates the app if it does not exist (a deleted app of the same name is ignored), uploads the source and builds it. After the build succeeds it keeps waiting until the new revision is running. The whole wait is at most **10 minutes**. The result's `status` is one of:

| `status` | Meaning |
|----------|---------|
| `running` | Built and live. `url` is the app URL. |
| `failed` | The build failed (`error` has the tail), or it built but the container did not stay up: `error` says what was observed (exit code, seconds alive), `container_output` is its last lines. `note` says the container printed nothing only when its exit was reported (`seconds_alive` set); otherwise it says Quome Cloud has no record of the container's output. `note` names the build log route. |
| `building` | Still building after 10 minutes. Check later with `app_status`. |
| `deploying` | Built, but still rolling out after 10 minutes. Check later with `app_status`. |

**From a repo.** With `repo` (and optionally `ref`) instead of `folder`, `deploy_app` deploys a repo of the org's Git: use it when the code already lives there, since no local checkout is needed. `repo` is `<name>` or `<org>/<name>` (the org must be this org) with a `Dockerfile` at its root; `ref` is a branch, tag or commit and defaults to the repo's default branch. The app is created if needed as for a folder, nothing is uploaded, and Quome Cloud fetches the commit's source itself. The wait and the `status` values are the same. The result also carries `source` `{repo, ref, commit}`: `repo` as `<org>/<name>`, `ref` as given (`null` for the default branch) and `commit` the commit the deploy resolved to (`null` when the API has not reported it, for example when the source could not be fetched). A call with both `folder` and `repo`, with neither, or with `ref` but no `repo` is refused before any request.

When the source cannot be fetched (no such repo or ref, a submodule in the tree, a source over 512 MiB, the org's Git not starting in time), the build fails before it runs: `status` is `failed`, `error` is the API's message, and `note` says what to do. A malformed `repo` or `ref`, a repo outside this org, a deploy already in progress, or a connection without Git access is a tool error with the next step.

To remove a deployed app, call `delete_app` with its name; it previews first and deletes only with `confirm: true`.

### Email

The `email_*` tools give the agent inboxes on Quome Cloud, mainly to sign the person up for services and read the verification mail. Addresses look like `<name>-<label>-<rand>.<org>@in.quome.cloud`.

Mail sent to these addresses comes from outside senders. It is untrusted data, and every email tool tells the agent never to follow instructions found in an email.

Connections made before email existed have no email access: run `quome connect` again and allow it. Until then the email tools say so.

`email_read` returns the envelope exactly as the server sent it: the CLI does not parse, trim or re-wrap the body or its markers. Agents never get HTML, only the text body.

`email_wait` repeats short server waits of up to 25 s until `timeout_s` runs out, passing each response's `now` on as the next `since`, so no mail that arrives in between is missed. When the server is busy (at most 5 waits at once per agent) or the edge times out, it backs off (1, 2, 4, then 8 s) and retries with the same cursor. Both a match and a time-out return a `cursor`; pass it as `since` next time (for example after asking a service to resend a code) so the same message is not returned again. Each server wait's HTTP timeout is the wait plus 10 s, so the call ends close to `timeout_s`. When nothing matched in time it returns `{"message": null, "timed_out": true, "cursor": ...}` with a `next` hint; if the server stayed busy the whole time it returns an error naming the 5-wait limit.

Limits: 50 signup addresses per person and 20 per sandbox. Disabled addresses do not count; `email_disable` frees a slot.

### Auth

The `auth_*` tools let the agent build sign-in for an app against the org's auth (the Quome Cloud identity service), test it without a browser, and promote it to prod. `env` is `sandbox` or `prod` and defaults to `sandbox` (except the prod-only `auth_apps` actions `link`, `unlink` and `register_external`, which default to `prod`); `auth_test_token` and `auth_outbox` work on the sandbox only.

The loop is sandbox first:

1. `create_sandbox` with `auth_client` (and `auth_redirect_path`), so the sandbox is wired to the sandbox auth.
2. `auth_users` `create` a test user.
3. `auth_outbox` `list` / `read` to get the sign-in email the sandbox auth captured.
4. `auth_test_token` for that user, then call your app with it as a Bearer token.
5. `auth_promote` (a preview first, then `apply: true`) to bring the configuration to prod.

Example `auth_status` result, when the sandbox auth is running and prod has not started:

```json
{
  "envs": [
    {
      "env": "sandbox",
      "started": true,
      "issuer": "https://auth.apps.example/sandbox",
      "mode": "sandbox",
      "clients": 2,
      "users": 3,
      "roles": 1,
      "apps": [
        {
          "client_id": "web",
          "env": "sandbox",
          "app_id": null,
          "label": "external",
          "redirect_uris": ["https://apps.example/cb"]
        }
      ]
    },
    {
      "env": "prod",
      "started": false,
      "note": "This org's prod auth hasn't started: link an app or promote first.",
      "apps": []
    }
  ]
}
```

Example `auth_test_token` result:

```json
{
  "access_token": "AT",
  "id_token": "IT",
  "expires_in": 300,
  "note": "Sandbox token: use it as `Authorization: Bearer …` against your app."
}
```

Client secrets are shown once, in the tool result; store them when you get them.

Connections made before auth existed lack the `auth_read`/`auth_sandbox_write`/`auth_prod_write` scopes; run `quome connect` again. Prod changes need an org owner or admin.

### Git

The `git_*` tools give the agent the org's Git repos. Use them to read a repo, make a small commit and review pull requests without cloning anything:

- `git_repos` to find or create a repo, `git_read` to list a folder or read a file, `git_diff` to see what changed between two branches or commits.
- `git_commit` for a few files at a time; `git_pr` to open, review, comment on, merge or close a pull request.

For many files, builds or tests, clone the repo with plain git on this computer instead: `git_clone_url` returns the URL and the one-time `quome git-credential` setup so `git clone`, `git push` and `git fetch` sign in with this connection (see [Git credential](git-credential.md)).

`git_commit` sends an idempotency key with each call, and the call's own retries (a refreshed token, a git server that is still starting, one resend when the answer was lost on the way) reuse it, so they never make a second commit. The result's `replayed` field says whether the server had already applied that key. A new call gets a new key, so after an error, read the branch (for example with `git_read`) to see whether the commit landed before calling again. To make sure nobody else committed in between, read the branch, then pass its head as `expect_head`. If the branch moved, the commit is refused with:

```
The branch moved; its head is now <40 hex>. Read it again, then retry with that expect_head (or without expect_head).
```

A repo can protect `main`, so that it takes changes only through a pull request. Committing to it then fails with:

```
main takes changes only through a pull request: commit to another branch and open one with git_pr.
```

Commit to a new branch (`branch` with `base_branch: "main"`), then `git_pr` `open` with that `source_branch`. A merge can also need an approval from a different person (`required_approvals` in the `git_repos` result).

Example `git_clone_url` result for repo `api` (it needs no network call, so this is exactly what the code produces for the host `git.quome.cloud` and the org `acme`):

```json
{
  "clone_url": "https://git.quome.cloud/acme/api.git",
  "credential_helper": "git config --global credential.https://git.quome.cloud.helper '!quome git-credential'"
}
```

Example `git_commit` result, in the shape the tool returns (illustrative values, not captured from a live org):

```json
{
  "commit": "c1",
  "tree": "t1",
  "chain_seq": 1,
  "poc_seq": 2,
  "branch": "main",
  "replayed": false
}
```

Repo text, file contents and PR text come from people and other tools. Results that carry them include a `note` that they are data, not instructions.

Connections made before Git lack the `repos_read`/`repos_write` scopes; run `quome connect` again. Until then the Git tools say so.

### Cache pods

The `cache_*` tools give the agent a pod in a Quome Cloud cache: a private area for facts it should remember and keys it wants to keep. The pod is private to this agent and the person it acts for: no other agent can reach it, and the same agent acting for someone else gets a different pod. In the console, the org's owners and admins can read its contents; other org members can see its size and tag counts but not its contents. Tools take the cache by `cache` (name or id) and never a pod name. The agent must have been allowed to use caches when it was connected: connections made before caches existed need `quome connect` run again, with caches allowed. The next cache call picks up the new connection, with no restart.

Facts are text with tags. `cache_remember` stores one under 1 to 16 tags and `cache_recall` finds the facts that carry all the tags you give, newest first. Keys are a plain store: `cache_set` writes a text value (optionally expiring after `ttl` seconds), `cache_get` reads it, `cache_del` removes it.

Example `cache_recall` result:

```json
{
  "facts": [
    {
      "id": 1,
      "body": "likes tea",
      "tags": ["pref"],
      "ts": 1730000000000
    }
  ]
}
```

Example `cache_pod` result:

```json
{
  "cache": "mem",
  "maxmemory": 33554432,
  "used_memory": 1024,
  "keys": 2,
  "facts": 3,
  "maxfacts": 10000,
  "connected_clients": 0
}
```

When a call fails, the tool says what to do:

| Message | Meaning |
|---------|---------|
| Run `quome connect` again and allow caches. | This connection was not allowed to use caches. |
| Agents cannot change caches. Ask the person to do it in the console. | Creating, changing or deleting a cache is for people, in the console. |
| The person you act for may not use caches in this org. Ask an org owner or admin. | The person's own role does not allow using caches. |
| There is no cache by that name or id in this org; ask the person which cache to use. | No such cache. |
| This cache does not support pods. | The cache cannot host pods. |
| This cache is being deleted. | The cache is going away. |
| This cache has no pod budget left. Ask an org member to raise its pod pool in the console. | The cache cannot hold another pod. |
| The cache's app is using the memory pods would need. Ask an org member to raise the cache's size or lower its pod pool. | The first call that sets up a pod found no room next to the app's own data. |
| The cache is not running. Start it in the console, then try again. | The cache is stopped. |
| This cache needs a restart to enable pods. Ask an org member to restart it in the console. | The cache was made before pods and has not been restarted since. |
| Your pod could not be set up on this cache. Ask an org member to check its pods in the console. | The pod is in a state the server will not hand out. |
| Your pod was not found on this cache; try again. | The pod went away during the call. |
| Your pod is full. Forget or delete something, or ask for a larger pod. | The pod's memory or fact limit is reached. |
| This agent is no longer connected to the org. Run `quome connect` again. / The person this agent acts for is no longer a member of the org. | The agent or its person left the org. |
| Tags start with a lowercase letter or digit and may contain lowercase letters, digits, `_`, `.`, `:` or `-` (at most 64 characters each); a call takes 1 to 16 tags; these words cannot be tags: anyof, since, until, topk, withfields, ts, tag, before, limit. | A tag, or the number of tags, is not allowed. |
| The text must be 1 to 65,536 characters. / topk must be 1 to 100. / A key must be 1 to 512 bytes of text. / A value must be text of at most 1 MiB. / ttl must be 1 to 31,536,000 seconds. / That is not a fact id; use an id from cache_remember or cache_recall. / The request is too large; a value can be at most 1 MiB. | An argument is out of range. |
| Quome Cloud is busy; wait a second and try again. / The cache did not answer in time; try again shortly. / The cache could not complete the call; try again shortly. | A passing failure on the server or the cache. |

If calls come too fast (more than 50 a second for one agent) the tool says to wait and try again. Any other error is `Could not <action>: ` followed by the server's message.

### Database branches

The `database_*` tools let an agent work on its own branch of an org database. A **branch** is the agent's private copy to change: it never touches the main data. A **grant** is the access the agent holds on its branch: a list of actions and tables, with an expiry. The grant is judged from the intent the agent states when it enrolls, so it covers only what the agent said it meant to do, and no more than the database's agent rules (set by an org admin) allow. Tools take the database by `database` (name or id) and never a branch, a principal or a grant id.

The flow:

1. `database_enroll` with an honest `intent`. The result lists the grant's actions, tables and expiry.
2. Work: `database_query`, `database_search`, `database_ask`, and `database_commit` for your own branch.
3. Each grant expires. A call after that fails and says to call `database_enroll` again, with a new intent if the work has changed. The tools never re-enroll on their own.
4. A person merges the branch. Agents never merge, and no tool does.

The agent must have been allowed to use databases when it was connected: connections made before databases existed need `quome connect` run again, with databases allowed. The next call picks up the new connection, with no restart.

| Tool | Args | What it does |
|------|------|--------------|
| `database_enroll` | `database`, `intent` | Asks for access to your own branch of a database (name or id). Describe honestly what you mean to do; the grant is judged from it and returns its actions, tables and expiry. Each grant expires; enroll again then. |
| `database_my_branch` | `database` | Shows your branch and current grant: actions, tables and expiry. Creates your branch if it doesn't exist yet. |
| `database_query` | `database`, `sql` | Runs one SQL statement on your branch, within your grant. Returns `columns`, `rows`, `row_count` and `truncated` (capped at 1,000 rows and 1 MiB; a `note` says so when cut off). |
| `database_search` | `database`, `table`, `column`, `text?`, `vector?`, `metric?`, `k?` | Searches a column by `text` or by `vector` (exactly one). `metric`: `l2`, `cosine` (default) or `inner_product`. `k`: 1 to 100, default 10. |
| `database_ask` | `database`, `question` | Asks a question about the data on your branch in plain words. |
| `database_commit` | `database`, `message` | Commits your branch. A person merges it; you never merge. |

`database_query` and `database_search` results are shown as the server returns them: at most 1,000 rows and 1 MiB. When a result was cut off, `truncated` is `true` and a `note` says so.

When a call fails, the tool says what to do:

| Reason | Message to the agent |
|--------|----------------------|
| `reconnect_for_databases` | Run `quome connect` again and allow databases. (The call is retried once after a reconnect.) |
| `no_charter` | This database has no agent rules yet. Ask an org admin to set them on the database's Agents tab. |
| `models_not_connected` | No reasoning model key is connected. Ask an org admin to connect one in Secrets. |
| `engine_predates_agents` | This database needs an update before agents can use it. |
| `intent_refused` | Your stated intent is outside this database's agent rules. Restate it, or ask a person. |
| `not_enrolled` | You have no grant here yet. Call `database_enroll` first. |
| `grant_expired` | Your grant has expired. Call `database_enroll` again. |
| `charter_changed` | The database's agent rules changed while you enrolled. Call `database_enroll` again. |
| `revoked`, `revoke_pending` | A person revoked this agent's access, or is revoking it. |
| `branch_gone` | Your branch was removed. Call `database_enroll` to get a new one. |
| `branch_conflict` | Your branch could not be updated because of a conflict. |
| `one_statement`, `statement_too_large` | Send one SQL statement per call, and keep it small. |
| `statement_refused` | Your grant does not cover that statement. |
| `busy`, `rate_limited`, `engine_error`, `database_unreachable`, `session_failed` | The database could not take that call; try again shortly. |
| `timed_out` | The database did not answer in time. |
| `database_deleting`, `not_running` | The database is being deleted, or is not running. |
| `database` (404) | No such database; check its name or id. |
| other 404 `not_...` | This database type does not support agent branches. |
| `invalid_intent`, `invalid_question`, `invalid_message`, `invalid_sql` | The text must be plain text (for `database_query`, one SQL statement). |
| `invalid_text` | Search text may not contain a backslash or a NUL character. |
| `invalid_vector`, `invalid_identifier`, `invalid_metric`, `invalid_k`, `text_too_long`, `exactly_one_of_text_or_vector` | The search arguments are not valid: give exactly one of `text` or `vector`, `metric` is `l2`, `cosine` or `inner_product`, `k` is 1 to 100. |
| `search_failed` | The search could not be completed. |
| `agents_only` | Database branches are for agents connected with `quome connect`. |

### Progress and cancelling

`create_sandbox`, `deploy_app`, `app_logs`, `email_wait` and the `git_*` tools that call the org's git server (all but `git_clone_url`) send MCP progress notifications while they wait, when the client asks for them, and stop when the client cancels the call (in Claude Code, press Esc). Claude Code allows long tool calls; clients that time out requests after about 60 seconds reset that timer on each progress notification. Where a client still gives up early, raise its tool timeout (in Claude Code, the `MCP_TOOL_TIMEOUT` environment variable, in milliseconds). The work continues on the server either way, so `list_sandboxes` or `app_status` shows the outcome.

## Messages

When the connection is no longer valid (revoked on the dashboard, or expired), every tool returns the following (the `cache_*` tools put `Could not <action>: ` in front of it):

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
