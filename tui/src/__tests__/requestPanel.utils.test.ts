import { describe, expect, it } from "vitest";
import {
  buildVScrollbar,
  clip,
  formatPollStrategy,
  formatRequestStatus,
} from "../utils/requestPanel.js";
import { type SidecarDescribe, type LastRequestView } from "../types.js";

describe("buildVScrollbar", () => {
  it("returns spaces when all content fits", () => {
    const bars = buildVScrollbar(0, 5, 10);
    expect(bars).toEqual(Array(10).fill(" "));
  });

  it("returns spaces for zero visible lines", () => {
    expect(buildVScrollbar(0, 10, 0)).toEqual([]);
  });

  it("produces thumb at top when scrollTop is 0", () => {
    const bars = buildVScrollbar(0, 20, 5);
    expect(bars[0]).toBe("█");
    expect(bars[bars.length - 1]).toBe("░");
  });

  it("produces thumb at bottom when scrolled to end", () => {
    const bars = buildVScrollbar(15, 20, 5);
    expect(bars[bars.length - 1]).toBe("█");
    expect(bars[0]).toBe("░");
  });

  it("returns correct length", () => {
    expect(buildVScrollbar(0, 100, 7)).toHaveLength(7);
  });
});

describe("clip", () => {
  it("passes through strings within limit", () => {
    expect(clip("hello", 10)).toBe("hello");
    expect(clip("hello", 5)).toBe("hello");
  });

  it("truncates with ellipsis", () => {
    expect(clip("hello world", 8)).toBe("hello...");
  });

  it("handles very short maxLength gracefully", () => {
    expect(clip("hello", 2)).toBe("he");
    expect(clip("hello", 3)).toBe("hel");
  });
});

describe("formatRequestStatus", () => {
  const base: LastRequestView = {
    sequence: 1,
    kind: "http",
    host: "example.com",
    request_id: "r1",
    pending: false,
  };

  it("returns 'pending' for pending requests", () => {
    expect(formatRequestStatus({ ...base, pending: true })).toBe("pending");
  });

  it("returns error string for errored requests", () => {
    expect(formatRequestStatus({ ...base, error: "timeout" })).toBe("ERR timeout");
  });

  it("formats successful response with code and elapsed", () => {
    const result = formatRequestStatus({ ...base, status_code: 200, elapsed_ms: 150 });
    expect(result).toBe("200 150ms");
  });

  it("includes size when present", () => {
    const result = formatRequestStatus({
      ...base,
      status_code: 200,
      elapsed_ms: 100,
      response_size_bytes: 2048,
    });
    expect(result).toContain("2.0KB");
  });

  it("uses ? when status code is missing", () => {
    const result = formatRequestStatus({ ...base });
    expect(result).toMatch(/^\?/);
  });
});

describe("formatPollStrategy", () => {
  const baseDescribe: SidecarDescribe = {
    schema_version: 1,
    logical_id: "test",
    name: "test",
    description: "",
    abi_version: 2,
    run_modes: [],
    state_persistence: "volatile",
    required_env_vars: [],
    optional_env_vars: [],
    capabilities_needed: [],
    artifact_types: [],
  };

  it("returns fallback when no poll_strategy", () => {
    expect(formatPollStrategy(baseDescribe)).toBe("freshness unspecified");
  });

  it("formats fixed_interval strategy", () => {
    const d = { ...baseDescribe, poll_strategy: { kind: "fixed_interval" as const, interval_secs: 60 } };
    expect(formatPollStrategy(d)).toBe("fresh every 1m");
  });

  it("formats exponential_backoff strategy", () => {
    const d = {
      ...baseDescribe,
      poll_strategy: { kind: "exponential_backoff" as const, base_secs: 5, max_secs: 60 },
    };
    expect(formatPollStrategy(d)).toBe("backoff 5s-1m");
  });

  it("formats jittered strategy", () => {
    const d = {
      ...baseDescribe,
      poll_strategy: { kind: "jittered" as const, base_secs: 30, jitter_secs: 10 },
    };
    expect(formatPollStrategy(d)).toBe("fresh every 30s + jitter");
  });
});
