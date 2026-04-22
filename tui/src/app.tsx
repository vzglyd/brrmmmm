import React, { useEffect, useMemo, useState } from "react";
import { Box, Text, useApp, useInput } from "ink";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve, dirname } from "node:path";

import {
  type DaemonMissionSummary,
  type ModuleParamField,
  type TuiState,
} from "./types.js";
import { initialState, reducer } from "./store.js";
import { useTerminalSize } from "./hooks/useTerminalSize.js";
import {
  abortMission,
  fetchMissionStatus,
  holdMission,
  launchMission,
  parseLaunchArgs,
  rescueRetryMission,
  resumeMission,
  watchDaemonStatus,
  watchMission,
} from "./stream.js";
import { formatDuration, formatLocalTime } from "./format.js";

import { Header } from "./components/Header.js";
import { EnvPanel } from "./components/EnvPanel.js";
import { PollStatus } from "./components/PollStatus.js";
import { RequestPanel } from "./components/RequestPanel.js";
import { ArtifactRow } from "./components/ArtifactRow.js";
import { EventLog } from "./components/EventLog.js";
import { HelpDialog } from "./components/HelpDialog.js";

interface AppProps {
  initialWasmPath?: string;
  rustBin: string;
  extraArgs: string[];
}

type View = "list" | "detail";
type FocusPane = "pipeline" | "raw" | "output";
type LauncherField = "wasm" | "name" | "env" | "paramsSource" | "paramsValue";

interface LauncherState {
  wasmPath: string;
  missionName: string;
  envText: string;
  paramsSource: "none" | "json" | "file";
  paramsValue: string;
  error: string | null;
  submitting: boolean;
}

interface FileBrowserState {
  dir: string;
  entries: Array<{ name: string; isDir: boolean }>;
  selectedIndex: number;
}

const FOCUS_ORDER: FocusPane[] = ["pipeline", "raw", "output"];
const LAUNCHER_FIELDS: LauncherField[] = [
  "wasm",
  "name",
  "env",
  "paramsSource",
  "paramsValue",
];
const AMBER = "#FFB300";

