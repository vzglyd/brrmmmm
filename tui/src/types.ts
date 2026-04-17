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

export type SidecarParamType = "string" | "integer" | "number" | "boolean" | "json";

export interface SidecarParamOption {
  value: unknown;
  label?: string;
}

export interface SidecarParamField {
  key: string;
  type: SidecarParamType;
  required: boolean;
  label?: string;
  help?: string;
  default?: unknown;
  options: SidecarParamOption[];
}

export interface SidecarParamsSchema {
  fields: SidecarParamField[];
}

export type PersistenceAuthority = "volatile" | "host_persisted" | "vendor_backed";

export type PollStrategy =
  | { kind: "fixed_interval"; interval_secs: number }
  | { kind: "exponential_backoff"; base_secs: number; max_secs: number }
  | { kind: "jittered"; base_secs: number; jitter_secs: number };

export interface SidecarDescribe {
  schema_version: number;
  logical_id: string;
  name: string;
  description: string;
  abi_version: number;
  run_modes: string[];
  state_persistence: PersistenceAuthority;
  required_env_vars: EnvVarSpec[];
  optional_env_vars: EnvVarSpec[];
  params?: SidecarParamsSchema | null;
  capabilities_needed: string[];
  poll_strategy?: PollStrategy;
  cooldown_policy?: { authority: PersistenceAuthority; min_interval_ms: number };
  artifact_types: string[];
}

export type SidecarPhase =
  | "idle"
  | "cooling_down"
  | "fetching"
  | "parsing"
  | "publishing"
  | "failed";

export type ActiveMode = "v1_legacy" | "managed_polling" | "interactive";

export interface SidecarRuntimeState {
  mode: ActiveMode;
  phase: SidecarPhase;
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
  describe?: SidecarDescribe;
}

// ── Event union ──────────────────────────────────────────────────────

export type BrrEvent =
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
      describe: SidecarDescribe;
    }
  | {
      type: "env_snapshot";
      ts: string;
      vars: EnvVarStatus[];
    }
  | {
      type: "phase";
      ts: string;
      phase: SidecarPhase;
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
  | { type: "log"; ts: string; message: string }
  | { type: "sidecar_exit"; ts: string; reason: string };

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
  abiVersion: number;
  describe: SidecarDescribe | null;
  envVars: EnvVarStatus[];
  // Combined env var list: spec (from describe) merged with snapshot (from --env args).
  mergedEnvVars: MergedEnvVar[];
  polling: {
    phase: SidecarPhase;
    sleepUntilMs: number | null;
    lastSuccessAt: string | null;
    consecutiveFailures: number;
    backoffMs: number | null;
  };
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
