import { describe, expect, it } from "vitest";
import { formatBytes, formatDuration, formatLocalTime } from "../format.js";

describe("formatLocalTime", () => {
  it("formats a millisecond timestamp to HH:MM:SS", () => {
    const date = new Date("2024-01-01T14:30:45Z");
    const result = formatLocalTime(date.getTime());
    expect(result).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("accepts a Date object", () => {
    const date = new Date("2024-01-01T14:30:45Z");
    expect(formatLocalTime(date)).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("accepts an ISO string", () => {
    expect(formatLocalTime("2024-01-01T14:30:45Z")).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("returns 'unknown' for invalid input", () => {
    expect(formatLocalTime("not-a-date")).toBe("unknown");
    expect(formatLocalTime(NaN)).toBe("unknown");
  });
});

describe("formatBytes", () => {
  it("formats bytes below 1024 with B suffix", () => {
    expect(formatBytes(0)).toBe("0B");
    expect(formatBytes(512)).toBe("512B");
    expect(formatBytes(1023)).toBe("1023B");
  });

  it("formats 1024+ bytes as KB", () => {
    expect(formatBytes(1024)).toBe("1.0KB");
    expect(formatBytes(1536)).toBe("1.5KB");
    expect(formatBytes(10240)).toBe("10.0KB");
  });
});

describe("formatDuration", () => {
  it("formats zero as 0s", () => {
    expect(formatDuration(0)).toBe("0s");
  });

  it("formats sub-minute durations in seconds", () => {
    expect(formatDuration(1000)).toBe("1s");
    expect(formatDuration(59000)).toBe("59s");
  });

  it("formats minute-range durations", () => {
    expect(formatDuration(60000)).toBe("1m");
    expect(formatDuration(90000)).toBe("1m 30s");
    expect(formatDuration(120000)).toBe("2m");
  });

  it("formats hour-range durations", () => {
    expect(formatDuration(3600000)).toBe("1h");
    expect(formatDuration(3660000)).toBe("1h 1m");
    expect(formatDuration(7200000)).toBe("2h");
  });

  it("treats negative input as zero", () => {
    expect(formatDuration(-5000)).toBe("0s");
  });
});