export function App({ initialWasmPath, rustBin: _rustBin, extraArgs }: AppProps) {
  const { exit } = useApp();
  const { rows } = useTerminalSize();
  const parsedArgs = useMemo(() => parseLaunchArgs(extraArgs), [extraArgs]);
  const [missions, setMissions] = useState<DaemonMissionSummary[]>([]);
  const [daemonError, setDaemonError] = useState<string | null>(null);
  const [selectedMission, setSelectedMission] = useState<string | null>(null);
  const [detailState, setDetailState] = useState<TuiState>(() =>
    initialState(initialWasmPath ?? "daemon"),
  );
  const [view, setView] = useState<View>("list");
  const [focusPane, setFocusPane] = useState<FocusPane>("pipeline");
  const [showHelp, setShowHelp] = useState(false);
  const [showLauncher, setShowLauncher] = useState(() => Boolean(initialWasmPath));
  const [launcherField, setLauncherField] = useState<LauncherField>("wasm");
  const [actionError, setActionError] = useState<string | null>(null);
  const [launcher, setLauncher] = useState<LauncherState>(() => ({
    wasmPath: initialWasmPath ?? "",
    missionName: "",
    envText: formatEnv(parsedArgs.env),
    paramsSource: parsedArgs.paramsSource,
    paramsValue:
      parsedArgs.paramsSource === "file"
        ? parsedArgs.paramsPath ?? ""
        : parsedArgs.params ?? "",
    error: null,
    submitting: false,
  }));
  const [paramValues, setParamValues] = useState<Record<string, string>>({});
  const [fileBrowser, setFileBrowser] = useState<FileBrowserState | null>(null);
  const pipelineHeight = pipelineHeightForRows(rows);
  const helpHeight = Math.max(8, rows - 9);
  const selectedSummary =
    missions.find((mission) => mission.name === selectedMission) ?? null;
  const paramFields: ModuleParamField[] =
    detailState.describe?.params?.fields ?? [];
  const paramFieldKey = paramFields
    .map((field) => `${field.key}:${field.type}`)
    .join("|");

  useEffect(() => {
    const handle = watchDaemonStatus(
      (nextMissions) => {
        setDaemonError(null);
        setMissions(nextMissions);
      },
      (message) => {
        setDaemonError(message);
        setMissions([]);
      },
    );

    return () => {
      handle.stop();
    };
  }, []);

  useEffect(() => {
    void fetchMissionStatus()
      .then((nextMissions) => {
        setDaemonError(null);
        setMissions(nextMissions);
      })
      .catch((error) => {
        setDaemonError(asErrorMessage(error));
      });
  }, []);

  useEffect(() => {
    if (missions.length === 0) {
      setSelectedMission(null);
      setView("list");
      return;
    }

    const missionStillExists =
      selectedMission && missions.some((mission) => mission.name === selectedMission);
    if (!missionStillExists) {
      setSelectedMission(missions[0]?.name ?? null);
      setView("list");
    }
  }, [missions, selectedMission]);

  useEffect(() => {
    if (!selectedSummary) {
      setDetailState(initialState(initialWasmPath ?? "daemon"));
      setParamValues({});
      return;
    }

    setDetailState(initialState(selectedSummary.wasm));
    setParamValues({});

    const handle = watchMission(
      selectedSummary.name,
      (event) => {
        setDetailState((state) => reducer(state, event));
      },
      (message) => {
        setDetailState((state) =>
          reducer(state, {
            type: "fatal_error",
            ts: new Date().toISOString(),
            message,
          }),
        );
      },
    );

    return () => {
      handle.stop();
    };
  }, [initialWasmPath, selectedSummary?.name, selectedSummary?.wasm]);

  useEffect(() => {
    if (paramFields.length === 0) {
      setParamValues({});
      return;
    }
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
  }, [paramFieldKey, paramFields]);

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

    if (showLauncher) {
      if (fileBrowser) {
        handleFileBrowserInput(input, key);
      } else {
        handleLauncherInput(input, key);
      }
      return;
    }

    if (input === "?" || input === "h") {
      setShowHelp(true);
      return;
    }

    if (input === "l" || input === "n") {
      setShowLauncher(true);
      setLauncherField("wasm");
      setActionError(null);
      return;
    }

    if (view === "list") {
      if (key.upArrow) {
        moveMissionSelection(-1);
        return;
      }
      if (key.downArrow) {
        moveMissionSelection(1);
        return;
      }
      if (key.return) {
        if (selectedSummary) {
          setView("detail");
          setFocusPane("pipeline");
        } else {
          setShowLauncher(true);
          setLauncherField("wasm");
          setActionError(null);
        }
        return;
      }
      if (input === "f" && selectedSummary) {
        void forceSelectedMission(selectedSummary.name);
        return;
      }
      if (input === "x" && selectedSummary) {
        void stopSelectedMission(selectedSummary.name);
        return;
      }
      if (input === " ") {
        void toggleHold(selectedSummary);
        return;
      }
      if (input === "q") {
        exit();
      }
      return;
    }

    // view === "detail"
    if (key.escape || input === "b") {
      setView("list");
      return;
    }
    if (key.tab) {
      cycleFocus();
      return;
    }
    if (input === "f" && selectedSummary) {
      void forceSelectedMission(selectedSummary.name);
      return;
    }
    if (input === "x" && selectedSummary) {
      void stopSelectedMission(selectedSummary.name);
      return;
    }
    if (input === " ") {
      void toggleHold(selectedSummary);
      return;
    }
    if (input === "q") {
      exit();
    }
  });

  return (
    <Box flexDirection="column" height={rows} overflow="hidden">
      <DashboardHeader
        missionCount={missions.length}
        daemonError={daemonError}
        selected={view === "detail" ? selectedSummary : null}
      />

      {showHelp ? (
        <HelpDialog describe={detailState.describe} height={helpHeight} />
      ) : showLauncher ? (
        <LauncherDialog
          state={launcher}
          field={launcherField}
          fileBrowser={fileBrowser}
          height={rows - 6}
        />
      ) : view === "list" ? (
        <MissionListView
          missions={missions}
          selectedMission={selectedMission}
          daemonError={daemonError}
          height={rows - 4}
        />
      ) : selectedSummary ? (
        <DetailView
          detailState={detailState}
          selectedSummary={selectedSummary}
          focusPane={focusPane}
          pipelineHeight={pipelineHeight}
          paramValues={paramValues}
          onParamChange={(key, value) =>
            setParamValues((current) => ({ ...current, [key]: value }))
          }
        />
      ) : (
        <EmptyDashboard daemonError={daemonError} />
      )}

      <DashboardStatusBar
        view={view}
        launcherOpen={showLauncher}
        helpOpen={showHelp}
        error={launcher.error ?? actionError ?? daemonError ?? detailState.error}
      />
    </Box>
  );

  function cycleFocus(): void {
    setFocusPane(
      (current) => FOCUS_ORDER[(FOCUS_ORDER.indexOf(current) + 1) % FOCUS_ORDER.length]!,
    );
  }

  function moveMissionSelection(direction: -1 | 1): void {
    if (missions.length === 0) {
      return;
    }
    const currentIndex = selectedMission
      ? missions.findIndex((mission) => mission.name === selectedMission)
      : 0;
    const nextIndex = Math.min(
      missions.length - 1,
      Math.max(0, currentIndex + direction),
    );
    setSelectedMission(missions[nextIndex]?.name ?? null);
  }

  async function forceSelectedMission(mission: string): Promise<void> {
    setActionError(null);
    try {
      await rescueRetryMission(mission);
    } catch (error) {
      setActionError(asErrorMessage(error));
    }
  }

  async function stopSelectedMission(mission: string): Promise<void> {
    setActionError(null);
    try {
      await abortMission(mission, "TUI abort requested");
    } catch (error) {
      setActionError(asErrorMessage(error));
    }
  }

  async function toggleHold(summary: DaemonMissionSummary | null): Promise<void> {
    if (!summary) {
      return;
    }
    setActionError(null);
    try {
      if (summary.state === "held") {
        await resumeMission(summary.name);
      } else {
        await holdMission(summary.name, "TUI hold requested");
      }
    } catch (error) {
      setActionError(asErrorMessage(error));
    }
  }

  function openFileBrowserAtDir(dir: string): void {
    try {
      const raw = readdirSync(dir, { withFileTypes: true });
      const entries = raw
        .filter((e) => e.isDirectory() || e.name.endsWith(".wasm"))
        .sort((a, b) => {
          if (a.isDirectory() && !b.isDirectory()) return -1;
          if (!a.isDirectory() && b.isDirectory()) return 1;
          return a.name.localeCompare(b.name);
        })
        .map((e) => ({ name: e.name, isDir: e.isDirectory() }));
      setFileBrowser({ dir, entries: [{ name: "..", isDir: true }, ...entries], selectedIndex: 0 });
    } catch {
      // ignore unreadable directories
    }
  }

  function openFileBrowser(fromPath: string): void {
    const trimmed = fromPath.trim();
    let dir: string;
    if (trimmed && (trimmed.endsWith("/") || trimmed.endsWith("\\"))) {
      dir = resolve(trimmed);
    } else if (trimmed.includes("/")) {
      dir = resolve(dirname(trimmed));
    } else {
      dir = process.cwd();
    }
    openFileBrowserAtDir(dir);
  }

  function handleFileBrowserInput(
    _input: string,
    key: {
      upArrow?: boolean;
      downArrow?: boolean;
      escape?: boolean;
      return?: boolean;
    },
  ): void {
    if (!fileBrowser) return;

    if (key.escape) {
      setFileBrowser(null);
      return;
    }

    if (key.upArrow) {
      setFileBrowser((current) =>
        current ? { ...current, selectedIndex: Math.max(0, current.selectedIndex - 1) } : null,
      );
      return;
    }

    if (key.downArrow) {
      setFileBrowser((current) =>
        current
          ? { ...current, selectedIndex: Math.min(current.entries.length - 1, current.selectedIndex + 1) }
          : null,
      );
      return;
    }

    if (key.return) {
      const entry = fileBrowser.entries[fileBrowser.selectedIndex];
      if (!entry) return;

      if (entry.isDir) {
        const newDir =
          entry.name === ".."
            ? resolve(fileBrowser.dir, "..")
            : resolve(fileBrowser.dir, entry.name);
        openFileBrowserAtDir(newDir);
      } else {
        const fullPath = resolve(fileBrowser.dir, entry.name);
        setLauncher((current) => ({ ...current, wasmPath: fullPath, error: null }));
        setFileBrowser(null);
      }
    }
  }

  function handleLauncherInput(
    input: string,
    key: {
      upArrow?: boolean;
      downArrow?: boolean;
      leftArrow?: boolean;
      rightArrow?: boolean;
      tab?: boolean;
      backspace?: boolean;
      delete?: boolean;
      escape?: boolean;
      return?: boolean;
      ctrl?: boolean;
      meta?: boolean;
    },
  ): void {
    if (key.escape || input === "l") {
      setShowLauncher(false);
      return;
    }

    if (key.return) {
      void submitLauncher();
      return;
    }

    // Tab on wasm field opens file browser
    if (key.tab && launcherField === "wasm") {
      openFileBrowser(launcher.wasmPath);
      return;
    }

    if (key.tab || key.downArrow) {
      setLauncherField(nextLauncherField(1));
      return;
    }

    if (key.upArrow) {
      setLauncherField(nextLauncherField(-1));
      return;
    }

    // f on wasm field opens file browser
    if (input === "f" && launcherField === "wasm") {
      openFileBrowser(launcher.wasmPath);
      return;
    }

    if (launcherField === "paramsSource") {
      if (key.leftArrow) {
        setLauncher((current) => ({
          ...current,
          paramsSource: rotateParamsSource(current.paramsSource, -1),
          error: null,
        }));
        return;
      }
      if (key.rightArrow || input === " ") {
        setLauncher((current) => ({
          ...current,
          paramsSource: rotateParamsSource(current.paramsSource, 1),
          error: null,
        }));
        return;
      }
    }

    if (key.backspace || key.delete) {
      updateLauncherField((value) => value.slice(0, -1));
      return;
    }

    if (input && !key.ctrl && !key.meta) {
      updateLauncherField((value) => `${value}${input}`);
    }
  }

  function nextLauncherField(direction: -1 | 1): LauncherField {
    const index = LAUNCHER_FIELDS.indexOf(launcherField);
    const nextIndex =
      (index + direction + LAUNCHER_FIELDS.length) % LAUNCHER_FIELDS.length;
    return LAUNCHER_FIELDS[nextIndex]!;
  }

  function updateLauncherField(
    transform: (value: string) => string,
  ): void {
    setLauncher((current) => {
      if (launcherField === "paramsSource") {
        return current;
      }
      const key =
        launcherField === "wasm"
          ? "wasmPath"
          : launcherField === "name"
            ? "missionName"
            : launcherField === "env"
              ? "envText"
              : "paramsValue";
      return {
        ...current,
        [key]: transform(current[key]),
        error: null,
      };
    });
  }

  async function submitLauncher(): Promise<void> {
    setLauncher((current) => ({ ...current, submitting: true, error: null }));
    try {
      const mission = await launchMission(buildLaunchRequest(launcher));
      setLauncher((current) => ({ ...current, submitting: false, error: null }));
      setShowLauncher(false);
      setSelectedMission(mission);
      setView("detail");
      setFocusPane("pipeline");
      setActionError(null);
    } catch (error) {
      setLauncher((current) => ({
        ...current,
        submitting: false,
        error: asErrorMessage(error),
      }));
    }
  }
}

