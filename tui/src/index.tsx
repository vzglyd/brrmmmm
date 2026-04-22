#!/usr/bin/env node
/**
 * brrmmmm TUI entry point.
 *
 * Usage:
 *   brrmmmm                                      → Ink daemon dashboard
 *   brrmmmm mission.wasm [--env KEY=VALUE ...]   → Ink dashboard with launcher prefilled
 *   brrmmmm run mission.wasm --once              → pass-through to Rust binary
 *   brrmmmm inspect mission.wasm                 → print describe() JSON and exit
 *   brrmmmm validate mission.wasm                → pass-through to Rust binary
 */

import { render } from "ink";
import React from "react";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, execSync } from "node:child_process";
import { existsSync } from "node:fs";

import { App } from "./app.js";

// ── Resolve Rust binary ──────────────────────────────────────────────

function findOnPath(name: string): string | null {
  try {
    const result = execSync(`which ${name} 2>/dev/null`, { encoding: "utf8" }).trim();
    return result || null;
  } catch {
    return null;
  }
}

function findRustBin(): string {
  const __dir = dirname(fileURLToPath(import.meta.url));
  // Development: tui/dist/ → ../../target/release/brrmmmm
  const devPath = resolve(__dir, "..", "..", "target", "release", "brrmmmm");
  if (existsSync(devPath)) return devPath;

  // Debug build fallback.
  const debugPath = resolve(__dir, "..", "..", "target", "debug", "brrmmmm");
  if (existsSync(debugPath)) return debugPath;

  // Installed variants on PATH.
  for (const name of ["brrmmmm-rs", "brrmmmm-core"]) {
    const found = findOnPath(name);
    if (found) return found;
  }

  console.error(
    "[brrmmmm-tui] Could not find the Rust backend binary.\n" +
      "  Expected one of:\n" +
      `    ${devPath}\n` +
      "    brrmmmm-rs (on PATH)\n" +
      "    brrmmmm-core (on PATH)\n" +
      "  Build the Rust binary with: cargo build --release"
  );
  process.exit(1);
}

// ── Pass-through to Rust binary ──────────────────────────────────────

function passThrough(rustBin: string, args: string[]): void {
  const child = spawn(rustBin, args, { stdio: "inherit" });
  child.on("exit", (code) => process.exit(code ?? 0));
}

// ── Main ─────────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const firstArg = args[0] ?? "";

// Pass-through conditions:
const isPassThrough =
  args.includes("--once") ||
  args.includes("--events") ||
  firstArg === "validate" ||
  process.env["NO_TUI"] === "1";

const rustBin = findRustBin();

if (isPassThrough) {
  // Translate 'brrmmmm <wasm>' shorthand to 'brrmmmm run <wasm>' for the Rust binary.
  if (firstArg.endsWith(".wasm")) {
    passThrough(rustBin, ["run", ...args]);
  } else {
    passThrough(rustBin, args);
  }
} else if (firstArg === "inspect") {
  passThrough(rustBin, args);
} else {
  // TUI mode.
  // Accept: brrmmmm [...dashboard]
  //         brrmmmm <wasm> [...prefill flags]
  //         brrmmmm run <wasm> [...prefill flags]
  let wasmPath: string | undefined;
  let extraArgs: string[];

  if (firstArg === "run") {
    wasmPath = args[1] || undefined;
    extraArgs = args.slice(2);
  } else {
    wasmPath = firstArg.endsWith(".wasm") ? firstArg : undefined;
    extraArgs = args.slice(1);
  }

  const { waitUntilExit } = render(<App initialWasmPath={wasmPath} rustBin={rustBin} extraArgs={extraArgs} />);

  await waitUntilExit();
}
