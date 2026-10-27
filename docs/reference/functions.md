# Functions

Run Python functions on Quome sandboxes with no cold start. QRust compiles a function to a
small WebAssembly component when you deploy it. Each call runs that component in a fresh wasm
instance inside a sandbox from a warm pool, and those sandboxes are already running and already
bootstrapped.

`quome fn` is a thin passthrough to `qufunction`, a separate binary, in the same way that
`quome host` passes through to `quome-host`. It passes along your `quome login`:

| variable | value | when |
|---|---|---|
| `QUOME_API_KEY` | your stored login (or `QUOME_TOKEN`) | unless already set |
| `QUOME_BASE_URL` | the API URL this CLI uses | unless already set |
| `QUOME_ORG_ID` | the linked org (`quome link`, `QUOME_ORG`, or the key's org) | unless already set |

Then it replaces itself with `qufunction`, so output, Ctrl-C and the exit code are qufunction's
own.

## Prerequisites

- `qufunction` on your `PATH`, in `~/.cargo/bin`, or at `$QUFUNCTION_BIN`. To install it, run
  `cargo install --git https://github.com/quome-cloud/qufunction` (this needs access to the repo).
- `qrustc` (QRust) and the `wasm32-wasip2` Rust target, on the machine you deploy from.

## Usage

```bash
quome fn deploy convert.py                        # compile + register
quome fn pool up                                  # warm sandboxes, off the request path
quome fn invoke convert units '{"value": 100, "unit": "f"}'
quome fn bench convert units '[1, "km"]' -n 40    # latency percentiles
quome fn ls | info NAME | rollback NAME | rm NAME
quome fn pool status | pool down
QUFUNCTION_TOKEN=… quome fn serve --listen 0.0.0.0:8787   # HTTP gateway, keeps the pool warm
```

`quome fn --help` and `quome fn <command> --help` print qufunction's own help. See the qufunction
README for how to write functions, pool settings (`min_idle`, `max_concurrency`, `max_uses = 1`
for a fresh sandbox per call), and limits.