function DashboardHeader({
  missionCount,
  daemonError,
  selected,
}: {
  missionCount: number;
  daemonError: string | null;
  selected: DaemonMissionSummary | null;
}) {
  return (
    <Box
      borderStyle="round"
      borderColor={AMBER}
      paddingX={1}
      justifyContent="space-between"
    >
      <Text bold color={AMBER}>
        brrmmmm dashboard
      </Text>
      <Text dimColor>
        {daemonError
          ? daemonError
          : selected
            ? `${selected.name} · ${selected.state}`
            : `${missionCount} mission${missionCount === 1 ? "" : "s"}`}
      </Text>
    </Box>
  );
}

function MissionListView({
  missions,
  selectedMission,
  daemonError,
  height,
}: {
  missions: DaemonMissionSummary[];
  selectedMission: string | null;
  daemonError: string | null;
  height: number;
}) {
  return (
    <Box
      borderStyle="single"
      borderColor={AMBER}
      flexDirection="column"
      paddingX={1}
      height={height}
      overflow="hidden"
    >
      <Text bold color={AMBER}>
        MISSIONS
      </Text>
      {daemonError ? (
        <Text color="red" wrap="truncate">
          {daemonError}
        </Text>
      ) : missions.length === 0 ? (
        <Box flexGrow={1} flexDirection="column">
          <Text dimColor>No missions running</Text>
          <Text dimColor>Press l to launch a mission</Text>
        </Box>
      ) : (
        missions.map((mission) => {
          const selected = mission.name === selectedMission;
          const lastSeen = mission.last_run_at_ms ?? mission.last_started_at_ms ?? null;
          return (
            <Box key={mission.name} flexDirection="row" marginTop={1}>
              <Text color={selected ? AMBER : undefined} bold={selected}>
                {selected ? ">" : " "}
              </Text>
              <Box flexGrow={1} flexDirection="column" marginLeft={1}>
                <Box justifyContent="space-between">
                  <Text color={selected ? AMBER : undefined} bold={selected} wrap="truncate">
                    {mission.name}
                  </Text>
                  <Text dimColor>
                    {mission.state} · {mission.phase}
                    {mission.held ? " [HELD]" : ""}
                    {mission.terminal ? " [TERMINAL]" : ""}
                  </Text>
                </Box>
                <Text dimColor>
                  {lastSeen ? `  last ${formatLocalTime(lastSeen)}` : "  never run"}
                  {mission.last_outcome_status ? ` · ${mission.last_outcome_status}` : ""}
                </Text>
              </Box>
            </Box>
          );
        })
      )}
      <Box marginTop={1}>
        <Text dimColor>Enter open · l launch · f retry · space hold/resume · x abort · ? help · q quit</Text>
      </Box>
    </Box>
  );
}

