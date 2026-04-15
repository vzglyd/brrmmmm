import React, { useState } from "react";
import { Box, useInput } from "ink";
import { type TuiState } from "../types.js";
import { ArtifactPane } from "./ArtifactPane.js";
import { PipeAnimation } from "./PipeAnimation.js";

interface Props {
  artifacts: TuiState["artifacts"];
  cycleCount: number;
}

export function ArtifactRow({ artifacts, cycleCount }: Props) {
  const [focusedPane, setFocusedPane] = useState<"raw" | "output">("output");

  useInput((input) => {
    if (input === "\t") {
      setFocusedPane((p) => (p === "raw" ? "output" : "raw"));
    }
  });

  return (
    <Box flexDirection="row" flexGrow={1}>
      <Box width="38%">
        <ArtifactPane
          title="RAW"
          artifact={artifacts.raw}
          isFocused={focusedPane === "raw"}
        />
      </Box>
      <Box width="24%" justifyContent="center" alignItems="center">
        <PipeAnimation
          publishedReceivedAt={artifacts.published?.received_at_ms ?? null}
          cycleCount={cycleCount}
        />
      </Box>
      <Box width="38%">
        <ArtifactPane
          title="OUTPUT"
          artifact={artifacts.published}
          isFocused={focusedPane === "output"}
        />
      </Box>
    </Box>
  );
}
