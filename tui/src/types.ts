// TypeScript mirror of the Rust Event enum and supporting types.
// Keep in sync with src/events.rs in the Rust binary.

export interface ArtifactMeta {
  kind: string;
  size_bytes: number;
  received_at_ms: number;
}

export interface EnvVarStatus {
  name: string;
  required: boolean;
  set: boolean;
}

export interface EnvVarSpec {
  name: string;
  description: string;
}

export type ModuleParamType = "string" | "integer" | "number" | "boolean" | "json";

export interface ModuleParamOption {
  value: unknown;
  label?: string;
}

export interface ModuleParamField {
  key: string;
  type: ModuleParamType;
  required: boolean;
  label?: string;
  help?: string;
  default?: unknown;
  options: ModuleParamOption[];
}

export interface ModuleParamsSchema {
  fields: ModuleParamField[];
}

export type PersistenceAuthority = "volatile" | "host_persisted" | "vendor_backed";

export type PollStrategy =
  | { kind: "fixed_interval"; interval_secs: number }
  | { kind: "exponential_backoff"; base_secs: number; max_secs: number }
  | { kind: "jittered"; base_secs: number; jitter_secs: number };

export interface ModuleDescribe {
  schema_version: number;
  logical_id: string;
  name: string;
  description: string;
  abi_version: number;
  run_modes: string[];
  state_persistence: PersistenceAuthority;
  required_env_vars: EnvVarSpec[];
  optional_env_vars: EnvVarSpec[];
  params?: ModuleParamsSchema | null;
  capabilities_needed: string[];
  poll_strategy?: PollStrategy;
  cooldown_policy?: { authority: PersistenceAuthority; min_interval_ms: number };
  artifact_types: string[];
}

export type MissionSchedulerState =
  | "launching"
  | "running"
  | "scheduled"
  | "held"
  | "awaiting_change"
  | "awaiting_operator"
  | "terminal_failure"
  | "idle";

export interface DaemonMissionSummary {
  name: string;
  state: MissionSchedulerState;
  phase: string;
  cycles: number;
  wasm: string;
  held: boolean;
  terminal: boolean;
  pid?: number | null;
  last_started_at_ms?: number | null;
  last_run_at_ms?: number | null;
  last_outcome_status?: string | null;
  next_wake_at_ms?: number | null;
  last_error?: string | null;
}

export type MissionPhase =
  | "idle"
  | "cooling_down"
  | "fetching"
  | "parsing"
  | "publishing"
  | "failed";

export type ActiveMode = "v1_legacy" | "managed_polling" | "interactive";

export interface MissionRuntimeState {
  mode: ActiveMode;
  phase: MissionPhase;
  next_allowed_at_ms?: number;
  next_scheduled_poll_at_ms?: number;
  last_success_at_ms?: number;
  last_failure_at_ms?: number;
  cooldown_until_ms?: number;
  consecutive_failures: number;
  backoff_ms?: number;
  last_raw_artifact?: ArtifactMeta;
  last_output_artifact?: ArtifactMeta;
  last_error?: string;
  describe?: ModuleDescribe;
}

// ── Mission outcome view ─────────────────────────────────────────────

export interface MissionOutcomeView {
  status: string;
  reason_code: string;
  risk_posture: string;
  next_attempt_policy: string;
  basis: string[];
  operator_action?: string;
  escalation_deadline?: string;
  rescue_window_open?: boolean;
}

// ── Event union ──────────────────────────────────────────────────────

export type BrrmmmmEvent =
  | {
      type: "started";
      ts: string;
      wasm_path: string;
      wasm_size_bytes: number;
      abi_version: number;
    }
  | {
      type: "describe";
      ts: string;
      describe: ModuleDescribe;
    }
  | {
      type: "env_snapshot";
      ts: string;
      vars: EnvVarStatus[];
    }
  | {
      type: "phase";
      ts: string;
      phase: MissionPhase;
    }
  | {
      type: "guest_event_fwd";
      ts: string;
      guest_ts_ms: number;
      kind: string;
      attrs: Record<string, unknown>;
    }
  | {
      type: "artifact_received";
      ts: string;
      kind: string;
      size_bytes: number;
      preview: string;
      artifact: ArtifactMeta;
    }
  | {
      type: "request_start";
      ts: string;
      request_id: string;
      kind: string;
      host: string;
      path?: string;
    }
  | {
      type: "request_done";
      ts: string;
      request_id: string;
      status_code?: number;
      elapsed_ms: number;
      response_size_bytes: number;
    }
  | {
      type: "request_error";
      ts: string;
      request_id: string;
      error_kind: string;
      message: string;
    }
  | {
      type: "sleep_start";
      ts: string;
      duration_ms: number;
      wake_at: string;
    }
  | {
      type: "mission_outcome";
      ts: string;
      reported_by: string;
      outcome: {
        status: string;
        reason_code: string;
        message: string;
        retry_after_ms?: number;
        operator_action?: string;
      };
      host_decision: {
        risk_posture: string;
        next_attempt_policy: string;
        basis: string[];
        synthesized: boolean;
      };
      escalation?: {
        action: string;
        deadline_at: string;
        deadline_at_ms: number;
        timeout_outcome: string;
      };
    }
  | { type: "fatal_error"; ts: string; message: string }
  | { type: "log"; ts: string; message: string }
  | { type: "module_exit"; ts: string; reason: string };

// ── TUI state ────────────────────────────────────────────────────────

export interface ArtifactView {
  kind: string;
  preview: string;
  size_bytes: number;
  received_at_ms: number;
}

export interface LastRequestView {
  sequence: number;
  kind: string;
  host: string;
  path?: string;
  request_id: string;
  status_code?: number;
  elapsed_ms?: number;
  response_size_bytes?: number;
  error?: string;
  pending: boolean;
}

export interface TuiState {
  wasmPath: string;
  abiVersion: number | null;
  hasStarted: boolean;
  describe: ModuleDescribe | null;
  envVars: EnvVarStatus[];
  // Combined env var list: spec (from describe) merged with snapshot (from --env args).
  mergedEnvVars: MergedEnvVar[];
  polling: {
    phase: MissionPhase;
    sleepUntilMs: number | null;
    lastSuccessAt: string | null;
    consecutiveFailures: number;
    backoffMs: number | null;
  };
  missionOutcome: MissionOutcomeView | null;
  lastRequest: LastRequestView | null;
  requests: LastRequestView[];
  artifacts: {
    raw: ArtifactView | null;
    normalized: ArtifactView | null;
    published: ArtifactView | null;
  };
  cycleCount: number;
  logs: string[];
  isRunning: boolean;
  error: string | null;
}

export interface MergedEnvVar {
  name: string;
  required: boolean;
  description: string;
  set: boolean;
}