function DetailView({
  detailState,
  selectedSummary,
  focusPane,
  pipelineHeight,
  paramValues,
  onParamChange,
}: {
  detailState: TuiState;
  selectedSummary: DaemonMissionSummary;
  focusPane: FocusPane;
  pipelineHeight: number;
  paramValues: Record<string, string>;
  onParamChange: (key: string, value: string) => void;
}) {
  return (
    <Box flexDirection="column" flexGrow={1} overflow="hidden">
      <Header
        wasmPath={detailState.wasmPath}
        abiVersion={detailState.abiVersion}
        hasStarted={detailState.hasStarted}
        describe={detailState.describe}
        error={detailState.error}
        startTimeMs={selectedSummary.last_started_at_ms ?? Date.now()}
      />

      <MissionMeta summary={selectedSummary} />

      <Box flexDirection="row">
        <Box width="50%">
          <EnvPanel
            vars={detailState.mergedEnvVars}
            params={detailState.describe?.params ?? null}
            hasStarted={detailState.hasStarted}
            isFocused={false}
            values={paramValues}
            onChange={onParamChange}
          />
        </Box>
        <Box width="50%">
          <PollStatus
            hasStarted={detailState.hasStarted}
            phase={detailState.polling.phase}
            sleepUntilMs={detailState.polling.sleepUntilMs}
            lastSuccessAt={detailState.polling.lastSuccessAt}
            consecutiveFailures={detailState.polling.consecutiveFailures}
            backoffMs={detailState.polling.backoffMs}
            pollStrategy={detailState.describe?.poll_strategy}
            persistenceAuthority={detailState.describe?.state_persistence}
            missionOutcome={detailState.missionOutcome}
          />
        </Box>
      </Box>

      <RequestPanel
        request={detailState.lastRequest}
        requests={detailState.requests}
        artifacts={detailState.artifacts}
        describe={detailState.describe}
        hasStarted={detailState.hasStarted}
        isFocused={focusPane === "pipeline"}
        height={pipelineHeight}
      />

      <Box flexGrow={1} overflow="hidden">
        <ArtifactRow
          artifacts={detailState.artifacts}
          cycleCount={detailState.cycleCount}
          focusedPane={
            focusPane === "raw" || focusPane === "output"
              ? focusPane
              : null
          }
        />
      </Box>

      <EventLog logs={detailState.logs} />
    </Box>
  );
}

