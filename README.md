# brrmmmm

An acquisition runtime for portable WASM sidecars.

---

## What is brrmmmm?

`brrmmmm` runs a sidecar and does not stop until it has obtained **one coherent unit
of external data** — or has conclusively exhausted every strategy it knows.

Each invocation is a single **acquisition mission**. The sidecar encodes everything
about how to complete that mission: which endpoints to call, how to handle failures,
how to refresh expired credentials, how to navigate a login form, how to recover from
a CAPTCHA. brrmmmm provides the runtime capabilities to execute those strategies and
enforces the acquisition budget the sidecar declares.

The caller receives exactly one result: the data, or a structured account of what was
tried and why it failed.

---

## Two levels of orchestration

External orchestrators (Airflow, Dagu, cron) decide **when** to trigger an acquisition.
brrmmmm is the orchestrator for **completing** it.

```
external orchestrator
    ↓  decides when to run
  brrmmmm
    ↓  orchestrates the full mission to completion
  sidecar.wasm
    ↓  encodes what "completion" looks like for this data source
HTTP / browser / file systems
```

The external orchestrator never sees retries, auth flows, or browser interactions. It
triggers a job and gets back a result. brrmmmm handles everything in between.

---

## The sidecar contract

A sidecar defines how to obtain one unit of data from one specific source.

It must:

- attempt to obtain the data using whatever means are appropriate
- handle all recoverable failure modes internally:
  - transient network errors → retry with backoff
  - expired API tokens → refresh and retry
  - expired sessions → re-authenticate (including browser-based flows)
  - CAPTCHA or complex UI challenges → AI-assisted resolution
- complete within its declared acquisition budget
- publish exactly one result via `channel_push`

That result is either the data, or a structured failure:

```json
{ "ok": true,  "data": { ... } }
{ "ok": false, "error": { "kind": "service_unavailable", "message": "...", "attempts": 4 } }
```

The sidecar author decides which failure modes are worth encoding remediation for.
Anything not covered is reported as unrecoverable.

---

## Remediation model

The full hierarchy of what a sidecar can do when an acquisition fails:

| Failure | Remedy |
|---|---|
| Network timeout / transient 5xx | Retry with exponential backoff |
| Rate limited (429) | Wait for `Retry-After`, then retry |
| API token expired (401) | Call token refresh endpoint, retry |
| Session expired, login form required | Drive browser login flow, retry |
| MFA prompt | Read TOTP from params or derive from shared secret |
| CAPTCHA | Take screenshot, invoke AI vision model, submit solution |
| IP block / account suspended | Report unrecoverable failure |
| Service permanently gone | Report unrecoverable failure |

brrmmmm provides the host capabilities for each level: `network_request` for API
flows, `browser_*` imports for UI-driven flows, `ai_*` imports for interpretation,
and `kv_*` imports for persisting session state across runs.

---

## Execution model

Each invocation:

1. Loads the sidecar WASM
2. Negotiates ABI version and reads the sidecar's describe contract
3. Provides the capabilities the sidecar declared it needs
4. Runs the sidecar and waits for it to publish a result
5. Enforces the acquisition budget declared in the describe contract
6. Returns the result or a structured failure

There is no outer polling loop. External orchestrators decide when to run brrmmmm.
brrmmmm owns the inner loop — everything needed to complete a single mission.

---

## 30-second demo

```bash
# Clone and install (the demo sidecar WASM is pre-built in demos/)
git clone https://github.com/vzglyd/brrmmmm && cd brrmmmm
cargo install --path .
WASM=demos/demo_weather_sidecar.wasm

# Validate, inspect, one-shot fetch
brrmmmm validate $WASM
brrmmmm --output table inspect $WASM
brrmmmm --output json run $WASM --once
```

`brrmmmm run --once` prints the sidecar's published payload to stdout:

```json
{"condition":"partly cloudy","is_day":true,"location":"Berlin","ok":true,"temperature_c":14.2,"wind_speed_ms":3.1}
```

---

## Install

```bash
git clone https://github.com/vzglyd/brrmmmm && cd brrmmmm
cargo install --path .
```

Requires Rust stable. Verify with `brrmmmm --version`.

The repo includes a pre-built demo sidecar at `demos/demo_weather_sidecar.wasm` — no
additional toolchain is needed to run the demo.

To build sidecars from source:

```bash
rustup target add wasm32-wasip1
```

---

## Usage

