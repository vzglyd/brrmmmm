import { type ChildProcess } from "node:child_process";
import { type BrrEvent } from "./types.js";
export type EventCallback = (event: BrrEvent) => void;
export type ExitCallback = (code: number | null) => void;
export interface StreamHandle {
    process: ChildProcess;
    stop: () => void;
}
/**
 * Spawn the Rust binary in --events mode and stream NDJSON events to `onEvent`.
 *
 * @param rustBin  Path to the brrmmmm Rust binary.
 * @param wasmPath Path to the .wasm sidecar file.
 * @param extraArgs Additional CLI args (e.g. --env KEY=VALUE).
 * @param onEvent  Called for each parsed NDJSON line.
 * @param onExit   Called when the subprocess exits.
 */
export declare function spawnEventStream(rustBin: string, wasmPath: string, extraArgs: string[], onEvent: EventCallback, onExit: ExitCallback): StreamHandle;