function MissionMeta({ summary }: { summary: DaemonMissionSummary }) {
  const lastSeen = summary.last_run_at_ms ?? summary.last_started_at_ms ?? null;
  const nextWake =
    summary.next_wake_at_ms && summary.next_wake_at_ms > Date.now()
      ? formatDuration(summary.next_wake_at_ms - Date.now())
      : null;

  return (
    <Box borderStyle="single" borderColor="gray" paddingX={1} justifyContent="space-between">
      <Text dimColor>
        state: {summary.state} · cycles: {summary.cycles}
        {summary.last_outcome_status ? ` · outcome: ${summary.last_outcome_status}` : ""}
      </Text>
      <Text dimColor>
        {lastSeen ? `last ${formatLocalTime(lastSeen)}` : "never run"}
        {nextWake ? ` · next ${nextWake}` : ""}
      </Text>
    </Box>
  );
}

function EmptyDashboard({ daemonError }: { daemonError: string | null }) {
  return (
    <Box borderStyle="single" borderColor="gray" flexDirection="column" paddingX={1}>
      <Text bold color={AMBER}>
        DASHBOARD
      </Text>
      <Text dimColor>
        {daemonError
          ? "Daemon unavailable. Install and start it before launching a mission."
          : "No mission selected. Press l to launch a mission."}
      </Text>
    </Box>
  );
}