```bash
# Validate — check the WASM loads and resolves all imports
brrmmmm validate sidecar.wasm

# Inspect — print the sidecar's self-described contract
brrmmmm inspect sidecar.wasm
brrmmmm --output table inspect sidecar.wasm

# Run — execute one acquisition mission and print the result to stdout
brrmmmm run sidecar.wasm --once

# Pass environment variables the sidecar expects
brrmmmm run sidecar.wasm --once \
  --env LASTFM_API_KEY=xxx \
  --env LASTFM_USERNAME=rodger

# Pass structured params to the sidecar's configure buffer
brrmmmm run sidecar.wasm --once \
  --params-json '{"location":"Daylesford, VIC"}'

# Or read params from a file
brrmmmm run sidecar.wasm --once --params-file sidecar-params.json

# Debug: log each channel_push to stderr
brrmmmm run sidecar.wasm --once --log-channel
```

All commands accept `--output json|text|table`. The default for `run` and `validate`
is `text`; for `inspect` it is `json`.

---

## TUI workbench

For interactive development, `brrmmmm` includes a TUI — a visual workbench for
observing a running sidecar in real time.

Invoke it by passing the WASM path directly, without a subcommand:

```bash
brrmmmm sidecar.wasm
```

The TUI shows the sidecar's lifecycle phase, published artifacts, network requests,
sleep announcements, and the full event stream as it happens. Build the Node.js
frontend first:

```bash
npm --prefix tui run build   # requires Node.js 20+
```

The TUI communicates with `brrmmmm` over the `--events` protocol: an NDJSON stream
on stdout carrying typed events (`started`, `describe`, `artifact_received`,
`request_start`, `request_done`, `sleep_start`, `log`, `sidecar_exit`, etc.).

---

## Sidecar ABI

The current ABI is version 1. A compliant sidecar exports:

| Export | Purpose |
|---|---|
| `vzglyd_sidecar_abi_version() -> u32` | Returns `1` — used by brrmmmm for version negotiation |
| `vzglyd_sidecar_describe_ptr() -> i32` | Pointer to a static JSON describe blob in WASM memory |
| `vzglyd_sidecar_describe_len() -> i32` | Byte length of the describe blob |
| `vzglyd_sidecar_start()` | Primary entry point (falls back to `_start` or `main`) |

The describe blob (`SidecarDescribe`) carries the sidecar's logical ID, name,
description, run modes, required/optional env vars, params schema, poll strategy,
cooldown policy, declared capabilities, acquisition budget, and artifact types.

`brrmmmm` refuses to load a sidecar that does not export `vzglyd_sidecar_abi_version`
or that returns an unrecognised version number.

---

## Artifact types

A sidecar may publish multiple named artifact kinds during a single mission:

| Kind | Purpose |
|---|---|
| `published_output` | The canonical result — what `run` prints to stdout |
| `raw_source_payload` | Debugging artifact: the unprocessed response from the source |
| `normalized_payload` | Debugging artifact: an intermediate parsed form |

The TUI displays all artifact types. `brrmmmm run` only surfaces `published_output`.

---

## Host ABI

The runtime exposes the `vzglyd_host` module to every sidecar:

| Function | Signature | Purpose |
|---|---|---|
| `channel_push` | `fn(ptr: i32, len: i32) -> i32` | Publish `published_output` result |
| `channel_poll` | `fn(ptr: i32, len: i32) -> i32` | (reserved; returns -1) |
| `channel_active` | `fn() -> i32` | Query whether a consumer is listening |
| `artifact_publish` | `fn(kind_ptr, kind_len, data_ptr, data_len) -> i32` | Publish a named artifact |
| `register_manifest` | `fn(ptr: i32, len: i32) -> i32` | Register the static describe blob |
| `params_len` | `fn() -> i32` | Query size of the params buffer |
| `params_read` | `fn(ptr: i32, len: i32) -> i32` | Read params JSON into sidecar memory |
| `sleep_ms` | `fn(duration_ms: i64) -> i32` | Sleep; host may return early on stop signal |
| `announce_sleep` | `fn(duration_ms: i64) -> i32` | Non-blocking: tell the host when next poll is planned |
| `network_request` | `fn(ptr: i32, len: i32) -> i32` | Submit a host-mediated network request |
| `network_response_len` | `fn() -> i32` | Query size of the pending response |
| `network_response_read` | `fn(ptr: i32, len: i32) -> i32` | Read the response into sidecar memory |
| `trace_span_start` | `fn(...) -> i32` | Start a distributed tracing span |
| `trace_span_end` | `fn(...) -> i32` | End a tracing span |
| `trace_event` | `fn(ptr: i32, len: i32) -> i32` | Emit an instant trace event |

