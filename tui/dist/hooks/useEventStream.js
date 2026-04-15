import { useEffect } from "react";
import { spawnEventStream } from "../stream.js";
/**
 * Spawn the Rust binary event stream and dispatch events.
 * Cleans up (kills the process) when the component unmounts.
 */
export function useEventStream(rustBin, wasmPath, extraArgs, dispatch, onExit) {
    useEffect(() => {
        let handle = null;
        handle = spawnEventStream(rustBin, wasmPath, extraArgs, dispatch, onExit);
        return () => {
            handle?.stop();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [rustBin, wasmPath]);
}
