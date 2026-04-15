import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useReducer, useCallback } from "react";
import { Box, useApp, useInput } from "ink";
import { initialState, reducer } from "./store.js";
import { useEventStream } from "./hooks/useEventStream.js";
import { Header } from "./components/Header.js";
import { EnvPanel } from "./components/EnvPanel.js";
import { PollStatus } from "./components/PollStatus.js";
import { RequestPanel } from "./components/RequestPanel.js";
import { ArtifactRow } from "./components/ArtifactRow.js";
import { EventLog } from "./components/EventLog.js";
import { StatusBar } from "./components/StatusBar.js";
export function App({ wasmPath, rustBin, extraArgs }) {
    const { exit } = useApp();
    const [state, dispatch] = useReducer(reducer, initialState(wasmPath));
    const onEvent = useCallback((event) => dispatch(event), [dispatch]);
    const onExit = useCallback((code) => {
        dispatch({
            type: "sidecar_exit",
            ts: new Date().toISOString(),
            reason: code === 0 ? "completed" : `exit code ${code ?? "null"}`,
        });
    }, [dispatch]);
    useEventStream(rustBin, wasmPath, extraArgs, onEvent, onExit);
    // q to quit
    useInput((input, key) => {
        if (input === "q" || (key.ctrl && input === "c")) {
            exit();
        }
    });
    const { describe, polling } = state;
    return (_jsxs(Box, { flexDirection: "column", children: [_jsx(Header, { wasmPath: state.wasmPath, abiVersion: state.abiVersion, describe: describe }), _jsxs(Box, { flexDirection: "row", children: [_jsx(Box, { width: "50%", children: _jsx(EnvPanel, { vars: state.mergedEnvVars }) }), _jsx(Box, { width: "50%", children: _jsx(PollStatus, { phase: polling.phase, sleepUntilMs: polling.sleepUntilMs, lastSuccessAt: polling.lastSuccessAt, consecutiveFailures: polling.consecutiveFailures, backoffMs: polling.backoffMs, pollStrategy: describe?.poll_strategy, persistenceAuthority: describe?.state_persistence }) })] }), _jsx(RequestPanel, { request: state.lastRequest }), _jsx(ArtifactRow, { artifacts: state.artifacts, cycleCount: state.cycleCount }), _jsx(EventLog, { logs: state.logs }), _jsx(StatusBar, { isRunning: state.isRunning, error: state.error })] }));
}
