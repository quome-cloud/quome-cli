# Tutorial: Deploy your first app

**Goal:** a container image running on Quome with a public URL.
**Time:** ~5 minutes. **You need:** the CLI installed and an API key ([getting started](../getting-started.md)).

## 1. Log in and link

```console
$ quome login
? API Key: ********
✓ Logged in
  Email  you@example.com

$ mkdir hello-quome && cd hello-quome
$ quome link
? Select organization: acme (0d9f...)
? Select application: (Skip - don't link an app)
✓ Linked
  Organization  acme
```

We skip the app link because the app doesn't exist yet.

## 2. Create the app

Any public container image works. We'll use nginx because it's instant:

```console
$ quome apps create hello --image nginx:1.27 --port 80
✓ Created application
  ID      7c9e6679-7425-40de-944b-e07fc1f90ae7
  Name    hello
  Status  pending
```

Two flags did all the work:

- `--image nginx:1.27` — the container to run. For your own app, push to any registry (`ghcr.io/you/app:tag`) and use that.
- `--port 80` — the port your container listens on (default is 8080; nginx uses 80).

## 3. Link the app and watch it start

```console
$ quome link --org 0d9f... --app 7c9e6679-7425-40de-944b-e07fc1f90ae7
✓ Linked
  Organization  acme
  Application   hello
```

Now watch the status flip from `pending` → `provisioning` → `running`:

```console
$ quome apps get
┌ hello ──────────────────────────────────────┐
│ ID       7c9e6679-...                       │
│ Name     hello                              │
│ Status   running                            │
│ Source   image                              │
│ Image    nginx:1.27                         │
│ URL      https://hello-acme.q.run           │
│ Created  2026-07-02 07:14:02                │
└─────────────────────────────────────────────┘
```

Open the URL. You should see the nginx welcome page — served from infrastructure that was provisioned into your org's own cloud project two minutes ago.

## 4. Check the logs

```console
$ quome logs
── hello-00001-abc ──
2026-07-02 07:15:58 INFO  ... start worker processes
2026-07-02 07:16:04 INFO  GET / HTTP/1.1 200
```

## 5. Ship an update

Push a new image tag to your registry, then trigger a redeploy:

```console
$ quome deployments create
✓ Deployment triggered
  ID      1ee7f2a4-...
  Status  created

$ quome deployments list
╭──────────────────────────────────────┬─────────────┬────────┬──────────────────╮
│ ID                                   │ STATUS      │ BRANCH │ CREATED          │
├──────────────────────────────────────┼─────────────┼────────┼──────────────────┤
│ 1ee7f2a4-...                         │ in_progress │ -      │ 2026-07-02 07:32 │
╰──────────────────────────────────────┴─────────────┴────────┴──────────────────╯
```

## 6. Clean up (optional)

```bash
quome apps delete 7c9e6679-7425-40de-944b-e07fc1f90ae7 --force
```

## What you learned

`create → link → get → logs → deploy` is the whole loop. Next:

- Your app probably needs config → [Manage secrets like a pro](manage-secrets-like-a-pro.md)
- Build from source instead of pushing images → [Deploy from GitHub](deploy-from-github.md)
- Automate this → [Scripting & CI](scripting-and-ci.md)
