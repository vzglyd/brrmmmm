# brrmmmm

brrmmmm completes acquisition missions reliably, durably, and explainably.

## Legal / ethical use

`brrmmmm` can execute mission modules that automate network access, browser login
flows, session refresh, and CAPTCHA remediation. That capability does not grant
authorization, waive third-party Terms of Service, or determine whether a
workflow is lawful in your jurisdiction.

Legal compliance, contractual compliance, target-service authorization, and
operator review remain the responsibility of the module author and the party
running the mission. The project documents runtime behavior only; it does not
provide legal advice.

## What it is

`brrmmmm` is an acquisition runtime for portable `wasm32-wasip1` mission
modules.

One invocation is one acquisition mission:

1. Load a mission module.
2. Read its static contract.
3. Provide the declared host capabilities.
4. Run it until it reports a terminal mission outcome.
5. Persist a durable mission record that explains what happened.

The runtime is CLI-first on purpose. External orchestrators decide when to run a
mission. `brrmmmm` owns the inner loop required to complete it.

## Why not just use `wasmtime`?

`wasmtime` runs Wasm modules.

`brrmmmm` adds the acquisition-runtime contract around the engine:

- ABI negotiation and module validation
- host capabilities for network, browser, AI, KV, params, sleep, tracing, and User-Agent control
- hard acquisition budgets and payload limits
- durable mission records and persistent mission continuity state
- structured events for live observers
- operator-facing defaults such as `brrmmmm.toml` discovery and file-based result delivery

If all you need is “execute this Wasm module”, use `wasmtime`.

If you need a runtime that completes acquisition missions and leaves behind a
durable explanation, use `brrmmmm`.

## Operator model

The intended integration model is a process boundary:

```text
external orchestrator
    -> decides when to run
brrmmmm
    -> completes one acquisition mission
mission-module.wasm
    -> declares how completion works for one source
```

That process boundary is deliberate. Browser automation, AI requests, policy
enforcement, persistence, and Wasm execution all stay inside one small runtime
world.

When `--result-path` or `mission.result_path` is configured, `brrmmmm` writes a
single atomic JSON mission record. Anything that needs the result can watch or
poll that file. The caller does not need to parent the `brrmmmm` process.

## Quick start

Install:

```bash
cargo install --path .
```

Inspect and validate a mission module:

```bash
brrmmmm validate path/to/mission-module.wasm
brrmmmm inspect path/to/mission-module.wasm --output table
```

Run once and print the published payload to stdout:

```bash
brrmmmm run path/to/mission-module.wasm --once --output json
```

Run once and write a durable mission record instead:

```bash
brrmmmm run path/to/mission-module.wasm --once --result-path mission.json
brrmmmm explain mission.json
```

Use working-directory config discovery:

```toml
# ./brrmmmm.toml
[mission]
wasm = "mission-module.wasm"
result_path = "mission.json"

[mission.env]
API_TOKEN = "..."
```

Then:

```bash
brrmmmm
```

## Durable mission records

Mission records are fixed-schema JSON. They are not just payload dumps.

Each record includes:

- `module`: resolved module identity such as `logical_id`, `name`, `abi_version`, and `wasm_path`
- `outcome`: typed terminal outcome such as `published`, `retryable_failure`, `terminal_failure`, or `operator_action_required`
- `host_decision`: exit-code category plus whether the final outcome was host-synthesized
- `explanation`: summary, message, and next action
- `artifacts`: captured `raw_source`, `normalized`, and `published_output` payloads when present
- `timing`: start, finish, and elapsed time
- `stats`: consecutive failures and persisted cooldown/failure timestamps

`brrmmmm explain mission.json` renders that contract back into operator-facing
text without replaying the run.

## Mission module contract

The current ABI is v3.

A compliant mission module exports:

- `brrmmmm_module_abi_version() -> u32`
- `brrmmmm_module_describe_ptr() -> i32`
- `brrmmmm_module_describe_len() -> i32`
- `brrmmmm_module_start()`

The runtime exposes the `brrmmmm_host` import module.

Important imports:

- `artifact_publish(kind_ptr, kind_len, data_ptr, data_len)` for artifacts
- `mission_outcome_report(ptr, len)` for the final typed mission outcome
- `params_len()` and `params_read(ptr, len)` for host-owned params
- `host_call(...)`, `host_response_len()`, and `host_response_read(...)` for network, browser, and AI capabilities
- `kv_*` for persisted host-owned state
- `sleep_ms(duration_ms)` for managed backoff or cooldown behavior

`validate` rejects modules that do not export the v3 contract or do not import
`brrmmmm_host.mission_outcome_report`.

## Explainability and continuity

`brrmmmm` does not treat a mission as “some bytes appeared on stdout”.

It tracks:

- the module contract and declared capabilities
- structured runtime events in `--events` mode
- a typed terminal mission outcome
- a mission ledger keyed by logical mission ID plus module hash
- a durable mission record for each invocation

That is the core product direction: the runtime should understand mission
completion, continuity, and failure explanation instead of acting like a thin
Wasm launcher with extra imports.

## Build mission modules

Install the Wasm target:

```bash
rustup target add wasm32-wasip1
```

Useful host prerequisites:

- Chrome or Chromium for modules that declare the `browser` capability
- `ANTHROPIC_API_KEY` for modules that declare the `ai` capability

The binary crate is the primary integration surface. The Rust library exists so
the CLI, tests, and TUI share one runtime implementation, but the supported API
surface is intentionally narrow.
