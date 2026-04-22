# brrmmmm

brrmmmm completes acquisition missions reliably, durably, and explainably.

## What it is

`brrmmmm` is an acquisition runtime for portable `wasm32-wasip1` mission
modules.

This is not for “hit an endpoint and grab a file”.

This is for hostile acquisition conditions: brittle browser flows, expiring
sessions, bot defenses, partial data, retries, cooldowns, and bounded human
rescue when automation runs out of road. Mission authors should think less
“bike ride to the shops” and more “Mars landing”: the target matters, the
environment fights back, and the runtime exists to give the mission every
legitimate chance to finish.

One invocation is one acquisition mission:

1. Load a mission module.
2. Read its static contract.
3. Provide the declared host capabilities.
4. Run it until it reports a terminal mission outcome.
5. Persist a durable mission record that explains what happened.

The runtime is CLI-first on purpose. External orchestrators decide when to run a
mission. `brrmmmm` owns the inner loop required to complete it.

## Mission doctrine

`brrmmmm` is designed around four promises:

- exhaust declared automation before conceding the mission
- persist continuity so retries, cooldowns, and prior outcomes survive process boundaries
- escalate to humans only as a bounded rescue path, never as an indefinite hang
- leave behind a durable explanation of what happened, what blocked progress, and what should happen next

“Whatever it takes” does not mean unlimited waiting or policy bypass. It means
using every declared capability, every allowed retry, and every bounded rescue
path before the attempt is closed.

## Mission assurance

The runtime now carries an explicit mission-assurance model instead of leaving
all closure semantics inside the Wasm module.

- `host_decision` records the runtime's risk posture, next-attempt policy, and basis tags
- retryable failures automatically enter a safe state with a default cooldown when the module does not provide one
- repeated identical failures with unchanged inputs trip a repeat-failure gate and close as `changed_conditions_required`
- `brrmmmm rehearse mission-module.wasm` exercises the runtime's host-side closure paths without launching a live acquisition

The design is evidence-backed rather than romanticized. `brrmmmm` is borrowing
program practices from NASA risk management, Soyuz safe-state operations, and
Chinese crewed-spaceflight emergency-readiness doctrine, then translating them
into software rules. See [docs/mission-assurance.md](docs/mission-assurance.md).

## Legal / ethical use

`brrmmmm` can execute mission modules that automate network access, browser login
flows, session refresh, and CAPTCHA remediation. That capability does not grant
authorization, waive third-party Terms of Service, or determine whether a
workflow is lawful in your jurisdiction.

Legal compliance, contractual compliance, target-service authorization, and
operator review remain the responsibility of the module author and the party
running the mission. The project documents runtime behavior only; it does not
provide legal advice.

## Why not just use `wasmtime`?

`wasmtime` runs Wasm modules.

`brrmmmm` adds the acquisition-runtime contract around the engine:

- ABI negotiation and module validation
- host capabilities for network, browser, AI, KV, params, sleep, tracing, and User-Agent control
- hard acquisition budgets and payload limits
- durable mission records and persistent mission continuity state
- bounded operator-rescue contracts with explicit expiry semantics
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
single atomic JSON mission result. Anything that needs the result should watch
or poll that file; it should not parse the TUI or embed the runtime as a
library.

Daemon-launched missions persist live and finalized JSON files at:

```text
~/.brrmmmm/missions/<mission_name>/<mission_name>.status.json
~/.brrmmmm/missions/<mission_name>/<mission_name>.out.json
```

Downstream consumers should watch `.status.json` for progress and read the
latest finalized payload from `.out.json`.

## Quick start

Install:

```bash
cargo install --path .
```

Inspect and validate a mission module:

```bash
brrmmmm validate path/to/mission-module.wasm
brrmmmm inspect path/to/mission-module.wasm --output table
brrmmmm rehearse path/to/mission-module.wasm --output json
```

Run once and print the published payload to stdout:

```bash
brrmmmm run path/to/mission-module.wasm --once --output json
```

Stdout mode is mainly for debugging and ad hoc inspection.

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

[assurance]
same_reason_retry_limit = 3
default_retry_after_ms = 300000

[mission.env]
API_TOKEN = "..."
```

Then:

```bash
brrmmmm
```

## Durable mission records

Mission records are fixed-schema JSON. They are not just payload dumps.

Each finalized `.out.json` record includes:

- `module`: resolved module identity such as `logical_id`, `name`, `abi_version`, and `wasm_path`
- `job`: runner mode, daemon mission name when one exists, scheduler state, cycle count, and canonical file paths
- `mission`: declared mission contract summary, including params, artifacts, env vars, and capabilities
- `attempt`: the latest finalized attempt sequence and timestamps
- `timeline`: a curated ledger of scheduler states, phases, interventions, and outcome milestones
- `challenges`: normalized obstacles observed during the latest attempt
- `interventions`: operator or daemon control actions such as hold, abort, or rescue retry
- `payload`: typed final payload envelope for downstream consumers
- `outcome`: typed terminal outcome such as `published`, `retryable_failure`, `terminal_failure`, or `operator_action_required`
- `host_decision`: exit-code category, risk posture, next-attempt policy, basis tags, and whether the final outcome was host-synthesized
- `explanation`: summary, message, and next action
- `escalation`: bounded operator rescue details such as `action`, `deadline_at`, and `timeout_outcome`
- `artifacts`: captured `raw_source`, `normalized`, and `published_output` payloads when present for diagnostics and secondary consumers
- `timing`: start, finish, and elapsed time
- `stats`: consecutive failures and persisted cooldown/failure timestamps

The live `.status.json` file uses the same job/mission/attempt/timeline
concepts, but reflects in-progress state instead of a finalized terminal
record.

`brrmmmm explain mission.json` renders that contract back into operator-facing
text without replaying the run. If a mission ended in `operator_action_required`,
`explain` is time-aware: it tells you whether the rescue window is still open or
whether the attempt has already closed as its declared timeout outcome. It also
surfaces the runtime's `risk_posture`, `next_attempt_policy`, and decision
`basis`.

## Mission module contract

The current ABI is v4.

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

Mission modules that may require human rescue declare:

- `describe.operator_fallback.timeout_ms`
- `describe.operator_fallback.on_timeout`

An `operator_action_required` outcome may tighten that rescue contract for one
attempt by setting:

- `operator_timeout_ms`
- `operator_timeout_outcome`

`validate` rejects modules that do not export the v4 contract, do not import
`brrmmmm_host.mission_outcome_report`, or declare an unbounded operator rescue
window.

## Explainability and continuity

`brrmmmm` does not treat a mission as “some bytes appeared on stdout”.

It tracks:

- the module contract and declared capabilities
- structured runtime events in `--events` mode
- a typed terminal mission outcome
- a runtime-owned host decision with risk posture and next-attempt policy
- an active bounded rescue window when operator intervention is required
- a mission ledger keyed by logical mission ID plus module hash
- an input fingerprint plus repeat-failure gate for unchanged conditions
- a durable mission record for each invocation

That is the core product direction: the runtime should understand mission
completion, continuity, safe closure, rescue expiry, and failure explanation
instead of acting like a thin Wasm launcher with extra imports.

## Build mission modules

Install the Wasm target:

```bash
rustup target add wasm32-wasip1
```

Useful host prerequisites:

- Chrome or Chromium for modules that declare the `browser` capability
- `ANTHROPIC_API_KEY` for modules that declare the `ai` capability

The `brrmmmm` binary is the supported integration surface. The Rust crate
primarily exists so the CLI, tests, and TUI share one runtime implementation;
downstream programs should run the binary and watch mission record files rather
than embedding the runtime.
