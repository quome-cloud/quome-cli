# Configuration

The CLI reads configuration from two files and a handful of environment variables. Flags always win.

## Files

### `~/.quome/config.json`

Written by `quome login` and `quome link`. Holds your token and per-directory links:

```json
{
  "user": {
    "token": "qk_...",
    "id": "a1b2c3d4-...",
    "email": "you@example.com"
  },
  "linked": {
    "/Users/jane/projects/my-api": {
      "org_id": "0d9f...",
      "org_name": "acme",
      "app_id": "7c9e...",
      "app_name": "my-api"
    }
  }
}
```

You rarely edit this by hand — `login`, `logout`, `link`, and `unlink` manage it.

### `settings.json`

Optional. Overrides the API endpoint. Looked up in this order:

1. `./settings.json` (current directory — per-project override)
2. `~/.quome/settings.json` (global)

```json
{
  "api_url": "https://quome.studio"
}
```

## Environment variables

| Variable | Effect |
|----------|--------|
| `QUOME_TOKEN` | API key; overrides the stored login |
| `QUOME_ORG` | Organization UUID; overrides the linked org |
| `QUOME_APP` | Application UUID; overrides the linked app |
| `QUOME_API_URL` | API base URL; overrides settings files and the default |
| `QUOME_DEBUG` | Set to anything to print raw API responses to stderr |

## Precedence (highest first)

| Setting | Order |
|---------|-------|
| Organization | `--org` flag → `QUOME_ORG` → linked directory |
| Application | `--app` flag → `QUOME_APP` → linked directory |
| Token | `QUOME_TOKEN` → `~/.quome/config.json` |
| API URL | `QUOME_API_URL` → `./settings.json` → `~/.quome/settings.json` → `https://quome.studio` |

## Debugging a request

```console
$ QUOME_DEBUG=1 quome apps list
DEBUG response: {"data":[{"id":"7c9e...","name":"hello",...}],"meta":{"total":1,...}}
```

Useful when a command errors and you want to see exactly what the API returned. See also [Troubleshooting](troubleshooting.md).
