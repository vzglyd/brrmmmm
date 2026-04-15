import React from "react";
import { Box, Text } from "ink";
import { type MergedEnvVar } from "../types.js";

interface Props {
  vars: MergedEnvVar[];
}

export function EnvPanel({ vars }: Props) {
  return (
    <Box borderStyle="single" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold>Environment Variables</Text>
      {vars.length === 0 ? (
        <Text dimColor>No env vars declared</Text>
      ) : (
        vars.map((v) => (
          <Box key={v.name} flexDirection="row" gap={1}>
            <Text color={v.set ? "green" : v.required ? "red" : "yellow"}>
              {v.set ? "✓" : "✗"}
            </Text>
            <Text bold={v.required}>{v.name}</Text>
            {v.required && !v.set && <Text color="red"> (required)</Text>}
            {v.description ? (
              <Text dimColor> — {v.description}</Text>
            ) : null}
          </Box>
        ))
      )}
    </Box>
  );
}
