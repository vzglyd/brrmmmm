import React, { useState, useEffect } from "react";
import { Box, Text } from "ink";

type PipeStage = "idle" | "ingesting" | "processing" | "emitting";

interface Props {
  publishedReceivedAt: number | null;
  cycleCount: number;
}

const HALF = "━━━━━━━━━━";
const STEP_MS = 800;

export function PipeAnimation({ publishedReceivedAt, cycleCount }: Props) {
  const [stage, setStage] = useState<PipeStage>("idle");

  useEffect(() => {
    if (publishedReceivedAt === null) return;
    setStage("ingesting");
    const t1 = setTimeout(() => setStage("processing"), STEP_MS);
    const t2 = setTimeout(() => setStage("emitting"), STEP_MS * 2);
    const t3 = setTimeout(() => setStage("idle"), STEP_MS * 3);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
      clearTimeout(t3);
    };
  }, [publishedReceivedAt]);

  const AMBER = "#FFB300";

  // Top-left: red when ingesting or processing; amber when emitting; gray at idle.
  const tlColor =
    stage === "ingesting" || stage === "processing" ? "red"
    : stage === "emitting" ? AMBER
    : "gray";

  // Bottom-left: red when ingesting only; amber when processing or emitting; gray at idle.
  const blColor =
    stage === "ingesting" ? "red"
    : stage === "processing" || stage === "emitting" ? AMBER
    : "gray";

  // Right side: amber when emitting; gray otherwise.
  const rColor = stage === "emitting" ? AMBER : "gray";

  const icon =
    stage === "ingesting" ? "◀"
    : stage === "processing" ? "▶"
    : stage === "emitting" ? "◆"
    : "·";

  return (
    <Box flexDirection="column" alignItems="center" paddingY={1}>
      {/* Top half of pipe */}
      <Text>
        <Text color={tlColor}>{HALF}</Text>
        <Text color={tlColor}>{icon}</Text>
        <Text color={rColor}>{HALF}</Text>
      </Text>
      {/* Bottom half of pipe */}
      <Text>
        <Text color={blColor}>{HALF}</Text>
        <Text color={blColor}>{icon}</Text>
        <Text color={rColor}>{HALF}</Text>
      </Text>
      {/* Run counter */}
      {cycleCount > 0 && (
        <Text dimColor>{`(${cycleCount} run${cycleCount === 1 ? "" : "s"})`}</Text>
      )}
    </Box>
  );
}
