// ── Initial state ────────────────────────────────────────────────────
export function initialState(wasmPath) {
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
        lastRequest: null,
        artifacts: { raw: null, normalized: null, published: null },
        cycleCount: 0,
        logs: [],
        isRunning: true,
        error: null,
    };
}
// ── Reducer ──────────────────────────────────────────────────────────
export function reducer(state, event) {
    switch (event.type) {
        case "started":
            return {
                ...state,
                abiVersion: event.abi_version,
                wasmPath: event.wasm_path,
            };
        case "describe": {
            const d = event.describe;
            const merged = buildMergedEnvVars([...d.required_env_vars.map((v) => ({ ...v, required: true })),
                ...d.optional_env_vars.map((v) => ({ ...v, required: false }))], state.envVars);
            return { ...state, describe: d, mergedEnvVars: merged };
        }
        case "env_snapshot": {
            const merged = buildMergedEnvVars(state.describe
                ? [
                    ...state.describe.required_env_vars.map((v) => ({ ...v, required: true })),
                    ...state.describe.optional_env_vars.map((v) => ({ ...v, required: false })),
                ]
                : [], event.vars);
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
            }
            else if (event.kind === "normalized_payload") {
                artifacts.normalized = view;
            }
            else {
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
            return {
                ...state,
                polling: { ...state.polling, phase: "fetching" },
                lastRequest: {
                    kind: event.kind,
                    host: event.host,
                    path: event.path,
                    request_id: event.request_id,
                    pending: true,
                },
            };
        case "request_done": {
            const req = state.lastRequest;
            return {
                ...state,
                lastRequest: req
                    ? {
                        ...req,
                        status_code: event.status_code,
                        elapsed_ms: event.elapsed_ms,
                        response_size_bytes: event.response_size_bytes,
                        pending: false,
                    }
                    : null,
            };
        }
        case "request_error": {
            const req = state.lastRequest;
            return {
                ...state,
                polling: {
                    ...state.polling,
                    phase: "failed",
                    consecutiveFailures: state.polling.consecutiveFailures + 1,
                },
                lastRequest: req
                    ? { ...req, error: event.message, pending: false }
                    : null,
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
        case "log": {
            const logs = [...state.logs, `${event.ts.slice(11, 19)} ${event.message}`];
            return { ...state, logs: logs.slice(-50) };
        }
        case "sidecar_exit":
            return { ...state, isRunning: false, error: `Sidecar exited: ${event.reason}` };
        default:
            return state;
    }
}
// ── Helpers ──────────────────────────────────────────────────────────
function buildMergedEnvVars(specs, statuses) {
    const statusMap = new Map(statuses.map((s) => [s.name, s.set]));
    const specNames = new Set(specs.map((s) => s.name));
    const fromSpecs = specs.map((s) => ({
        name: s.name,
        required: s.required,
        description: s.description,
        set: statusMap.get(s.name) ?? false,
    }));
    // Include vars passed via --env that are not in the spec (v1 mode).
    const extra = statuses
        .filter((s) => !specNames.has(s.name))
        .map((s) => ({
        name: s.name,
        required: false,
        description: "",
        set: s.set,
    }));
    return [...fromSpecs, ...extra];
}
