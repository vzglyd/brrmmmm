# Release checklist

This checklist defines "done enough to release" for brrmmmm.

## Automated gate

Run:

```bash
moon run core:ci
moon run core:release-dry-run
```

If Moon is not installed globally, use:

```bash
npx --package @moonrepo/cli@2.2.1 moon run core:ci
npx --package @moonrepo/cli@2.2.1 moon run core:release-dry-run
```

If Moon is unavailable on a machine, run the underlying commands:

```bash
cargo check
cargo test
npm --prefix tui run build
cargo package --allow-dirty
```

The automated gate must prove:

- The Rust CLI compiles.
- The deterministic WASM fixture builds.
- `validate`, `inspect`, `run --once`, and `run --once --events` work against the fixture.
- The Ink TUI TypeScript build passes.
- Cargo packaging can be dry-run locally.

## Manual real-sidecar gate

Choose one real sidecar that represents normal production use. Do not put this in CI unless its secrets, network dependencies, and vendor uptime are stable.

Run:

```bash
brrmmmm validate path/to/sidecar.wasm
brrmmmm inspect path/to/sidecar.wasm > inspect.json
brrmmmm run path/to/sidecar.wasm --once > payload.json
brrmmmm run path/to/sidecar.wasm --once --events > events.ndjson
brrmmmm path/to/sidecar.wasm
```

Accept the release only if:

- `inspect.json` contains the real contract: modes, params, artifacts, polling, cooldown, and env vars.
- `payload.json` is exactly the JSON the consumer should parse.
- `events.ndjson` contains valid NDJSON with no raw payload line mixed into the stream.
- The TUI explains how to consume `published_output`, lets params be edited, shows local clock time, and keeps the pipeline scrollable.
- Failures point to the next developer action.

## Release shape

- Rust remains the canonical runtime and CLI.
- The TypeScript Ink app remains the public TUI frontend for this release.
- Moonrepo is the repo-level task manager across Rust, Node, and fixture builds.
- Ratatui remains a future option, not a release blocker.
