import { useEffect, useRef, useCallback } from "react";
import { spawnEventStream, type StreamHandle } from "../stream.js";
import { type BrrmmmmEvent } from "../types.js";

/**
 * Connect to the daemon-backed event stream and dispatch mission events.
 * Returns a `sendCommand` callback for mission control actions.
 * Cleans up the watch connection when the component unmounts.
 */
export function useEventStream(
  rustBin: string,
  wasmPath: string,
  extraArgs: string[],
  dispatch: (event: BrrmmmmEvent) => void,
  onExit: (code: number | null) => void
): (cmd: string) => void {
  const handleRef = useRef<StreamHandle | null>(null);

  useEffect(() => {
    const handle = spawnEventStream(rustBin, wasmPath, extraArgs, dispatch, onExit);
    handleRef.current = handle;

    return () => {
      handle.stop();
      handleRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rustBin, wasmPath]);

  return useCallback((cmd: string) => {
    handleRef.current?.sendCommand(cmd);
  }, []);
}
