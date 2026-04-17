import React, { useReducer, useCallback, useEffect, useState } from "react";
import { Box, useApp, useInput } from "ink";

import { type BrrEvent, type SidecarParamField } from "./types.js";
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
import { HelpDialog } from "./components/HelpDialog.js";

interface AppProps {
  wasmPath: string;
  rustBin: string;
  extraArgs: string[];
}

type FocusPane = "params" | "pipeline" | "raw" | "output";

const FOCUS_ORDER: FocusPane[] = ["params", "pipeline", "raw", "output"];

export function App({ wasmPath, rustBin, extraArgs }: AppProps) {
  const { exit } = useApp();
  const [state, dispatch] = useReducer(reducer, initialState(wasmPath));
  const [focusPane, setFocusPane] = useState<FocusPane>("params");
  const [showHelp, setShowHelp] = useState(false);
  const [paramValues, setParamValues] = useState<Record<string, string>>(() =>
    initialParamValuesFromArgs(extraArgs),
  );

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

  const sendCommand = useEventStream(rustBin, wasmPath, extraArgs, onEvent, onExit);

  const { rows } = useTerminalSize();
  const { describe, polling } = state;
  const pipelineHeight = pipelineHeightForRows(rows);
  const helpHeight = Math.max(8, rows - 9);
  const paramFields = describe?.params?.fields ?? [];
  const paramFieldKey = paramFields.map((field) => `${field.key}:${field.type}`).join("|");

  useEffect(() => {
    if (paramFields.length === 0) return;
    setParamValues((current) => {
      let changed = false;
      const next = { ...current };
      for (const field of paramFields) {
        if (next[field.key] === undefined && field.default !== undefined) {
          next[field.key] = paramDefaultText(field.default);
          changed = true;
        }
      }
      return changed ? next : current;
    });
  }, [paramFieldKey]);

  // q = quit outside text entry, f = force refresh (skip current sleep and poll immediately)
  useInput((input, key) => {
    if (key.ctrl && input === "c") {
      exit();
      return;
    }

    if (showHelp) {
      if (input === "h" || input === "?" || input === "q" || key.escape) {
        setShowHelp(false);
      }
      return;
    }

    if (input === "?" || (input === "h" && focusPane !== "params")) {
      setShowHelp(true);
      return;
    }

    if (key.tab) {
      setFocusPane((pane) => FOCUS_ORDER[(FOCUS_ORDER.indexOf(pane) + 1) % FOCUS_ORDER.length]);
      return;
    }
    if (input === "q" && focusPane !== "params") {
      exit();
    }
    if (input === "f" && focusPane !== "params") {
      const paramsJson = buildParamsJson(paramFields, paramValues);
      if (paramsJson) {
        sendCommand(`params_json ${paramsJson}`);
      }
      sendCommand("force");
    }
  });

  return (
    <Box flexDirection="column" height={rows} overflow="hidden">
      {/* Header */}
      <Header
        wasmPath={state.wasmPath}
        abiVersion={state.abiVersion}
        describe={describe}
      />

      {showHelp ? (
        <HelpDialog describe={describe} height={helpHeight} />
      ) : (
        <>
          {/* Two-column middle row: EnvPanel + PollStatus */}
          <Box flexDirection="row">
            <Box width="50%">
              <EnvPanel
                vars={state.mergedEnvVars}
                params={describe?.params ?? null}
                manifestPending={describe == null}
                isFocused={focusPane === "params"}
                values={paramValues}
                onChange={(key, value) => {
                  setParamValues((current) => ({ ...current, [key]: value }));
                }}
              />
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

          {/* Pipeline trace */}
          <RequestPanel
            request={state.lastRequest}
            requests={state.requests}
            artifacts={state.artifacts}
            describe={state.describe}
            isFocused={focusPane === "pipeline"}
            height={pipelineHeight}
          />

          {/* Three-column artifact row: raw | pipe animation | output — grows to fill remaining space */}
          <Box flexGrow={1} overflow="hidden">
            <ArtifactRow
              artifacts={state.artifacts}
              cycleCount={state.cycleCount}
              focusedPane={focusPane === "raw" || focusPane === "output" ? focusPane : null}
            />
          </Box>

          {/* Log strip */}
          <EventLog logs={state.logs} />
        </>
      )}

      {/* Status bar */}
      <StatusBar isRunning={state.isRunning} error={state.error} focusPane={focusPane} isHelpOpen={showHelp} />
    </Box>
  );
}

function pipelineHeightForRows(rows: number): number {
  if (rows < 22) return 5;
  return Math.min(10, Math.max(6, Math.floor(rows * 0.18)));
}

function initialParamValuesFromArgs(extraArgs: string[]): Record<string, string> {
  const index = extraArgs.indexOf("--params-json");
  if (index < 0) return {};
  const raw = extraArgs[index + 1];
  if (!raw) return {};
  try {
    const value = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(
      Object.entries(value).map(([key, val]) => [key, paramDefaultText(val)]),
    );
  } catch {
    return {};
  }
}

function paramDefaultText(value: unknown): string {
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : JSON.stringify(value);
}

function buildParamsJson(fields: SidecarParamField[], values: Record<string, string>): string | null {
  if (fields.length === 0) return null;
  const params: Record<string, unknown> = {};
  for (const field of fields) {
    const text = values[field.key] ?? "";
    if (!field.required && text.trim() === "") continue;
    params[field.key] = coerceParamValue(field, text);
  }
  return JSON.stringify(params);
}

function coerceParamValue(field: SidecarParamField, text: string): unknown {
  switch (field.type) {
    case "integer":
      return Number.parseInt(text, 10);
    case "number":
      return Number.parseFloat(text);
    case "boolean":
      return ["1", "true", "yes", "on"].includes(text.trim().toLowerCase());
    case "json":
      try {
        return JSON.parse(text);
      } catch {
        return text;
      }
    case "string":
    default:
      return text;
  }
}
