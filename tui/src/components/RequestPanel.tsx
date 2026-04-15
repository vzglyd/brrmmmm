import React from "react";
import { Box, Text } from "ink";
import { type LastRequestView, type ArtifactView, type SidecarDescribe } from "../types.js";

interface Props {
  request: LastRequestView | null;
  artifacts: {
    raw: ArtifactView | null;
    normalized: ArtifactView | null;
    published: ArtifactView | null;
  };
  describe: SidecarDescribe | null;
}

const AMBER = "#FFB300";

function statusColor(code?: number): string {
  if (!code) return "white";
  if (code < 300) return AMBER;
  if (code < 400) return "yellow";
  return "red";
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  return `${(bytes / 1024).toFixed(1)}KB`;
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return d.toTimeString().slice(0, 8);
}

export function RequestPanel({ request, artifacts, describe }: Props) {
  const hasAnyActivity = request !== null || artifacts.raw !== null || artifacts.published !== null;

  return (
    <Box borderStyle="single" borderColor="gray" flexDirection="column" paddingX={1}>
      <Text bold color={AMBER}>Pipeline</Text>
      {!hasAnyActivity ? (
        <Text dimColor>Waiting for first cycle…</Text>
      ) : (
        <Box flexDirection="column">
          {/* HTTP call */}
          {request && (
            <Box flexDirection="row" gap={1}>
              <Text color={AMBER} bold>
                {request.kind === "https_get" ? "https_get" : request.kind}
              </Text>
              <Text dimColor>
                ("{request.host}"
                {request.path ? `, "${request.path}"` : ""})
              </Text>
              {request.pending ? (
                <Text color="yellow"> ⠿ pending…</Text>
              ) : request.error ? (
                <Text color="red"> → ERR {request.error}</Text>
              ) : (
                <Text color={statusColor(request.status_code)}>
                  {" → "}
                  {request.status_code ?? "?"}{" "}
                  {request.elapsed_ms !== undefined ? `${request.elapsed_ms}ms` : ""}
                  {request.response_size_bytes ? ` ${formatBytes(request.response_size_bytes)}` : ""}
                </Text>
              )}
            </Box>
          )}

          {/* raw_source_payload artifact */}
          {artifacts.raw && (
            <Box flexDirection="row" gap={1}>
              <Text dimColor>  raw_source_payload</Text>
              <Text dimColor>
                {formatBytes(artifacts.raw.size_bytes)} · {formatTime(artifacts.raw.received_at_ms)}
              </Text>
            </Box>
          )}

          {/* normalized_payload artifact */}
          {artifacts.normalized && (
            <Box flexDirection="row" gap={1}>
              <Text dimColor>  normalized_payload</Text>
              <Text dimColor>
                {formatBytes(artifacts.normalized.size_bytes)} · {formatTime(artifacts.normalized.received_at_ms)}
              </Text>
            </Box>
          )}

          {/* publish_output call */}
          {artifacts.published && (
            <Box flexDirection="row" gap={1}>
              <Text color={AMBER} bold>publish_output</Text>
              <Text dimColor>
                ({formatBytes(artifacts.published.size_bytes)})
                {" → "}
                {formatTime(artifacts.published.received_at_ms)}
              </Text>
            </Box>
          )}

          {/* Capabilities hint from manifest — shown only before first activity */}
          {!request && describe && describe.capabilities_needed.length > 0 && (
            <Text dimColor>
              capabilities: {describe.capabilities_needed.join(", ")}
            </Text>
          )}
        </Box>
      )}
    </Box>
  );
}