Network requests use a JSON wire protocol:

```json
// Request (sidecar → host)
{"wire_version": 1, "kind": "https_get", "host": "api.example.com", "path": "/data", "headers": []}

// Response (host → sidecar)
{"wire_version": 1, "kind": "http", "status_code": 200, "headers": [], "body": [...]}
```

---

## Planned capabilities

The following host imports are designed and will be implemented to support the full
remediation model:

**`browser_*` — browser automation (implemented)**
brrmmmm uses existing browser automation tooling (headless Chrome via CDP) to execute
browser sessions on behalf of sidecars — it is not a browser automation framework
itself. Sidecars drive the session via a JSON action protocol (`navigate`, `fill`,
`click`, `press`, `wait_for_selector`, `wait_for_url`, `current_url`, `get_cookies`,
`get_text`, `get_html`, `evaluate_json`).
Enables login form flows, OAuth consent screens, and page scraping.
Declare `"capabilities_needed": ["browser"]`.

For local runner testing, set `BRRMMMM_BROWSER_HEADLESS=false` on the brrmmmm
process to launch Chrome with a visible window:

```sh
BRRMMMM_BROWSER_HEADLESS=false brrmmmm run demos/browser_login_fixture.wasm --once
```

**`ai_*` — AI model invocation**
Sidecars submit prompts (including images) to a host-managed language model. Enables
CAPTCHA resolution, interpretation of unstructured page content, and other tasks that
require visual or semantic understanding. The host owns the API key and model selection.
Declare `"capabilities_needed": ["ai"]`.

**`acquisition_timeout_secs` in describe**
Sidecars declare their expected acquisition budget. brrmmmm enforces it. The default
is 30 seconds; a sidecar that drives a browser login flow may declare 120 seconds.

---

## Why WASM sidecars?

**Why not containers?** Cold-start time, memory overhead, and orchestration complexity
are wrong for this use case. A WASM sidecar loads in under 5ms; a Docker container
for the same job is 10–100× heavier.

**Why not a native plugin?** Native plugins break ABI across OS versions, compiler
versions, and architectures. A WASM binary compiles once and runs identically on
Linux x86\_64, macOS ARM, and embedded targets.

**Why ~2MB? That seems large.** Each sidecar bundles its own DNS resolver (Google DoH),
TLS stack (rustls + rustls-rustcrypto), and HTTP/1.1 client — zero runtime dependencies
beyond WASI sockets. The host needs no TLS library, no API knowledge, no JSON parser.
The 2MB is a one-time cost that buys complete isolation.

**Why not have the host make HTTP calls?** The host would then need to know the API
endpoint, auth scheme, response schema, and parsing logic for every sidecar. The sidecar
author owns everything about the data source; the host author owns the runtime.
That separation is the entire point.

**Is the ABI stable?** `vzglyd_sidecar_abi_version()` returns the ABI version the
sidecar was compiled against. brrmmmm refuses to load mismatched versions. New ABI
features are additive.

---

## Design principles

- **Mission-oriented** — one invocation, one acquisition mission, seen through to completion
- **Synchronous** — the caller blocks until the mission succeeds or conclusively fails
- **Exhaustive remediation** — the sidecar encodes every recovery strategy it knows; brrmmmm executes them
- **Bounded** — the sidecar declares its acquisition budget; brrmmmm enforces it
- **Portable** — sidecars are `wasm32-wasip1` binaries; they run anywhere brrmmmm runs
- **Opaque internals** — how many retries, browser actions, or AI calls the sidecar makes is nobody else's business
- **Capability-declared** — sidecars declare which host capabilities they need; brrmmmm validates them at load time

---

## What brrmmmm is not

- Not an external workflow engine or scheduler (that's Dagu, Airflow, cron)
- Not a distributed system
- Not responsible for deciding when to trigger an acquisition

---

## Development

```bash
cargo test
npm --prefix tui run build
```

With Moonrepo:

```bash
moon run core:ci
# or without a global install:
npx --package @moonrepo/cli@2.2.1 moon run core:ci
```

---

## Requirements

- Rust stable
- `wasm32-wasip1` target for building sidecars: `rustup target add wasm32-wasip1`
- Node.js 20+ for the TUI frontend

---

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
