# Link: `link`, `unlink`

Linking binds the **current directory** to an organization and (optionally) an application. Linked context is what lets you run `quome logs` instead of `quome logs --org <uuid> --app <uuid>` in every project.

## `quome link`

```
Usage: quome link [OPTIONS]

Options:
      --org <ORG>  Organization ID (skips interactive selection)
      --app <APP>  Application ID (skips interactive selection)
```

Interactive:

```console
$ quome link
? Select organization: acme (0d9f...)
? Select application: my-api (7c9e...)
✓ Linked
  Organization  acme
  Application   my-api
```

You can skip the app ("Skip - don't link an app") — org-level commands (`secrets`, `db`, `events`, ...) only need the org.

Non-interactive:

```bash
quome link --org 0d9f... --app 7c9e...
```

Links are stored per-directory in `~/.quome/config.json`, so each project directory can point at a different org/app.

### Precedence

Any command that uses linked context resolves it as: `--org`/`--app` flag → `QUOME_ORG`/`QUOME_APP` env var → the linked directory. So links are the default, never a cage.

## `quome unlink`

```console
$ quome unlink
Success! Unlinked current directory.
```

Removes the link for the current directory only. Other directories keep theirs.
