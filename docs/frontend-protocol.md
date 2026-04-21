# brrmmmm frontend protocol

The Rust CLI is the canonical runtime. Frontends talk to it over stdio rather
than scraping terminal output.

## Starting the backend

```bash
brrmmmm run mission-module.wasm --once --events [--env KEY=VALUE ...] [--params-json '{"key":"value"}']
```

In `--events` mode, stdout is NDJSON. Stderr is reserved for process-level
failures.

## Commands

Frontends may write commands to stdin:

| Command | Meaning |
|---|---|
| `force` | Skip the current host-controlled sleep and continue as soon as the runtime reaches the next sleep point. |
| `params_json <json-object>` | Replace host-owned params and request a refresh. |

Invalid commands are ignored unless the backend emits a `log` event.

## Events

Frontends should handle at least these event types:

| Event | Meaning |
|---|---|
| `env_snapshot` | Which configured env vars are present. |
| `started` | Loaded module path, size, and ABI version. |
| `describe` | Static mission-module contract. |
| `phase` | Canonical runtime phase update. |
| `request_start` / `request_done` / `request_error` | Network request lifecycle. |
| `artifact_received` | New `raw_source_payload`, `normalized_payload`, or `published_output`. |
| `sleep_start` | Runtime entered a managed cooldown or backoff. |
| `mission_outcome` | Final typed mission outcome plus the runtime-owned `host_decision`, including bounded operator-rescue metadata when the mission escalates to a human. |
| `module_exit` | Wasm execution terminated. |
| `log` | Runtime or mission-module log line. |

## Output contract

Application code should treat `published_output` as the primary payload.

- `brrmmmm run mission-module.wasm --once` prints only `published_output` bytes to stdout when no result file is configured.
- `brrmmmm run mission-module.wasm --once --result-path mission.json` writes a durable mission record and keeps stdout empty.
- `brrmmmm run mission-module.wasm --once --events` prints only NDJSON events to stdout.
- `brrmmmm run mission-module.wasm --once --events --result-path mission.json` keeps stdout as NDJSON and still writes the mission record file.

`raw_source_payload` and `normalized_payload` are diagnostic artifacts, not the
primary consumer contract.

## Runtime modes

| Mode | Meaning |
|---|---|
| `managed_polling` | The module declares params, artifacts, cooldown policy, operator fallback, and capabilities; `inspect` is the contract source. |
| `interactive` | Params may change while the module is alive; the module should re-read `params_len` / `params_read`. |
