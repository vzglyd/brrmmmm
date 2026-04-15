import React from "react";
import { Box, Text } from "ink";
import { type LastRequestView } from "../types.js";

interface Props {
  request: LastRequestView | null;
}

function statusColor(code?: number): string {
  if (!code) return "white";
  if (code < 300) return "green";
  if (code < 400) return "yellow";
  return "red";
}

function formatBytes(bytes?: number): string {
  if (!bytes) return "";
  if (bytes < 1024) return `${bytes}B`;
  return `${(bytes / 1024).toFixed(1)}KB`;
}

export function RequestPanel({ request }: Props) {
  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1}>
      <Text bold>Last Request</Text>
      {!request ? (
        <Text dimColor>No requests yet</Text>
      ) : (
        <Box flexDirection="row" gap={1} flexWrap="wrap">
          {request.pending ? (
            <Text color="yellow">⠿</Text>
          ) : request.status_code ? (
            <Box
              borderStyle="single"
              borderColor={statusColor(request.status_code)}
              paddingX={1}
            >
              <Text color={statusColor(request.status_code)} bold>
                {request.status_code}
              </Text>
            </Box>
          ) : (
            <Box borderStyle="single" borderColor="red" paddingX={1}>
              <Text color="red">ERR</Text>
            </Box>
          )}

          <Text bold>
            {request.kind === "https_get" ? "GET" : request.kind.toUpperCase()}
          </Text>
          <Text>
            {request.host}
            {request.path ?? ""}
          </Text>

          {!request.pending && request.elapsed_ms !== undefined && (
            <Text dimColor>
              {request.elapsed_ms}ms{" "}
              {request.response_size_bytes
                ? formatBytes(request.response_size_bytes)
                : ""}
            </Text>
          )}

          {request.error && (
            <Text color="red">{request.error}</Text>
          )}
        </Box>
      )}
    </Box>
  );
}