function LauncherDialog({
  state,
  field,
  fileBrowser,
  height,
}: {
  state: LauncherState;
  field: LauncherField;
  fileBrowser: FileBrowserState | null;
  height: number;
}) {
  return (
    <Box
      borderStyle="round"
      borderColor={AMBER}
      flexDirection="column"
      paddingX={1}
      height={height}
      overflow="hidden"
    >
      <Text bold color={AMBER}>
        Launch Mission
      </Text>
      <LauncherRow
        label="WASM path"
        value={state.wasmPath}
        selected={field === "wasm"}
      />
      {fileBrowser ? (
        <FileBrowserPanel browser={fileBrowser} />
      ) : (
        <>
          {field === "wasm" ? (
            <Text dimColor>  Tab or f → open file browser</Text>
          ) : null}
          <LauncherRow
            label="Mission name"
            value={state.missionName}
            selected={field === "name"}
          />
          <LauncherRow
            label="Env vars"
            value={state.envText}
            selected={field === "env"}
          />
          <LauncherRow
            label="Params mode"
            value={state.paramsSource}
            selected={field === "paramsSource"}
          />
          <LauncherRow
            label={state.paramsSource === "file" ? "Params file" : "Params JSON"}
            value={state.paramsValue}
            selected={field === "paramsValue"}
          />
        </>
      )}
      <Box marginTop={1} flexDirection="column">
        {fileBrowser ? (
          <Text dimColor>↑/↓ browse · Enter select · Esc cancel browser</Text>
        ) : (
          <>
            <Text dimColor>Enter launch · Tab/↑/↓ move · Left/Right params mode · Esc close</Text>
            <Text dimColor>Env vars: KEY=VALUE, KEY2=VALUE2</Text>
          </>
        )}
        {state.error ? <Text color="red">{state.error}</Text> : null}
        {state.submitting ? <Text color={AMBER}>Launching...</Text> : null}
      </Box>
    </Box>
  );
}

