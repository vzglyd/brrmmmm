import React from "react";
import { Box, Text } from "ink";
import { type MergedEnvVar } from "../types.js";

const AMBER = "#FFB300";

interface Props {
  vars: MergedEnvVar[];
}

export function EnvPanel({ vars }: Props) {
  // Only show manifest-declared params (those with a description or required flag set by the spec).
  // Extra --env vars not in the manifest are still shown so devs know what's active.
  const declared = vars.filter((v) => v.required || v.description !== "");
  const extras = vars.filter((v) => !v.required && v.description === "" && v.set);

  return (
    <Box borderStyle="single" borderColor="gray" flexDirection="column" paddingX={1} flexGrow={1}>
      <Text bold color={AMBER}>Parameters</Text>
      {declared.length === 0 && extras.length === 0 ? (
        <Text dimColor>No parameters declared · use --env KEY=VALUE</Text>
      ) : (
        <>
          {declared.map((v) => (
            <Box key={v.name} flexDirection="row" gap={1}>
              <Text color={v.set ? AMBER : v.required ? "red" : "gray"}>
                {v.set ? "✓" : "✗"}
              </Text>
              <Text bold={v.required}>{v.name}</Text>
              {v.required && !v.set && <Text color="red">(required)</Text>}
              {v.description ? <Text dimColor>— {v.description}</Text> : null}
            </Box>
          ))}
          {extras.map((v) => (
            <Box key={v.name} flexDirection="row" gap={1}>
              <Text color={AMBER}>✓</Text>
              <Text dimColor>{v.name}</Text>
              <Text dimColor>— via --env</Text>
            </Box>
          ))}
        </>
      )}
    </Box>
  );
}
