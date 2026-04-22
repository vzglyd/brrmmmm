import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { resolveLaunchWasmPath } from "../stream.js";

describe("stream launch path resolution", () => {
  it("resolves relative mission paths to absolute paths", () => {
    expect(resolveLaunchWasmPath("missions/demo-weather/demo.wasm")).toBe(
      resolve("missions/demo-weather/demo.wasm"),
    );
  });

  it("preserves absolute mission paths", () => {
    const absolute = resolve("/tmp/demo.wasm");
    expect(resolveLaunchWasmPath(absolute)).toBe(absolute);
  });
});
