import React from "react";
import { Box, Text } from "ink";
import { type MissionOutcomeView, type MissionPhase, type PollStrategy } from "../types.js";
import { useCountdown } from "../hooks/useCountdown.js";
import { formatDuration, formatLocalTime } from "../format.js";

interface Props {
  hasStarted: boolean;
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

const PHASE_SEQ: MissionPhase[] = ["idle", "fetching", "parsing", "publishing"];
const PHASE_SEQ_LABELS: Record<string, string> = {
  idle: "IDLE",
  fetching: "FETCH",
  parsing: "PARSE",
  publishing: "PUBLISH",
};

export function PollStatus({
  hasStarted,
  phase,
  sleepUntilMs,
  lastSuccessAt,
  consecutiveFailures,
  backoffMs,
  pollStrategy,
  persistenceAuthority,
  missionOutcome,
}: Props) {
  if (!hasStarted) {
    return (
      <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
        <Text bold>MISSION STATUS</Text>
        <Text dimColor>Waiting for daemon launch...</Text>
      </Box>
    );
  }

  const countdown = useCountdown(sleepUntilMs);
  const isSleeping = sleepUntilMs !== null && countdown !== "" && countdown !== "0s";
  const isOffSeq = !PHASE_SEQ.includes(phase);

  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold>MISSION STATUS</Text>

      {/* Phase sequence indicator */}
      <Box flexDirection="row" gap={0}>
        <Text dimColor>SEQ  </Text>
        {PHASE_SEQ.map((p, i) => (
          <React.Fragment key={p}>
            {i > 0 && <Text dimColor> → </Text>}
            <Text
              bold={p === phase}
              color={p === phase ? phaseColor(p) : undefined}
              dimColor={p !== phase}
            >
              {PHASE_SEQ_LABELS[p]}
            </Text>
          </React.Fragment>
        ))}
      </Box>

      {/* Off-sequence phase status (cooling_down, failed) */}
      {isOffSeq && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>STATE</Text>
          <Text color={phaseColor(phase)}>{phase.replace(/_/g, " ").toUpperCase()}</Text>
        </Box>
      )}

      <Box flexDirection="row" gap={1}>
        <Text dimColor>STRATEGY</Text>
        <Text>{strategyLabel(pollStrategy)}</Text>
      </Box>

      {isSleeping && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>NEXT WAKE</Text>
          <Text color="#FFB300" bold>
            {countdown}
          </Text>
        </Box>
      )}

      {lastSuccessAt && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>LAST SUCCESS</Text>
          <Text>{formatLocalTime(lastSuccessAt)}</Text>
        </Box>
      )}

      {consecutiveFailures > 0 && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>FAILURES</Text>
          <Text color="red">{consecutiveFailures}</Text>
          {backoffMs !== null && (
            <Text dimColor> (backoff {formatDuration(backoffMs)})</Text>
          )}
        </Box>
      )}

      {missionOutcome && (
        <>
          <Box flexDirection="row" gap={1}>
            <Text dimColor>RISK</Text>
            <Text color={riskPostureColor(missionOutcome.risk_posture)}>
              {missionOutcome.risk_posture.replace(/_/g, " ").toUpperCase()}
            </Text>
          </Box>
          <Box flexDirection="row" gap={1}>
            <Text dimColor>POLICY</Text>
            <Text>{missionOutcome.next_attempt_policy.replace(/_/g, " ")}</Text>
          </Box>
          {missionOutcome.rescue_window_open === true && missionOutcome.escalation_deadline && (
            <Box flexDirection="row" gap={1}>
              <Text color="#FFB300" bold>RESCUE WINDOW</Text>
              <Text color="#FFB300">{missionOutcome.escalation_deadline}</Text>
            </Box>
          )}
          {missionOutcome.rescue_window_open === false && (
            <Box flexDirection="row" gap={1}>
              <Text dimColor>RESCUE WINDOW EXPIRED</Text>
            </Box>
          )}
        </>
      )}

      {persistenceAuthority && (
        <Box flexDirection="row" gap={1}>
          <Text dimColor>PERSIST</Text>
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
