import React, { useState, useEffect, useRef } from "react";
import { Box, Text, useInput, measureElement, type DOMElement } from "ink";
import { type ArtifactView } from "../types.js";

interface Props {
  title: string;
  artifact: ArtifactView | null;
  isFocused: boolean;
}

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export function ArtifactPane({ title, artifact, isFocused }: Props) {
  const [scrollTop, setScrollTop] = useState(0);
  // Start with a reasonable guess; corrected after first layout measurement.
  const [visibleLines, setVisibleLines] = useState(10);
  const boxRef = useRef<DOMElement>(null);

  // Re-measure on every render so terminal resize is picked up automatically.
  // React will skip the re-render if the value hasn't changed.
  useEffect(() => {
    if (boxRef.current) {
      const { height } = measureElement(boxRef.current);
      // border top+bottom (2) + title row (1) + meta row (1) = 4 overhead
      setVisibleLines(Math.max(1, height - 4));
    }
  });

  const lines = artifact ? prettyJson(artifact.preview).split("\n") : [];
  const totalLines = lines.length;

  // Clamp scroll when content or viewport changes.
  const maxScroll = Math.max(0, totalLines - visibleLines);
  const safeScrollTop = Math.min(scrollTop, maxScroll);

  useInput(
    (_, key) => {
      if (key.upArrow) setScrollTop((t) => Math.max(0, t - 1));
      if (key.downArrow) setScrollTop((t) => Math.min(t + 1, maxScroll));
      if (key.pageUp) setScrollTop((t) => Math.max(0, t - visibleLines));
      if (key.pageDown) setScrollTop((t) => Math.min(t + visibleLines, maxScroll));
    },
    { isActive: isFocused },
  );

  const visibleContent = lines.slice(safeScrollTop, safeScrollTop + visibleLines);

  const scrollHint =
    totalLines > visibleLines
      ? ` ${safeScrollTop > 0 ? "↑" : " "}${safeScrollTop + visibleLines < totalLines ? "↓" : " "} ${safeScrollTop + 1}–${Math.min(safeScrollTop + visibleLines, totalLines)}/${totalLines}`
      : "";

  return (
    <Box
      ref={boxRef}
      borderStyle="single"
      borderColor={isFocused ? "#FFB300" : "gray"}
      flexDirection="column"
      paddingX={1}
      flexGrow={1}
      overflow="hidden"
    >
      {/* Title row */}
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color={isFocused ? "#FFB300" : "white"}>
          {title}
        </Text>
        {scrollHint !== "" && <Text dimColor>{scrollHint}</Text>}
      </Box>

      {/* Content */}
      {!artifact ? (
        <Text dimColor>No {title.toLowerCase()} data yet</Text>
      ) : (
        <Box flexDirection="column" overflow="hidden">
          <Text dimColor>
            {artifact.size_bytes}B —{" "}
            {new Date(artifact.received_at_ms).toISOString().slice(11, 19)}
          </Text>
          {visibleContent.map((line, i) => (
            <Text key={safeScrollTop + i} wrap="truncate">
              {line}
            </Text>
          ))}
        </Box>
      )}
    </Box>
  );
}
