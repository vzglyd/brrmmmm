import React from "react";
import { Box } from "ink";
import { type TuiState } from "../types.js";
import { ArtifactPane } from "./ArtifactPane.js";
import { PipeAnimation } from "./PipeAnimation.js";

interface Props {
  artifacts: TuiState["artifacts"];
  cycleCount: number;
}

export function ArtifactRow({ artifacts, cycleCount }: Props) {
  return (
    <Box flexDirection="row">
      <Box width="38%">
        <ArtifactPane title="RAW" artifact={artifacts.raw} />
      </Box>
      <Box width="24%" justifyContent="center" alignItems="center">
        <PipeAnimation
          publishedReceivedAt={artifacts.published?.received_at_ms ?? null}
          cycleCount={cycleCount}
        />
      </Box>
      <Box width="38%">
        <ArtifactPane title="OUTPUT" artifact={artifacts.published} />
      </Box>
    </Box>
  );
}
