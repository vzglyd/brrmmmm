import React from "react";
import { Box, Text } from "ink";
import { type ArtifactView } from "../types.js";

interface Props {
  title: string;
  artifact: ArtifactView | null;
}

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

function truncate(text: string, maxLines: number): string {
  const lines = text.split("\n");
  if (lines.length <= maxLines) return text;
  return lines.slice(0, maxLines).join("\n") + `\n… (${lines.length - maxLines} more lines)`;
}

export function ArtifactPane({ title, artifact }: Props) {
  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold color="cyan">
        {title}
      </Text>
      {!artifact ? (
        <Text dimColor>No {title.toLowerCase()} data yet</Text>
      ) : (
        <Box flexDirection="column">
          <Text dimColor>
            {artifact.size_bytes}B —{" "}
            {new Date(artifact.received_at_ms).toISOString().slice(11, 19)}
          </Text>
          <Text wrap="truncate">
            {truncate(prettyJson(artifact.preview), 10)}
          </Text>
        </Box>
      )}
    </Box>
  );
}
