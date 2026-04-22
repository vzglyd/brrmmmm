# Release checklist

This is the minimum release bar for `brrmmmm`.

## Automated gate

Preferred:

```bash
moon run core:ci
moon run core:docs
moon run core:release-dry-run
```

Fallback:

```bash
cargo check
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo doc --no-deps
npm --prefix tui run build
npm --prefix tui test
cargo package --allow-dirty
```

The automated gate must prove:

- the runtime compiles and packages cleanly
- docs build with `missing_docs` denied
- the deterministic mission-module fixture builds
- `validate`, `inspect`, `run --once`, `run --once --events`, and `explain` work against fixtures
- `rehearse` works against at least one fixture with operator fallback
- the TUI build and tests pass

## Runtime invariants

Do not release unless tests cover:

- distinct handling for missing versus corrupted persisted state
- atomic writes for runtime state, mission ledgers, and mission records
- bounded params, artifacts, KV state, and host payloads
- validated runtime phase transitions
- deterministic config/input exit behavior
- v4 ABI validation, including `brrmmmm_module_start`, `brrmmmm_host.mission_outcome_report`, and bounded operator fallback validation
- durable mission-record generation for success, timeout, and failure paths
- host decision rendering with `risk_posture`, `next_attempt_policy`, and `basis`
- repeat-failure gating for unchanged inputs, including `--override-retry-gate`

## Manual real-mission gate

Choose one real mission module representative of production use.

Run:

```bash
brrmmmm validate path/to/mission-module.wasm
brrmmmm inspect path/to/mission-module.wasm > inspect.json
brrmmmm run path/to/mission-module.wasm --once > payload.json
brrmmmm run path/to/mission-module.wasm --once --result-path mission.json
brrmmmm run path/to/mission-module.wasm --once --events > events.ndjson
brrmmmm explain mission.json
brrmmmm path/to/mission-module.wasm
```

Accept the release only if:

- `inspect.json` contains the real contract, including host imports and artifacts
- `payload.json` is the intended consumer payload
- `mission.json` is a valid schema-v1 mission record with `job`, `attempt`, `timeline`, `payload`, `host_decision.risk_posture`, `host_decision.next_attempt_policy`, and `host_decision.basis`
- `events.ndjson` is valid NDJSON with no raw payload line mixed into the stream
- `brrmmmm explain mission.json` gives the next operator action or expired rescue classification without replaying the mission
- `brrmmmm rehearse path/to/mission-module.wasm` renders the expected host-side closure drills
- the TUI surfaces `published_output`, params, mission outcome, and logs coherently
