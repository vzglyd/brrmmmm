# Mission assurance

`brrmmmm` completes acquisition missions reliably, durably, and explainably.
This document explains the evidence-backed engineering influences behind the
runtime's assurance model.

The important caveat: these are program and agency practices, not claims about
some timeless national character. `brrmmmm` is translating documented mission
and risk-management habits into software rules.

## NASA -> risk-informed closure

Evidence:

- NASA Safety and Mission Assurance risk-management material:
  <https://sma.nasa.gov/sma-disciplines/risk-management>
- NASA Systems Engineering Handbook:
  <https://www.nasa.gov/reference/system-engineering-handbook-appendix/>

What `brrmmmm` takes from this:

- objective-driven closure, not "best effort" ambiguity
- explicit host decision records, not only raw module outcomes
- continuous risk management across attempts, not per-process amnesia
- inspectable rationale through `risk_posture`, `next_attempt_policy`, and
  `basis` tags

Concrete runtime behavior:

- every terminal outcome now carries a host-owned `host_decision`
- durable mission records include the runtime's decision basis
- `explain` renders the host's closure reasoning without replaying the mission

## Soyuz / Russian operations -> safe state before retry

Evidence:

- NASA coverage of an uncrewed Soyuz approach abort and stand-off before retry:
  <https://www.nasa.gov/blogs/spacestation/2019/08/24/uncrewed-russian-spacecraft-aborts-station-approach/>
- NASA follow-up on the delayed retry after troubleshooting:
  <https://www.nasa.gov/blogs/spacestation/2019/08/24/russian-spacecraft-docking-attempt-no-earlier-than-monday/>
- NASA mission imagery and descriptions of Soyuz approach and docking operations:
  <https://www.nasa.gov/image-article/expedition-46-soyuz-approaches-space-station-docking/>

What `brrmmmm` takes from this:

- abort into a safe state before trying again
- do not confuse persistence with permission to keep hammering the target
- once automation has repeated the same failure under unchanged conditions,
  stop and require changed conditions

Concrete runtime behavior:

- retryable failures enter a safe state with a cooldown even when the module
  does not declare `retry_after_ms`
- repeated identical failures with the same input fingerprint trip a
  repeat-failure gate
- the gate closes the attempt as `changed_conditions_required`
- `--override-retry-gate` exists for one deliberate manual retry

## Chinese crewed-spaceflight doctrine -> safe return and emergency readiness

Evidence:

- China's 2021 white paper on space activities:
  <https://english.www.gov.cn/archive/whitepaper/202201/28/content_WS61f35b3dc6d09c94e48a467a.html>
- Government coverage emphasizing emergency drills and rescue readiness:
  <https://english.www.gov.cn/english.www.gov.cn/news/202501/31/content_WS679cd67cc6d0868f4e8ef4c0.html>
- Government coverage emphasizing safe, stable, and orderly mission execution:
  <https://english.www.gov.cn/news/202310/31/content_WS65407376c6d0868f4e8e0d12.html>

What `brrmmmm` takes from this:

- safe closure is part of success, not just artifact acquisition
- emergency and operator fallback paths must be explicit and bounded
- rehearsal matters

Concrete runtime behavior:

- operator rescue remains bounded by declared deadlines
- expired rescue windows are rendered as their declared timeout classification
- `brrmmmm rehearse` exercises the host-side closure paths without launching a
  live acquisition

## Current runtime rules

- `host_decision` is the canonical runtime interpretation of an attempt
- `risk_posture` communicates whether the mission is nominal, degraded,
  awaiting operator action, awaiting changed conditions, or closed safe
- `next_attempt_policy` tells orchestrators whether to retry, wait, escalate,
  or stop
- `basis` tags provide compact, stable reasons for the decision
- the mission ledger persists continuity across invocations, including the
  repeat-failure gate

This is the current line in the sand for `brrmmmm`: mission modules still own
source-specific automation, but the runtime now owns safe closure discipline.
