import { PassThrough, Writable } from "node:stream";
import React, { type ReactElement } from "react";
import { render } from "ink";
import { describe, expect, it } from "vitest";

import { HelpDialog } from "../components/HelpDialog.js";
import { RequestPanel } from "../components/RequestPanel.js";
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
});

async function renderToText(node: ReactElement): Promise<string> {
  const stdout = new MemoryOutput();
  const stdin = new MemoryInput();
  const instance = render(node, {
    stdout: stdout as unknown as NodeJS.WriteStream,
    stdin: stdin as unknown as NodeJS.ReadStream,
    debug: true,
    exitOnCtrlC: false,
    patchConsole: false,
  });

  await new Promise((resolve) => {
    setTimeout(resolve, 25);
  });
  instance.unmount();
  instance.cleanup();

  return stripAnsi(stdout.output);
}

function stripAnsi(value: string): string {
  return value.replace(/\u001B\[[0-9;?]*[ -/]*[@-~]/g, "");
}
