# brrmmmm frontend protocol

The Rust CLI is the canonical runtime. Frontends, including the Ink TUI, talk to it through a small stdio protocol instead of scraping terminal output.

## Starting the backend

```bash
brrmmmm run sidecar.wasm --events [--env KEY=VALUE ...] [--params-json '{"key":"value"}']
```

In `--events` mode, stdout is newline-delimited JSON. Stderr is reserved for process-level failures. Frontends must treat every stdout line as one JSON event.

## Commands

Frontends send commands to backend stdin:

| Command | Meaning |
|---|---|
| `force` | Skip the current sleep and poll as soon as the sidecar reaches a host-controlled sleep point. |
| `params_json <json-object>` | Replace host-owned params and request a refresh. |

Invalid commands are ignored unless the backend emits a `log` event.

## Events

Events use the Rust `Event` enum shape in `src/events.rs`. Current frontends should handle at least:

| Event | Required frontend behavior |
|---|---|
| `env_snapshot` | Show which passed env vars are present. |
| `started` | Record WASM path, size, and ABI version. |
| `describe` | Render the sidecar contract, params, modes, artifacts, and polling strategy. |
| `phase` | Update canonical runtime phase. |
| `request_start` | Add or update the current network request. |
| `request_done` | Mark the request as completed. |
| `request_error` | Mark the request as failed; no matching `request_done` is emitted for transport failures. |
| `artifact_received` | Update artifact panes and published output state. |
| `sleep_start` | Start countdown to the next poll. |
| `log` | Append to the log strip. |
| `sidecar_exit` | Mark the backend as stopped. |

## Output contract

Application code should consume `published_output`.

- `brrmmmm run sidecar.wasm --once` prints only `published_output` bytes to stdout.
- `brrmmmm run sidecar.wasm --once --events` prints only NDJSON events to stdout; the payload is available inside the `artifact_received` event for `published_output`.
- `raw_source_payload` and `normalized_payload` are debugging artifacts, not the primary consumer contract.

## Runtime modes

| Mode | Developer expectation |
|---|---|
| `v1_legacy` | No reliable static manifest. Treat output as opaque JSON and validate in the consumer. |
| `managed_polling` | Sidecar declares params, artifacts, polling, cooldown, and capabilities. Use `inspect` as the contract. |
| `interactive` | Params can change while the sidecar is alive. Sidecars should read `params_len`/`params_read` each cycle. |

Future frontends, including a possible Ratatui frontend, should implement this protocol rather than bind to Ink internals.
