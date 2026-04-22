import { resolve } from "node:path";
import { PassThrough, Writable } from "node:stream";
import React, { type ReactElement } from "react";
import { render } from "ink";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../stream.js", () => ({
  abortMission: vi.fn(),
  fetchMissionStatus: vi.fn().mockResolvedValue([]),
  holdMission: vi.fn(),
  inspectMission: vi.fn(),
  launchMission: vi.fn(),
  parseLaunchArgs: vi.fn(() => ({
    env: {},
    paramsSource: "none",
  })),
  rescueRetryMission: vi.fn(),
  resumeMission: vi.fn(),
  watchDaemonStatus: vi.fn(() => ({ stop: vi.fn() })),
  watchMission: vi.fn(() => ({ stop: vi.fn() })),
}));

import { App } from "../app.js";
import { HelpDialog } from "../components/HelpDialog.js";
import { RequestPanel } from "../components/RequestPanel.js";
import * as stream from "../stream.js";
import { type ArtifactView } from "../types.js";

class MemoryOutput extends Writable {
  columns = 120;
  rows = 40;
  isTTY = true;
  output = "";

  override _write(
    chunk: unknown,
    _encoding: BufferEncoding,
    callback: (error?: Error | null) => void,
  ): void {
    this.output += String(chunk);
    callback();
  }
}

class MemoryInput extends PassThrough {
  isTTY = true;
  isRaw = false;

  setRawMode(value: boolean): this {
    this.isRaw = value;
    return this;
  }

  ref(): this {
    return this;
  }

  unref(): this {
    return this;
  }
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(stream.fetchMissionStatus).mockResolvedValue([]);
  vi.mocked(stream.inspectMission).mockResolvedValue(null);
  vi.mocked(stream.launchMission).mockRejectedValue(new Error("launchMission not stubbed"));
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("terminal rendering", () => {
  it("renders help with output usage and runtime mode guidance", async () => {
    const output = await renderToText(<HelpDialog describe={null} height={24} />);

    expect(output).toContain("Use The Output");
    expect(output).toContain("published_output");
    expect(output).toContain("Runtime Modes");
    expect(output).toContain("managed_polling");
  });

  it("renders pipeline artifact state through Ink", async () => {
    const published: ArtifactView = {
      kind: "published_output",
      preview: "{\"ok\":true}",
      size_bytes: 11,
      received_at_ms: 0,
    };

    const output = await renderToText(
      <RequestPanel
        request={null}
        requests={[]}
        artifacts={{ raw: null, normalized: null, published }}
        describe={null}
        hasStarted={true}
        isFocused={true}
        height={8}
      />,
    );

    expect(output).toContain("COMMS");
    expect(output).toContain("published_output");
    expect(output).toContain("11B");
  });

  it("shows mission start placeholder before first mission event", async () => {
    const output = await renderToText(
      <RequestPanel
        request={null}
        requests={[]}
        artifacts={{ raw: null, normalized: null, published: null }}
        describe={null}
        hasStarted={false}
        isFocused={true}
        height={8}
      />,
    );

    expect(output).toContain("Waiting for mission start...");
  });

  it("renders the empty dashboard without entering a state update loop", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const output = await renderToText(
      <App initialWasmPath={undefined} rustBin="brrmmmm" extraArgs={[]} />,
      100,
    );

    expect(output).toContain("No missions running");
    expect(output).toContain("ADD MISSION");
    expect(
      errorSpy.mock.calls.some(([message]) =>
        String(message).includes("Maximum update depth exceeded"),
      ),
    ).toBe(false);
  });

  it("renders the arming panel with visible ARM and CANCEL buttons", async () => {
    const output = await renderToText(
      <App initialWasmPath="/tmp/demo.wasm" rustBin="brrmmmm" extraArgs={[]} />,
      50,
    );

    expect(output).toContain("Arm Mission");
    expect(output).toContain("ARM");
    expect(output).toContain("CANCEL");
  });

  it("auto-inspects the selected WASM and hydrates schema-backed fields", async () => {
    vi.mocked(stream.inspectMission).mockResolvedValue({
      schema_version: 1,
      logical_id: "brrmmmm.fixture.test",
      name: "Fixture",
      description: "Fixture mission",
      abi_version: 4,
      run_modes: ["managed_polling"],
      state_persistence: "volatile",
      required_env_vars: [],
      optional_env_vars: [],
      params: {
        fields: [
          {
            key: "location_name",
            type: "string",
            required: false,
            label: "Location name",
            help: "Shown in output",
            default: "Berlin",
            options: [],
          },
        ],
      },
      capabilities_needed: [],
      artifact_types: ["published_output"],
    });

    const output = await renderToText(
      <App
        initialWasmPath={resolve("..", "demos", "demo_weather_sidecar.wasm")}
        rustBin="brrmmmm"
        extraArgs={[]}
      />,
      250,
    );

    expect(output).toContain("Location name (string)");
    expect(output).toContain("Berlin");
  });

  it("keeps a launched mission selected while daemon status catches up", async () => {
    vi.mocked(stream.launchMission).mockResolvedValue("demo-mission");

    const session = renderToSession(
      <App
        initialWasmPath={resolve("..", "demos", "demo_weather_sidecar.wasm")}
        rustBin="brrmmmm"
        extraArgs={[]}
      />,
    );

    await session.wait(25);
    await pressKeys(session, ["\u001B[B", "\u001B[B", "\u001B[B", "\u001B[B", "\r"]);
    await session.wait(100);

    const output = session.read();

    session.instance.unmount();
    session.instance.cleanup();

    expect(output).toContain("demo-mission");
    expect(output).toContain("state: launching");
  });
});

async function renderToText(node: ReactElement, waitMs = 25): Promise<string> {
  const session = renderToSession(node);
  await session.wait(waitMs);
  session.instance.unmount();
  session.instance.cleanup();
  return session.read();
}

function renderToSession(node: ReactElement) {
  const stdout = new MemoryOutput();
  const stdin = new MemoryInput();
  const instance = render(node, {
    stdout: stdout as unknown as NodeJS.WriteStream,
    stdin: stdin as unknown as NodeJS.ReadStream,
    debug: true,
    exitOnCtrlC: false,
    patchConsole: false,
  });

  return {
    stdin,
    instance,
    read: () => stripAnsi(stdout.output),
    wait: (ms: number) =>
      new Promise<void>((resolve) => {
        setTimeout(resolve, ms);
      }),
  };
}

async function pressKeys(
  session: ReturnType<typeof renderToSession>,
  keys: string[],
  waitMs = 20,
): Promise<void> {
  for (const key of keys) {
    session.stdin.write(key);
    await session.wait(waitMs);
  }
}

function stripAnsi(value: string): string {
  return value.replace(/\u001B\[[0-9;?]*[ -/]*[@-~]/g, "");
}
