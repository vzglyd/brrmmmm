import { existsSync, readFileSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

import {
  type BrrmmmmEvent,
  type DaemonMissionSummary,
  type ModuleDescribe,
} from "./types.js";

export interface WatchHandle {
  stop: () => void;
}

export interface LaunchArgs {
  env: Record<string, string>;
  params?: string;
  paramsSource: "none" | "json" | "file";
  paramsPath?: string;
}

export interface LaunchRequest {
  wasm: string;
  name?: string;
  env: Record<string, string>;
  params?: string;
}

type DaemonCommand =
  | LaunchCommand
  | { type: "abort"; mission: string; reason: string }
  | { type: "hold"; mission: string; reason: string }
  | { type: "resume"; mission: string }
  | { type: "rescue"; mission: string; action: "retry" | "abort"; reason: string }
  | { type: "status" }
  | { type: "watch"; mission: string }
  | { type: "watch_status" }
  | { type: "inspect"; wasm: string };

interface LaunchCommand {
  type: "launch";
  wasm: string;
  name?: string;
  env: Record<string, string>;
  params?: string;
}

type DaemonResponse =
  | { type: "launched"; mission: string }
  | { type: "ok"; mission: string }
  | { type: "error"; message: string }
  | { type: "full"; message: string }
  | { type: "status"; missions: DaemonMissionSummary[] }
  | { type: "event"; mission: string; line: string }
  | { type: "inspected"; describe: ModuleDescribe | null };

export function daemonSocketPath(): string {
  return join(homedir(), ".brrmmmm", "daemon.sock");
}

export function resolveLaunchWasmPath(wasmPath: string): string {
  return resolve(wasmPath);
}

export function parseLaunchArgs(extraArgs: string[]): LaunchArgs {
  const env: Record<string, string> = {};
  let params: string | undefined;
  let paramsSource: LaunchArgs["paramsSource"] = "none";
  let paramsPath: string | undefined;

  for (let index = 0; index < extraArgs.length; index += 1) {
    const arg = extraArgs[index];
    if (arg === "-e" || arg === "--env") {
      const value = extraArgs[index + 1];
      index += 1;
      if (!value) {
        continue;
      }
      const split = value.indexOf("=");
      if (split > 0) {
        env[value.slice(0, split)] = value.slice(split + 1);
      }
      continue;
    }

    if (arg === "-j" || arg === "--params-json") {
      params = extraArgs[index + 1];
      paramsSource = params ? "json" : "none";
      index += 1;
      continue;
    }

    if (arg === "-f" || arg === "--params-file") {
      paramsPath = extraArgs[index + 1];
      index += 1;
      if (paramsPath && existsSync(paramsPath)) {
        params = readFileSync(paramsPath, "utf8");
        paramsSource = "file";
      }
    }
  }

  return { env, params, paramsSource, paramsPath };
}

export async function inspectMission(wasm: string): Promise<ModuleDescribe | null> {
  const response = await sendDaemonCommand({
    type: "inspect",
    wasm: resolveLaunchWasmPath(wasm),
  });
  if (response.type === "inspected") return response.describe;
  if (response.type === "error") throw new Error(response.message);
  throw new Error("unexpected response from daemon inspect");
}

export async function launchMission(request: LaunchRequest): Promise<string> {
  const response = await sendDaemonCommand({
    type: "launch",
    wasm: resolveLaunchWasmPath(request.wasm),
    name: request.name,
    env: request.env,
    params: request.params,
  });
  if (response.type === "launched") {
    return response.mission;
  }
  if (response.type === "error" || response.type === "full") {
    throw new Error(response.message);
  }
  throw new Error("unexpected response from daemon launch");
}

export async function rescueRetryMission(mission: string): Promise<void> {
  await expectOkResponse({
    type: "rescue",
    mission,
    action: "retry",
    reason: "TUI force refresh requested",
  });
}

export async function abortMission(mission: string, reason: string): Promise<void> {
  await expectOkResponse({
    type: "abort",
    mission,
    reason,
  });
}

export async function holdMission(mission: string, reason: string): Promise<void> {
  await expectOkResponse({
    type: "hold",
    mission,
    reason,
  });
}

export async function resumeMission(mission: string): Promise<void> {
  await expectOkResponse({
    type: "resume",
    mission,
  });
}

export async function fetchMissionStatus(): Promise<DaemonMissionSummary[]> {
  const response = await sendDaemonCommand({ type: "status" });
  if (response.type === "status") {
    return response.missions;
  }
  if (response.type === "error") {
    throw new Error(response.message);
  }
  throw new Error("unexpected response from daemon status");
}

export function watchMission(
  mission: string,
  onEvent: (event: BrrmmmmEvent) => void,
  onError: (message: string) => void,
  onClose?: () => void,
): WatchHandle {
  return watchDaemonStream(
    { type: "watch", mission },
    (response) => {
      if (response.type === "event") {
        try {
          onEvent(JSON.parse(response.line) as BrrmmmmEvent);
        } catch {
          // Ignore malformed daemon event payloads.
        }
        return;
      }
      if (response.type === "error") {
        onError(`mission watch error: ${response.message}`);
      }
    },
    onError,
    onClose,
  );
}

export function watchDaemonStatus(
  onStatus: (missions: DaemonMissionSummary[]) => void,
  onError: (message: string) => void,
): WatchHandle {
  return watchDaemonStream(
    { type: "watch_status" },
    (response) => {
      if (response.type === "status") {
        onStatus(response.missions);
        return;
      }
      if (response.type === "error") {
        onError(response.message);
      }
    },
    onError,
  );
}

async function expectOkResponse(command: DaemonCommand): Promise<void> {
  const response = await sendDaemonCommand(command);
  if (response.type === "ok") {
    return;
  }
  if (response.type === "error" || response.type === "full") {
    throw new Error(response.message);
  }
  throw new Error("unexpected response from daemon");
}

function watchDaemonStream(
  command: DaemonCommand,
  onResponse: (response: DaemonResponse) => void,
  onError: (message: string) => void,
  onClose?: () => void,
): WatchHandle {
  const socketPath = daemonSocketPath();
  let stopped = false;
  let failed = false;
  const socket = createConnection(socketPath);
  const lines = createInterface({ input: socket });

  socket.on("connect", () => {
    socket.write(`${JSON.stringify(command)}\n`);
  });

  lines.on("line", (line) => {
    if (!line.trim()) {
      return;
    }
    try {
      onResponse(JSON.parse(line) as DaemonResponse);
    } catch (error) {
      failed = true;
      onError(asErrorMessage(error));
    }
  });

  socket.on("error", (error) => {
    if (stopped) {
      return;
    }
    failed = true;
    onError(asErrorMessage(error));
  });

  socket.on("close", () => {
    lines.close();
    if (!stopped && !failed) {
      onClose?.();
    }
  });

  return {
    stop: () => {
      stopped = true;
      socket.destroy();
    },
  };
}

async function sendDaemonCommand(command: DaemonCommand): Promise<DaemonResponse> {
  const socket = await connectSocket(daemonSocketPath());
  const lines = createInterface({ input: socket });

  try {
    return await new Promise<DaemonResponse>((resolve, reject) => {
      let settled = false;

      const finish = (fn: () => void): void => {
        if (settled) {
          return;
        }
        settled = true;
        fn();
      };

      lines.once("line", (line) => {
        finish(() => {
          try {
            resolve(JSON.parse(line) as DaemonResponse);
          } catch (error) {
            reject(error);
          }
        });
      });

      socket.once("error", (error) => {
        finish(() => reject(error));
      });

      socket.once("close", () => {
        finish(() => reject(new Error("daemon closed connection without response")));
      });

      socket.write(`${JSON.stringify(command)}\n`);
    });
  } finally {
    lines.close();
    socket.end();
    socket.destroy();
  }
}

async function connectSocket(socketPath: string): Promise<Socket> {
  if (!existsSync(socketPath)) {
    throw new Error(
      `cannot connect to brrmmmm daemon at ${socketPath}\nrun \`brrmmmm daemon start\` first`,
    );
  }

  return await new Promise<Socket>((resolve, reject) => {
    const socket = createConnection(socketPath);
    const onError = (error: Error): void => {
      socket.removeListener("connect", onConnect);
      reject(error);
    };
    const onConnect = (): void => {
      socket.removeListener("error", onError);
      resolve(socket);
    };
    socket.once("error", onError);
    socket.once("connect", onConnect);
  });
}

function asErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
