import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
/**
 * Spawn the Rust binary in --events mode and stream NDJSON events to `onEvent`.
 *
 * @param rustBin  Path to the brrmmmm Rust binary.
 * @param wasmPath Path to the .wasm sidecar file.
 * @param extraArgs Additional CLI args (e.g. --env KEY=VALUE).
 * @param onEvent  Called for each parsed NDJSON line.
 * @param onExit   Called when the subprocess exits.
 */
export function spawnEventStream(rustBin, wasmPath, extraArgs, onEvent, onExit) {
    const child = spawn(rustBin, ["run", wasmPath, "--events", ...extraArgs], { stdio: ["ignore", "pipe", "pipe"] });
    const rl = createInterface({ input: child.stdout });
    rl.on("line", (line) => {
        if (!line.trim())
            return;
        try {
            const event = JSON.parse(line);
            onEvent(event);
        }
        catch {
            // Silently ignore malformed lines (should not happen in practice).
        }
    });
    // Swallow stderr — it's suppressed in --events mode, but be safe.
    child.stderr?.resume();
    child.on("exit", (code) => {
        rl.close();
        onExit(code);
    });
    return {
        process: child,
        stop: () => {
            child.kill("SIGTERM");
        },
    };
}
