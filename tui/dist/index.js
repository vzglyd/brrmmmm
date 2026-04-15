#!/usr/bin/env node
import { jsx as _jsx } from "react/jsx-runtime";
/**
 * brrmmmm TUI entry point.
 *
 * Usage:
 *   brrmmmm sidecar.wasm [--env KEY=VALUE ...]   → Ink TUI (default)
 *   brrmmmm run sidecar.wasm --once              → pass-through to Rust binary
 *   brrmmmm inspect sidecar.wasm                 → print describe() JSON and exit
 *   brrmmmm validate sidecar.wasm                → pass-through to Rust binary
 */
import { render } from "ink";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, execSync } from "node:child_process";
import { existsSync } from "node:fs";
import { App } from "./app.js";
// ── Resolve Rust binary ──────────────────────────────────────────────
function findOnPath(name) {
    try {
        const result = execSync(`which ${name} 2>/dev/null`, { encoding: "utf8" }).trim();
        return result || null;
    }
    catch {
        return null;
    }
}
function findRustBin() {
    const __dir = dirname(fileURLToPath(import.meta.url));
    // Development: tui/dist/ → ../../target/release/brrmmmm
    const devPath = resolve(__dir, "..", "..", "target", "release", "brrmmmm");
    if (existsSync(devPath))
        return devPath;
    // Debug build fallback.
    const debugPath = resolve(__dir, "..", "..", "target", "debug", "brrmmmm");
    if (existsSync(debugPath))
        return debugPath;
    // Installed variants on PATH.
    for (const name of ["brrmmmm-rs", "brrmmmm-core"]) {
        const found = findOnPath(name);
        if (found)
            return found;
    }
    console.error("[brrmmmm-tui] Could not find the Rust backend binary.\n" +
        "  Expected one of:\n" +
        `    ${devPath}\n` +
        "    brrmmmm-rs (on PATH)\n" +
        "    brrmmmm-core (on PATH)\n" +
        "  Build the Rust binary with: cargo build --release");
    process.exit(1);
}
// ── Pass-through to Rust binary ──────────────────────────────────────
function passThrough(rustBin, args) {
    const child = spawn(rustBin, args, { stdio: "inherit" });
    child.on("exit", (code) => process.exit(code ?? 0));
}
// ── Main ─────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const firstArg = args[0] ?? "";
// Pass-through conditions:
const isPassThrough = args.includes("--once") ||
    args.includes("--events") ||
    firstArg === "validate" ||
    process.env["NO_TUI"] === "1";
const rustBin = findRustBin();
if (isPassThrough) {
    // Translate 'brrmmmm <wasm>' shorthand to 'brrmmmm run <wasm>' for the Rust binary.
    if (firstArg.endsWith(".wasm")) {
        passThrough(rustBin, ["run", ...args]);
    }
    else {
        passThrough(rustBin, args);
    }
}
else if (firstArg === "inspect") {
    passThrough(rustBin, args);
}
else {
    // TUI mode.
    // Accept: brrmmmm <wasm> [...flags]
    //         brrmmmm run <wasm> [...flags]
    let wasmPath;
    let extraArgs;
    if (firstArg === "run") {
        wasmPath = args[1] ?? "";
        extraArgs = args.slice(2);
    }
    else {
        wasmPath = firstArg;
        extraArgs = args.slice(1);
    }
    if (!wasmPath || !wasmPath.endsWith(".wasm")) {
        console.error("Usage: brrmmmm <sidecar.wasm> [--env KEY=VALUE ...]\n" +
            "       brrmmmm run <sidecar.wasm> --once\n" +
            "       brrmmmm inspect <sidecar.wasm>");
        process.exit(1);
    }
    const { waitUntilExit } = render(_jsx(App, { wasmPath: wasmPath, rustBin: rustBin, extraArgs: extraArgs }));
    await waitUntilExit();
}
