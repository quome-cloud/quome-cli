# Logs: `logs`

Fetch recent application logs, grouped by the serving revision (each deployment creates a new revision, so groups line up with deploys).

```
Usage: quome logs [OPTIONS]

Options:
      --app <APP>      Application ID (uses linked app if not provided)
      --org <ORG>      Organization ID (uses linked org if not provided)
  -n, --limit <LIMIT>  Number of log entries to fetch [default: 200]
      --json           Output as JSON
```

```console
$ quome logs
── my-api-00003-xyz ──
2026-07-02 07:22:31 INFO  Server listening on port 3000
2026-07-02 07:22:35 INFO  GET /healthz 200 2ms
2026-07-02 07:23:02 WARN  Slow query: 1240ms
2026-07-02 07:23:41 ERROR connection reset by peer
```

Severities are color-coded: `DEBUG` dim, `INFO` blue, `WARN` yellow, `ERROR` red.

## Examples

```bash
quome logs -n 500                    # more history
quome logs --app 7c9e6679-...        # a specific app, no link needed
quome logs --json | jq -r '.revisions[].logs[] | select(.severity=="ERROR") | .message'
```

The `--json` shape mirrors the API: `{"revisions": [{"revision_name": ..., "logs": [{"timestamp", "severity", "message"}]}]}`.

> **Note:** `logs` is a snapshot, not a live tail. Re-run it (or `watch -n 5 quome logs -n 50`) to follow along.