function FileBrowserPanel({ browser }: { browser: FileBrowserState }) {
  return (
    <Box
      borderStyle="single"
      borderColor="gray"
      flexDirection="column"
      paddingX={1}
      marginTop={1}
      flexGrow={1}
      overflow="hidden"
    >
      <Text dimColor>{browser.dir}/</Text>
      {browser.entries.map((entry, index) => {
        const selected = index === browser.selectedIndex;
        return (
          <Text
            key={`${entry.name}-${index}`}
            color={selected ? AMBER : undefined}
            dimColor={!selected}
            wrap="truncate"
          >
            {selected ? ">" : " "} {entry.name}{entry.isDir ? "/" : ""}
          </Text>
        );
      })}
    </Box>
  );
}

function LauncherRow({
  label,
  value,
  selected,
}: {
  label: string;
  value: string;
  selected: boolean;
}) {
  return (
    <Text color={selected ? AMBER : undefined} wrap="truncate">
      {selected ? ">" : " "} {label}: {value}
      {selected ? "█" : ""}
    </Text>
  );
}

function DashboardStatusBar({
  view,
  launcherOpen,
  helpOpen,
  error,
}: {
  view: View;
  launcherOpen: boolean;
  helpOpen: boolean;
  error: string | null;
}) {
  return (
    <Box borderStyle="single" borderColor="gray" paddingX={1} justifyContent="space-between">
      <Text dimColor>
        {helpOpen
          ? "↑/↓/PgUp/PgDn scroll · h/?/Esc close · Ctrl+C quit"
          : launcherOpen
            ? "Enter launch · Tab/↑/↓ move · Left/Right params mode · Esc close"
            : view === "list"
              ? "↑/↓ select · Enter open · l launch · f retry · space hold/resume · x abort · ? help · q quit"
              : "b/Esc back · Tab cycle panels · l launch · f retry · space hold/resume · ? help · q quit"}
      </Text>
      {error ? <Text color="red">{error}</Text> : null}
    </Box>
  );
}

function pipelineHeightForRows(rows: number): number {
  if (rows < 22) return 5;
  return Math.min(10, Math.max(6, Math.floor(rows * 0.18)));
}

function formatEnv(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join(", ");
}

function parseEnvText(text: string): Record<string, string> {
  return text
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean)
    .reduce<Record<string, string>>((env, part) => {
      const split = part.indexOf("=");
      if (split > 0) {
        env[part.slice(0, split)] = part.slice(split + 1);
      }
      return env;
    }, {});
}

function buildLaunchRequest(state: LauncherState) {
  const wasmPath = state.wasmPath.trim();
  if (!wasmPath.endsWith(".wasm")) {
    throw new Error("launch path must point to a .wasm mission module");
  }

  let params: string | undefined;
  if (state.paramsSource === "json") {
    params = state.paramsValue.trim() || undefined;
  } else if (state.paramsSource === "file") {
    const path = state.paramsValue.trim();
    if (!path) {
      throw new Error("params file path is empty");
    }
    if (!existsSync(path)) {
      throw new Error(`params file not found: ${path}`);
    }
    params = readFileSync(path, "utf8");
  }

  return {
    wasm: wasmPath,
    name: state.missionName.trim() || undefined,
    env: parseEnvText(state.envText),
    params,
  };
}

function rotateParamsSource(
  current: LauncherState["paramsSource"],
  direction: -1 | 1,
): LauncherState["paramsSource"] {
  const order: LauncherState["paramsSource"][] = ["none", "json", "file"];
  const index = order.indexOf(current);
  return order[(index + direction + order.length) % order.length]!;
}

function paramDefaultText(value: unknown): string {
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : JSON.stringify(value);
}

function asErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
