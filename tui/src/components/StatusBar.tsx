import React from "react";
import { Box, Text } from "ink";

interface Props {
  isRunning: boolean;
  error: string | null;
  focusPane: string;
  isHelpOpen: boolean;
}

export function StatusBar({ isRunning, error, focusPane, isHelpOpen }: Props) {
  return (
    <Box borderStyle="single" borderColor="gray" paddingX={1} justifyContent="space-between">
      <Text dimColor>
        {isHelpOpen
          ? "help | Up/Down/PgUp/PgDn scroll | h/?/Esc close | Ctrl+C quit"
          : `focus: ${focusPane} | Tab focus | ? help | type params | f relaunch outside params | q quit`}
      </Text>
      {error ? (
        <Text color="red">{error}</Text>
      ) : !isRunning ? (
        <Text color="yellow">MODULE STOPPED</Text>
      ) : null}
    </Box>
  );
}
