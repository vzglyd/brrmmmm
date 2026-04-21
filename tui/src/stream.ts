import { existsSync, readFileSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

import { type BrrmmmmEvent } from "./types.js";

export type EventCallback = (event: BrrmmmmEvent) => void;
export type ExitCallback = (code: number | null) => void;

export interface StreamHandle {
  stop: () => void;
  sendCommand: (cmd: string) => void;
}

interface LaunchArgs {
  env: Record<string, string>;
  params?: string;
}

interface LaunchCommand {
  type: "launch";
  wasm: string;
  name?: string;
  env: Record<string, string>;
  params?: string;
}

type DaemonCommand =
  | LaunchCommand
  | { type: "abort"; mission: string; reason: string }
  | { type: "status" }
  | { type: "watch"; mission: string };

type DaemonResponse =
  | { type: "launched"; mission: string }
  | { type: "ok"; mission: string }
  | { type: "error"; message: string }
  | { type: "status"; missions: Array<{ name: string }> }
  | { type: "event"; mission: string; line: string };

/**
 * Connect to the daemon socket, launch a mission, and stream its NDJSON events.
 * The `f` command relaunches the mission with the latest in-memory params.
 */
export function spawnEventStream(
  _rustBin: string,
  wasmPath: string,
  extraArgs: string[],
  onEvent: EventCallback,
  onExit: ExitCallback
): StreamHandle {
  const socketPath = daemonSocketPath();
  const launchArgs = parseLaunchArgs(extraArgs);
  let missionName: string | null = null;
  let latestParams = launchArgs.params;
  let stopped = false;
  let restarting = false;
  let watchSocket: Socket | null = null;
  let watchGeneration = 0;

  void bootstrap();

  return {
    stop: () => {
      stopped = true;
      closeWatch();
    },
    sendCommand: (cmd: string) => {
      if (cmd.startsWith("params_json ")) {
        latestParams = cmd.slice("params_json ".length).trim() || undefined;
        return;
      }
      if (cmd === "force") {
        void restartMission();
      }
    },
  };

  async function bootstrap(): Promise<void> {
    try {
      missionName = await launchMission();
      openWatch(missionName);
    } catch (error) {
      emitLog(asErrorMessage(error));
      onExit(1);
    }
  }

  async function restartMission(): Promise<void> {
    if (stopped || restarting || !missionName) {
      return;
    }
    restarting = true;
    const currentMission = missionName;
    closeWatch();
    try {
      await sendDaemonCommand(socketPath, {
        type: "abort",
        mission: currentMission,
        reason: "TUI refresh requested",
      }).catch(() => undefined);
      await waitForMissionAbsence(socketPath, currentMission);
      missionName = await launchMission(currentMission);
      openWatch(missionName);
    } catch (error) {
      emitLog(asErrorMessage(error));
      onExit(1);
    } finally {
      restarting = false;
    }
  }

  async function launchMission(name?: string): Promise<string> {
    const response = await sendDaemonCommand(socketPath, {
      type: "launch",
      wasm: wasmPath,
      name,
      env: launchArgs.env,
      params: latestParams,
    });
    if (response.type === "launched") {
      return response.mission;
    }
    if (response.type === "error") {
      throw new Error(response.message);
    }
    throw new Error("unexpected response from daemon launch");
  }

  function openWatch(mission: string): void {
    const generation = ++watchGeneration;
    const socket = createConnection(socketPath);
    watchSocket = socket;
    const lines = createInterface({ input: socket });

    socket.on("connect", () => {
      socket.write(`${JSON.stringify({ type: "watch", mission })}\n`);
    });

    lines.on("line", (line) => {
      if (generation !== watchGeneration || !line.trim()) {
        return;
      }
      let response: DaemonResponse;
      try {
        response = JSON.parse(line) as DaemonResponse;
      } catch {
        return;
      }
      if (response.type === "event") {
        try {
          onEvent(JSON.parse(response.line) as BrrmmmmEvent);
        } catch {
          // Ignore malformed daemon event payloads.
        }
      } else if (response.type === "error") {
        emitLog(`daemon watch error: ${response.message}`);
        onExit(1);
      }
    });

    socket.on("error", (error) => {
      if (!stopped && generation === watchGeneration) {
        emitLog(`daemon socket error: ${error.message}`);
        onExit(1);
      }
    });

    socket.on("close", () => {
      lines.close();
      if (!stopped && generation === watchGeneration && !restarting) {
        onExit(0);
      }
    });
  }

  function closeWatch(): void {
    watchGeneration += 1;
    watchSocket?.destroy();
    watchSocket = null;
  }

  function emitLog(message: string): void {
    onEvent({
      type: "log",
      ts: new Date().toISOString(),
      message,
    });
  }
}

async function sendDaemonCommand(
  socketPath: string,
  command: DaemonCommand
): Promise<DaemonResponse> {
  const socket = await connectSocket(socketPath);
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
      `cannot connect to brrmmmm daemon at ${socketPath}\nhint: run \`brrmmmm daemon start\``
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

async function waitForMissionAbsence(
  socketPath: string,
  mission: string
): Promise<void> {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const response = await sendDaemonCommand(socketPath, { type: "status" });
    if (
      response.type === "status" &&
      !response.missions.some((entry) => entry.name === mission)
    ) {
      return;
    }
    await new Promise((resolve) => {
      setTimeout(resolve, 50);
    });
  }
  throw new Error(`mission '${mission}' did not leave the daemon registry`);
}

function parseLaunchArgs(extraArgs: string[]): LaunchArgs {
  const env: Record<string, string> = {};
  let params: string | undefined;

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
      index += 1;
      continue;
    }

    if (arg === "-f" || arg === "--params-file") {
      const path = extraArgs[index + 1];
      index += 1;
      if (path && existsSync(path)) {
        params = readFileSync(path, "utf8");
      }
    }
  }

  return params ? { env, params } : { env };
}

function daemonSocketPath(): string {
  const base = process.env["BRRMMMM_HOME"] ?? join(homedir(), ".brrmmmm");
  return join(base, "daemon.sock");
}

function asErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
