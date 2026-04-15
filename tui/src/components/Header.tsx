import React from "react";
import { Box, Text } from "ink";
import { type SidecarDescribe } from "../types.js";

interface Props {
  wasmPath: string;
  abiVersion: number;
  describe: SidecarDescribe | null;
}

export function Header({ wasmPath, abiVersion, describe }: Props) {
  const name = describe?.name ?? wasmPath.split("/").pop() ?? wasmPath;
  const desc = describe?.description ?? "v1 sidecar (no manifest)";
  const modes = describe?.run_modes?.join(", ") ?? "legacy";

  return (
    <Box
      borderStyle="round"
      borderColor="cyan"
      paddingX={1}
      flexDirection="row"
      justifyContent="space-between"
    >
      <Box flexDirection="column">
        <Text bold color="cyan">
          {name}
        </Text>
        <Text dimColor>{desc}</Text>
      </Box>
      <Box flexDirection="column" alignItems="flex-end">
        <Text dimColor>
          ABI v{abiVersion}{"  "}
          <Text color="yellow">{modes}</Text>
        </Text>
        <Text dimColor>{wasmPath}</Text>
      </Box>
    </Box>
  );
}
