# Deploy: `deploy`

Upload a directory of static files straight to Quome — no git push, no build step on our end. This is the CLI's one-shot path for static sites: `quome deploy` builds a manifest of your directory, uploads every file to signed GCS URLs in parallel, and flips the app's live pointer once the upload is validated.

```
Usage: quome deploy [OPTIONS] [DIRECTORY]

Arguments:
  [DIRECTORY]  Site root to deploy (your build output — must contain index.html) [default: .]

Options:
  -a, --app <APP>  App slug or UUID (uses linked app if not provided)
      --create     Create the app if the slug doesn't exist yet
      --org <ORG>  Organization ID (uses linked org if not provided)
      --json       Output as JSON
```

## The index.html requirement

`DIRECTORY` must contain `index.html` at its root — that's the same check the server runs on drag-drop uploads, so a bad deploy fails locally before any bytes move. Point `deploy` at your build output directory (`dist/`, `build/`, `out/`, ...), not your source tree.

Along the way, junk is filtered the same way the dashboard's drag-drop does: `.git/`, `node_modules/`, `.DS_Store`, `Thumbs.db`, `__MACOSX/` are dropped; meaningful dotfiles like `.well-known/` are kept. Deploys are capped at 5000 files.

## `--app` / `--create`

- No `--app`: uses the directory's linked app (`quome link`).
- `--app <slug-or-uuid>`: targets that app. A bare UUID is used as-is; anything else is looked up by slug in the org.
- `--app <slug> --create`: if no app with that slug exists yet, creates a new static app (source type `static`, framework `plain`) and deploys into it — the one-command path for a brand-new site.

Without `--create`, an unresolved slug fails with the list of existing app slugs so you can fix a typo.

## Permissions

Deploying resolves to an **admin-level** permission on `app` (create, upload, and finalize together move what's live) — a key needs scope `*` or `admin:app`. A `write:app` key can list and update apps but `quome deploy` will 403. See [Authentication → Scopes](../authentication.md#scopes).

## What happens under the hood

1. Build the local file manifest (path + size per file).
2. `POST .../static/sites` — idempotent; provisions the site's GCS bucket on first deploy, no-ops after.
3. `POST .../static/deployments` — starts an upload session and returns one V4 signed PUT URL per file.
4. Upload every file directly to GCS, 8 at a time, with a progress bar.
5. `POST .../static/deployments/{id}/finalize` — validates the upload and promotes it to the live deploy.
6. Poll deploy status every 2s (up to 180s) until it reaches `active` or `failed`.

The uploads bypass the CLI's usual authenticated client entirely: signed URLs carry their own auth in the query string, so sending an API key header would break the GCS signature. If a deploy fails to reach `active`/`failed` within the poll budget, re-run `quome deploy` — restarting is always safe, it's a new upload session.

## Example

```console
$ quome login
? API key: ************************************
✓ Logged in

$ quome deploy ./dist --app my-site --create
App my-site not found — creating it.
Deploying 42 files (183204 bytes) to my-site
██████████████████████████████ 183204/183204 179.3 KiB/s
✓ Deployed
  URL  https://my-site.example.com
```

`--json` prints `{"deployment_id": ..., "status": "active", "url": ...}` — `url` is `null` while DNS/routing is still catching up (rare; check the app page in that case).
