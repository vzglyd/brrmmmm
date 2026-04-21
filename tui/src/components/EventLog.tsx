import React from "react";
import { Box, Text } from "ink";

interface Props {
  logs: string[];
}

export function EventLog({ logs }: Props) {
  const recent = logs.slice(-4);

  return (
    <Box borderStyle="single" borderColor="gray" flexDirection="column" paddingX={1}>
      <Text dimColor bold>
        FLIGHT LOG
      </Text>
      {recent.length === 0 ? (
        <Text dimColor>No log messages yet</Text>
      ) : (
        recent.map((line, i) => (
          <Text key={i} dimColor wrap="truncate">
            {line}
          </Text>
        ))
      )}
    </Box>
  );
}
