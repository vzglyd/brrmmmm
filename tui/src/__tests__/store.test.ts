import { describe, expect, it } from "vitest";
import { initialState, reducer } from "../store.js";

const TS = "2024-01-01T12:00:00Z";
const WASM = "test.wasm";

function makeState() {
  return initialState(WASM);
}

describe("initialState", () => {
  it("sets wasmPath and defaults", () => {
    const s = makeState();
    expect(s.wasmPath).toBe(WASM);
    expect(s.isRunning).toBe(true);
    expect(s.cycleCount).toBe(0);
    expect(s.artifacts.raw).toBeNull();
    expect(s.artifacts.normalized).toBeNull();
    expect(s.artifacts.published).toBeNull();
  });
});

describe("reducer", () => {
  it("phase event updates polling phase", () => {
    const s = reducer(makeState(), { type: "phase", ts: TS, phase: "fetching" });
    expect(s.polling.phase).toBe("fetching");
  });

  it("request_start creates a pending request", () => {
    const s = reducer(makeState(), {
      type: "request_start",
      ts: TS,
      request_id: "r1",
      kind: "http",
      host: "example.com",
    });
    expect(s.lastRequest?.pending).toBe(true);
    expect(s.lastRequest?.request_id).toBe("r1");
    expect(s.lastRequest?.sequence).toBe(1);
    expect(s.polling.phase).toBe("fetching");
  });

  it("request_done resolves the pending request", () => {
    const s0 = reducer(makeState(), {
      type: "request_start",
      ts: TS,
      request_id: "r1",
      kind: "http",
      host: "example.com",
    });
    const s1 = reducer(s0, {
      type: "request_done",
      ts: TS,
      request_id: "r1",
      status_code: 200,
      elapsed_ms: 123,
      response_size_bytes: 456,
    });
    expect(s1.lastRequest?.pending).toBe(false);
    expect(s1.lastRequest?.status_code).toBe(200);
    expect(s1.lastRequest?.elapsed_ms).toBe(123);
  });

  it("request_error marks request as failed", () => {
    const s0 = reducer(makeState(), {
      type: "request_start",
      ts: TS,
      request_id: "r1",
      kind: "http",
      host: "example.com",
    });
    const s1 = reducer(s0, {
      type: "request_error",
      ts: TS,
      request_id: "r1",
      error_kind: "timeout",
      message: "connection timed out",
    });
    expect(s1.lastRequest?.pending).toBe(false);
    expect(s1.lastRequest?.error).toBe("connection timed out");
    expect(s1.polling.phase).toBe("failed");
    expect(s1.polling.consecutiveFailures).toBe(1);
  });

  it("artifact_received sets raw artifact", () => {
    const s = reducer(makeState(), {
      type: "artifact_received",
      ts: TS,
      kind: "raw_source_payload",
      size_bytes: 100,
      preview: "{}",
      artifact: { kind: "raw_source_payload", size_bytes: 100, received_at_ms: 1000 },
    });
    expect(s.artifacts.raw?.kind).toBe("raw_source_payload");
    expect(s.artifacts.raw?.size_bytes).toBe(100);
  });

  it("artifact_received for published_output increments cycleCount", () => {
    const s = reducer(makeState(), {
      type: "artifact_received",
      ts: TS,
      kind: "published_output",
      size_bytes: 50,
      preview: "done",
      artifact: { kind: "published_output", size_bytes: 50, received_at_ms: 2000 },
    });
    expect(s.cycleCount).toBe(1);
    expect(s.artifacts.published?.kind).toBe("published_output");
  });

  it("log event appends formatted log entry", () => {
    const s = reducer(makeState(), { type: "log", ts: TS, message: "hello" });
    expect(s.logs).toHaveLength(1);
    expect(s.logs[0]).toContain("hello");
  });

  it("module_exit sets isRunning false", () => {
    const s = reducer(makeState(), { type: "module_exit", ts: TS, reason: "done" });
    expect(s.isRunning).toBe(false);
    expect(s.error).toContain("done");
  });

  it("sleep_start records sleep info", () => {
    const wake = new Date(Date.now() + 5000).toISOString();
    const s = reducer(makeState(), {
      type: "sleep_start",
      ts: TS,
      duration_ms: 5000,
      wake_at: wake,
    });
    expect(s.polling.backoffMs).toBe(5000);
    expect(s.polling.phase).toBe("idle");
    expect(s.polling.sleepUntilMs).toBeGreaterThan(0);
  });

  it("does not mutate original state", () => {
    const original = makeState();
    reducer(original, { type: "phase", ts: TS, phase: "fetching" });
    expect(original.polling.phase).toBe("idle");
  });

  it("requests list is capped at 8 entries", () => {
    let s = makeState();
    for (let i = 0; i < 10; i++) {
      s = reducer(s, {
        type: "request_start",
        ts: TS,
        request_id: `r${i}`,
        kind: "http",
        host: "example.com",
      });
    }
    expect(s.requests.length).toBeLessThanOrEqual(8);
  });
});
