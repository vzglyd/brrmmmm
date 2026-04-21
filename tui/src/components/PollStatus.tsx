import React from "react";
import { Box, Text } from "ink";
import { type MissionOutcomeView, type MissionPhase, type PollStrategy } from "../types.js";
import { useCountdown } from "../hooks/useCountdown.js";
import { formatDuration, formatLocalTime } from "../format.js";

interface Props {
  phase: MissionPhase;
  sleepUntilMs: number | null;
  lastSuccessAt: string | null;
  consecutiveFailures: number;
  backoffMs: number | null;
  pollStrategy?: PollStrategy;
  persistenceAuthority?: string;
  missionOutcome?: MissionOutcomeView | null;
}

function strategyLabel(s?: PollStrategy): string {
  if (!s) return "unknown";
  switch (s.kind) {
    case "fixed_interval":
      return `every ${formatDuration(s.interval_secs * 1000)}`;
    case "exponential_backoff":
      return `backoff ${formatDuration(s.base_secs * 1000)}-${formatDuration(s.max_secs * 1000)}`;
    case "jittered":
      return `jittered +/-${formatDuration(s.jitter_secs * 1000)} @ ${formatDuration(s.base_secs * 1000)}`;
  }
}

function phaseColor(phase: MissionPhase): string {
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

function phaseLabel(phase: MissionPhase): string {
  return phase.replace(/_/g, " ");
}

function riskPostureColor(posture: string): string {
  switch (posture) {
    case "nominal":
      return "green";
    case "degraded":
      return "yellow";
    case "awaiting_operator":
      return "cyan";
    case "awaiting_changed_conditions":
      return "magenta";
    case "closed_safe":
      return "gray";
    default:
      return "white";
  }
}

export function PollStatus({
  phase,
  sleepUntilMs,
  lastSuccessAt,
  consecutiveFailures,
  backoffMs,
  pollStrategy,
  persistenceAuthority,
  missionOutcome,
}: Props) {
  const countdown = useCountdown(sleepUntilMs);
  const isSleeping = sleepUntilMs !== null && countdown !== "" && countdown !== "0s";

  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold>Mission Status</Text>

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
          <Text color="#FFB300" bold>
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
          <Text>{formatLocalTime(lastSuccessAt)}</Text>
        </Box>
      )}

      {consecutiveFailures > 0 && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>failures:</Text>
          <Text color="red">{consecutiveFailures}</Text>
          {backoffMs !== null && (
            <Text dimColor> (backoff {formatDuration(backoffMs)})</Text>
          )}
        </Box>
      )}

      {missionOutcome && (
        <>
          <Box flexDirection="row" gap={1}>
            <Text dimColor>risk posture:</Text>
            <Text color={riskPostureColor(missionOutcome.risk_posture)}>
              {missionOutcome.risk_posture.replace(/_/g, " ")}
            </Text>
          </Box>
          <Box flexDirection="row" gap={1}>
            <Text dimColor>next policy:</Text>
            <Text>{missionOutcome.next_attempt_policy.replace(/_/g, " ")}</Text>
          </Box>
          {missionOutcome.rescue_window_open === true && missionOutcome.escalation_deadline && (
            <Box flexDirection="row" gap={1}>
              <Text color="#FFB300" bold>rescue window open until</Text>
              <Text color="#FFB300">{missionOutcome.escalation_deadline}</Text>
            </Box>
          )}
          {missionOutcome.rescue_window_open === false && (
            <Box flexDirection="row" gap={1}>
              <Text dimColor>rescue window expired</Text>
            </Box>
          )}
        </>
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
