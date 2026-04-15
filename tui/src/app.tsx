import React, { useReducer, useCallback } from "react";
import { Box, useApp, useInput } from "ink";

import { type BrrEvent } from "./types.js";
import { initialState, reducer } from "./store.js";
import { useEventStream } from "./hooks/useEventStream.js";
import { useTerminalSize } from "./hooks/useTerminalSize.js";

import { Header } from "./components/Header.js";
import { EnvPanel } from "./components/EnvPanel.js";
import { PollStatus } from "./components/PollStatus.js";
import { RequestPanel } from "./components/RequestPanel.js";
import { ArtifactRow } from "./components/ArtifactRow.js";
import { EventLog } from "./components/EventLog.js";
import { StatusBar } from "./components/StatusBar.js";

interface AppProps {
  wasmPath: string;
  rustBin: string;
  extraArgs: string[];
}

export function App({ wasmPath, rustBin, extraArgs }: AppProps) {
  const { exit } = useApp();
  const [state, dispatch] = useReducer(reducer, initialState(wasmPath));

  const onEvent = useCallback(
    (event: BrrEvent) => dispatch(event),
    [dispatch]
  );

  const onExit = useCallback(
    (code: number | null) => {
      dispatch({
        type: "sidecar_exit",
        ts: new Date().toISOString(),
        reason: code === 0 ? "completed" : `exit code ${code ?? "null"}`,
      });
    },
    [dispatch]
  );

  useEventStream(rustBin, wasmPath, extraArgs, onEvent, onExit);

  const { rows } = useTerminalSize();

  // q to quit
  useInput((input, key) => {
    if (input === "q" || (key.ctrl && input === "c")) {
      exit();
    }
  });

  const { describe, polling } = state;

  return (
    <Box flexDirection="column" height={rows} overflow="hidden">
      {/* Header */}
      <Header
        wasmPath={state.wasmPath}
        abiVersion={state.abiVersion}
        describe={describe}
      />

      {/* Two-column middle row: EnvPanel + PollStatus */}
      <Box flexDirection="row">
        <Box width="50%">
          <EnvPanel vars={state.mergedEnvVars} />
        </Box>
        <Box width="50%">
          <PollStatus
            phase={polling.phase}
            sleepUntilMs={polling.sleepUntilMs}
            lastSuccessAt={polling.lastSuccessAt}
            consecutiveFailures={polling.consecutiveFailures}
            backoffMs={polling.backoffMs}
            pollStrategy={describe?.poll_strategy}
            persistenceAuthority={describe?.state_persistence}
          />
        </Box>
      </Box>

      {/* Last request */}
      <RequestPanel request={state.lastRequest} />

      {/* Three-column artifact row: raw | pipe animation | output — grows to fill remaining space */}
      <Box flexGrow={1} overflow="hidden">
        <ArtifactRow artifacts={state.artifacts} cycleCount={state.cycleCount} />
      </Box>

      {/* Log strip */}
      <EventLog logs={state.logs} />

      {/* Status bar */}
      <StatusBar isRunning={state.isRunning} error={state.error} />
    </Box>
  );
}
