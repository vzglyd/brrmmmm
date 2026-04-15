import React from "react";
import { Box, Text } from "ink";
import { type PollStrategy, type SidecarPhase } from "../types.js";
import { useCountdown } from "../hooks/useCountdown.js";

interface Props {
  phase: SidecarPhase;
  sleepUntilMs: number | null;
  lastSuccessAt: string | null;
  consecutiveFailures: number;
  backoffMs: number | null;
  pollStrategy?: PollStrategy;
  persistenceAuthority?: string;
}

function strategyLabel(s?: PollStrategy): string {
  if (!s) return "unknown";
  switch (s.kind) {
    case "fixed_interval":
      return `every ${s.interval_secs}s`;
    case "exponential_backoff":
      return `backoff ${s.base_secs}s–${s.max_secs}s`;
    case "jittered":
      return `jittered ±${s.jitter_secs}s @ ${s.base_secs}s`;
  }
}

function phaseColor(phase: SidecarPhase): string {
  switch (phase) {
    case "fetching":
      return "yellow";
    case "publishing":
      return "green";
    case "failed":
      return "red";
    case "cooling_down":
      return "magenta";
    default:
      return "white";
  }
}

function phaseLabel(phase: SidecarPhase): string {
  return phase.replace(/_/g, " ");
}

export function PollStatus({
  phase,
  sleepUntilMs,
  lastSuccessAt,
  consecutiveFailures,
  backoffMs,
  pollStrategy,
  persistenceAuthority,
}: Props) {
  const countdown = useCountdown(sleepUntilMs);
  const isSleeping = sleepUntilMs !== null && countdown !== "" && countdown !== "00:00";

  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold>Polling Status</Text>

      <Box flexDirection="row" gap={1}>
        <Text dimColor>strategy:</Text>
        <Text>{strategyLabel(pollStrategy)}</Text>
      </Box>

      <Box flexDirection="row" gap={1}>
        <Text dimColor>phase:</Text>
        <Text color={phaseColor(phase)}>{phaseLabel(phase)}</Text>
      </Box>

      {isSleeping && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>next poll in:</Text>
          <Text color="cyan" bold>
            {countdown}
          </Text>
        </Box>
      )}

      {phase === "fetching" && (
        <Box flexDirection="row" gap={1}>
          <Text color="yellow">⠿ fetching...</Text>
        </Box>
      )}

      {lastSuccessAt && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>last success:</Text>
          <Text>{lastSuccessAt.slice(11, 19)}</Text>
        </Box>
      )}

      {consecutiveFailures > 0 && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>failures:</Text>
          <Text color="red">{consecutiveFailures}</Text>
          {backoffMs !== null && (
            <Text dimColor> (backoff {Math.round(backoffMs / 1000)}s)</Text>
          )}
        </Box>
      )}

      {persistenceAuthority && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>persistence:</Text>
          <Text
            color={
              persistenceAuthority === "vendor_backed"
                ? "green"
                : persistenceAuthority === "host_persisted"
                ? "yellow"
                : "gray"
            }
          >
            {persistenceAuthority.replace(/_/g, " ")}
          </Text>
        </Box>
      )}
    </Box>
  );
}
