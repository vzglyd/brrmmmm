import { useEffect } from "react";
import { spawnEventStream, type StreamHandle } from "../stream.js";
import { type BrrEvent } from "../types.js";

/**
 * Spawn the Rust binary event stream and dispatch events.
 * Cleans up (kills the process) when the component unmounts.
 */
export function useEventStream(
  rustBin: string,
  wasmPath: string,
  extraArgs: string[],
  dispatch: (event: BrrEvent) => void,
  onExit: (code: number | null) => void
): void {
  useEffect(() => {
    let handle: StreamHandle | null = null;

    handle = spawnEventStream(rustBin, wasmPath, extraArgs, dispatch, onExit);

    return () => {
      handle?.stop();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rustBin, wasmPath]);
}
