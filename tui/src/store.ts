import {
  type BrrmmmmEvent,
  type MergedEnvVar,
  type MissionOutcomeView,
  type TuiState,
} from "./types.js";
import { formatLocalTime } from "./format.js";

// ── Initial state ────────────────────────────────────────────────────

export function initialState(wasmPath: string): TuiState {
  return {
    wasmPath,
    abiVersion: 1,
    describe: null,
    envVars: [],
    mergedEnvVars: [],
    polling: {
      phase: "idle",
      sleepUntilMs: null,
      lastSuccessAt: null,
      consecutiveFailures: 0,
      backoffMs: null,
    },
    missionOutcome: null,
    lastRequest: null,
    requests: [],
    artifacts: { raw: null, normalized: null, published: null },
    cycleCount: 0,
    logs: [],
    isRunning: true,
    error: null,
  };
}

// ── Reducer ──────────────────────────────────────────────────────────

export function reducer(state: TuiState, event: BrrmmmmEvent): TuiState {
  switch (event.type) {
    case "started":
      return {
        ...state,
        abiVersion: event.abi_version,
        wasmPath: event.wasm_path,
      };

    case "describe": {
      const d = event.describe;
      const merged = buildMergedEnvVars(
        [...d.required_env_vars.map((v) => ({ ...v, required: true })),
         ...d.optional_env_vars.map((v) => ({ ...v, required: false }))],
        state.envVars
      );
      return { ...state, describe: d, mergedEnvVars: merged };
    }

    case "env_snapshot": {
      const merged = buildMergedEnvVars(
        state.describe
          ? [
              ...state.describe.required_env_vars.map((v) => ({ ...v, required: true })),
              ...state.describe.optional_env_vars.map((v) => ({ ...v, required: false })),
            ]
          : [],
        event.vars
      );
      return { ...state, envVars: event.vars, mergedEnvVars: merged };
    }

    case "phase":
      return {
        ...state,
        polling: { ...state.polling, phase: event.phase },
      };

    case "artifact_received": {
      const view = {
        kind: event.kind,
        preview: event.preview,
        size_bytes: event.size_bytes,
        received_at_ms: event.artifact.received_at_ms,
      };
      const artifacts = { ...state.artifacts };
      if (event.kind === "raw_source_payload") {
        artifacts.raw = view;
      } else if (event.kind === "normalized_payload") {
        artifacts.normalized = view;
      } else {
        // "published_output" + v1 channel_push
        artifacts.published = view;
      }
      const isPublished = event.kind === "published_output";
      // A successful push resets failures and records success time.
      return {
        ...state,
        artifacts,
        cycleCount: isPublished ? state.cycleCount + 1 : state.cycleCount,
        polling: {
          ...state.polling,
          phase: isPublished ? "publishing" : state.polling.phase,
          lastSuccessAt: event.ts,
          consecutiveFailures: 0,
          backoffMs: null,
        },
      };
    }

    case "request_start":
      const previousRequest = state.requests[state.requests.length - 1];
      const request = {
        sequence: previousRequest ? previousRequest.sequence + 1 : 1,
        kind: event.kind,
        host: event.host,
        path: event.path,
        request_id: event.request_id,
        pending: true,
      };
      return {
        ...state,
        polling: { ...state.polling, phase: "fetching" },
        lastRequest: request,
        requests: [...state.requests, request].slice(-8),
      };

    case "request_done": {
      const updateRequest = (req: typeof state.requests[number]) =>
        req.request_id === event.request_id
          ? {
              ...req,
              status_code: event.status_code,
              elapsed_ms: event.elapsed_ms,
              response_size_bytes: event.response_size_bytes,
              pending: false,
            }
          : req;
      const requests = state.requests.map(updateRequest);
      const req = state.lastRequest ? updateRequest(state.lastRequest) : null;
      return {
        ...state,
        lastRequest: req,
        requests,
      };
    }

    case "request_error": {
      const updateRequest = (req: typeof state.requests[number]) =>
        req.request_id === event.request_id
          ? { ...req, error: event.message, pending: false }
          : req;
      const requests = state.requests.map(updateRequest);
      const req = state.lastRequest ? updateRequest(state.lastRequest) : null;
      return {
        ...state,
        polling: {
          ...state.polling,
          phase: "failed",
          consecutiveFailures: state.polling.consecutiveFailures + 1,
        },
        lastRequest: req,
        requests,
      };
    }

    case "sleep_start": {
      const wakeMs = Date.parse(event.wake_at);
      return {
        ...state,
        polling: {
          ...state.polling,
          phase: "idle",
          sleepUntilMs: isNaN(wakeMs) ? null : wakeMs,
          backoffMs: event.duration_ms,
        },
      };
    }

    case "mission_outcome": {
      const escalation = event.escalation;
      const rescueWindowOpen = escalation
        ? Date.now() <= escalation.deadline_at_ms
        : undefined;
      const outcome: MissionOutcomeView = {
        status: event.outcome.status,
        reason_code: event.outcome.reason_code,
        risk_posture: event.host_decision.risk_posture,
        next_attempt_policy: event.host_decision.next_attempt_policy,
        basis: event.host_decision.basis,
        operator_action: event.outcome.operator_action,
        escalation_deadline: escalation?.deadline_at,
        rescue_window_open: rescueWindowOpen,
      };
      return { ...state, missionOutcome: outcome };
    }

    case "log": {
      const logs = [...state.logs, `${formatLocalTime(event.ts)} ${event.message}`];
      return { ...state, logs: logs.slice(-50) };
    }

    case "module_exit":
      return { ...state, isRunning: false, error: `Mission module exited: ${event.reason}` };

    default:
      return state;
  }
}

// ── Helpers ──────────────────────────────────────────────────────────

function buildMergedEnvVars(
  specs: Array<{ name: string; description: string; required: boolean }>,
  statuses: Array<{ name: string; set: boolean }>
): MergedEnvVar[] {
  const statusMap = new Map(statuses.map((s) => [s.name, s.set]));
  const specNames = new Set(specs.map((s) => s.name));

  const fromSpecs: MergedEnvVar[] = specs.map((s) => ({
    name: s.name,
    required: s.required,
    description: s.description,
    set: statusMap.get(s.name) ?? false,
  }));

  // Include vars passed via --env that are not in the spec (v1 mode).
  const extra: MergedEnvVar[] = statuses
    .filter((s) => !specNames.has(s.name))
    .map((s) => ({
      name: s.name,
      required: false,
      description: "",
      set: s.set,
    }));

  return [...fromSpecs, ...extra];
}
