import { type BrrEvent } from "../types.js";
/**
 * Spawn the Rust binary event stream and dispatch events.
 * Cleans up (kills the process) when the component unmounts.
 */
export declare function useEventStream(rustBin: string, wasmPath: string, extraArgs: string[], dispatch: (event: BrrEvent) => void, onExit: (code: number | null) => void): void;
