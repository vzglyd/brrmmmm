# brrmmmm

Standalone sidecar runner for VZGLYD sidecar WASM modules.

Loads any `wasm32-wasip1` sidecar, provides the host ABI it expects, and delivers the data payload to stdout.

## Usage

```bash
# One-shot fetch — prints JSON payload to stdout (pipeable)
brrmmmm run weather-sidecar.wasm --once

# Run continuously (sidecar controls polling)
brrmmmm run news-sidecar.wasm --log-channel

# Set environment variables the sidecar expects
brrmmmm run lastfm-sidecar.wasm --once \
  --env LASTFM_API_KEY=xxx \
  --env LASTFM_USERNAME=rodger

# Pass sidecar params through vzglyd_configure before running
brrmmmm run weather-sidecar.wasm --once \
  --params-json '{"api_key":"xxx","location":"Daylesford, VIC"}'

# Or read those params from a JSON file
brrmmmm run weather-sidecar.wasm --once --params-file sidecar-params.json

# Validate a WASM module loads and resolves imports
brrmmmm validate afl-sidecar.wasm
```

## Finish-line workflow

`brrmmmm` has two public surfaces:

- the Rust CLI/runtime, which is the canonical sidecar host
- the Ink TUI, which is a frontend over the CLI's `--events` protocol

For day-to-day development:

```bash
cargo test
npm --prefix tui run build
```

With Moonrepo installed, the same cross-ecosystem gate is:

```bash
moon run core:ci
```

Without a global Moon install:

```bash
npx --package @moonrepo/cli@2.2.1 moon run core:ci
```

The core acceptance path is:

```bash
brrmmmm validate sidecar.wasm
brrmmmm inspect sidecar.wasm
brrmmmm run sidecar.wasm --once > payload.json
brrmmmm sidecar.wasm
```

Application code should consume `published_output`. `raw_source_payload` and `normalized_payload` are debugging artifacts for developers and TUI frontends.

See `docs/frontend-protocol.md` for the stable NDJSON/stdin frontend protocol and `docs/release-checklist.md` for the public-release gate.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  brrmmmm (native host runtime)                      │
│                                                     │
│  WASI preview1 ───── env vars, stdio, filesystem    │
│  vzglyd_host ─────── channel, network, logging      │
│                                                     │
│  ┌───────────────────────────────────────────────┐  │
│  │  sidecar.wasm (wasm32-wasip1)                 │  │
│  │                                               │  │
│  │  poll_loop {                                  │  │
│  │    Ok(payload) → channel_push(payload)        │  │
│  │    Err(e) → exponential backoff               │  │
│  │  }                                            │  │
│  │                                               │  │
│  │  fetch() {                                    │  │
│  │    https_get("api.example.com", "/data")      │  │
│  │    parse(body) → typed payload                │  │
│  │    serde_json::to_vec(&payload)               │  │
│  │  }                                            │  │
│  └───────────────────────────────────────────────┘  │
│                                                     │
│  stdout ← channel_push captures first payload       │
└─────────────────────────────────────────────────────┘
```

### Why the Sidecar Is a Fat WASM Binary

Each sidecar `.wasm` is ~1.5–2MB because it carries its own network stack:

- **DNS resolver** — Google DoH bootstrap, TTL-cached
- **TLS stack** — rustls + rustls-rustcrypto (pure Rust, no OpenSSL)
- **HTTP/1.1 client** — hand-rolled parser supporting Content-Length and chunked encoding

This is intentional. The sidecar is a **self-contained data-fetching unit** that knows nothing about its host. It only requires two things:

1. **WASI preview1** — for raw TCP sockets (`sock_open`, `sock_connect`, etc.) during DNS bootstrapping and direct TLS connections
2. **`vzglyd_host` module** — for host-mediated network requests and the data channel to its paired slide

The host-mediated path (`vzglyd_host::network_request`) delegates HTTPS/TCP to the host runtime (brrmmmm natively via `reqwest`/`std::net`, or the VZGLYD native/web engine). The direct WASI path (DNS → TLS → HTTP in-WASM) is a fallback used during bootstrapping.

### Why This Is the Right Abstraction

**A sidecar is a standardized microservice for periodic data fetching.** Its contract is simple:

1. It knows *what* data to fetch and *how* to parse it
2. It knows *when* to fetch it (poll interval with exponential backoff on failure)
3. It knows *nothing* about who consumes the data

The consumer (a VZGLYD slide) only needs to know:
- The payload type (`WeatherPayload`, `NewsPayload`, etc.) — a JSON-serializable struct
- How to render it — entirely up to the slide's visual design

This decoupling means:

| Benefit | Explanation |
|---|---|
| **Multiple visual designs** | Any number of slide crates can depend on the same sidecar. A "flat weather" slide and a "radar weather" slide both use `lume-weather-sidecar`. |
| **Testable in isolation** | `brrmmmm run weather-sidecar.wasm --once` fetches real data and prints JSON. No VZGLYD engine needed. |
| **Portable** | The sidecar is a WASM binary with a well-defined ABI. Any project that implements `vzglyd_host` + WASI can run it — not just VZGLYD. |
| **Zero host-side parsing** | The host doesn't parse weather APIs, RSS feeds, or Last.fm responses. That logic lives in the sidecar where it belongs. |
| **Exponential backoff built in** | Network failures are handled automatically. The poll loop doubles its interval on error, capped at 60 seconds. |

### The Sidecar Pattern Beyond VZGLYD

While developed for VZGLYD slides, this pattern generalizes to any project that needs periodic external data:

```
your-project/
  ├── sidecar.wasm    ← fetches, parses, serializes (portable)
  └── your_runtime    ← provides vzglyd_host + WASI
```

A sidecar can serve data to:
- **Embedded dashboards** (any display system)
- **API aggregation layers** (multiple sources → unified payload)
- **Monitoring agents** (health checks, metrics collection)
- **Data pipelines** (ETL jobs packaged as WASM)

The `vzglyd_host` ABI is the integration contract. brrmmmm provides a native implementation; the VZGLYD native engine and web engine each provide their own.

## The vzglyd_host ABI

| Function | Signature | Purpose |
|---|---|---|
| `channel_push` | `fn(ptr: *const u8, len: i32) -> i32` | Sidecar pushes data payload to host |
| `channel_poll` | `fn(ptr: *mut u8, len: i32) -> i32` | Host delivers data back to sidecar |
| `channel_active` | `fn() -> i32` | Query whether consumer is listening |
| `log_info` | `fn(ptr: *const u8, len: i32) -> i32` | Sidecar logs to host |
| `network_request` | `fn(ptr: *const u8, len: i32) -> i32` | Submit host-mediated network request |
| `network_response_len` | `fn() -> i32` | Query pending response size |
| `network_response_read` | `fn(ptr: *mut u8, len: i32) -> i32` | Read response into sidecar buffer |
| `trace_span_start/end` | `fn(...)` → i32 | Distributed tracing spans |
| `trace_event` | `fn(ptr, len)` → i32 | Instant trace events |

Network requests use a JSON wire protocol:

```json
// Request (sidecar → host)
{"wire_version": 1, "kind": "https_get", "host": "api.example.com", "path": "/data", "headers": []}

// Response (host → sidecar)
{"wire_version": 1, "kind": "http", "status_code": 200, "headers": [], "body": [...]}
```

## Requirements

- Rust stable
- `wasm32-wasip1` target for building sidecars: `rustup target add wasm32-wasip1`

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
