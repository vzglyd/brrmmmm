import React from "react";
import { Box, Text } from "ink";

interface Props {
  isRunning: boolean;
  error: string | null;
}

export function StatusBar({ isRunning, error }: Props) {
  return (
    <Box borderStyle="single" borderColor="gray" paddingX={1} justifyContent="space-between">
      <Text dimColor>q quit  •  Tab switch pane  •  ↑↓ PgUp/PgDn scroll  •  Ctrl+C stop sidecar</Text>
      {error ? (
        <Text color="red">{error}</Text>
      ) : !isRunning ? (
        <Text color="yellow">Sidecar stopped</Text>
      ) : null}
    </Box>
  );
}
