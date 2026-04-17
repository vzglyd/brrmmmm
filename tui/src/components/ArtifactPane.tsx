import React, { useState, useEffect, useRef } from "react";
import { Box, Text, useInput, measureElement, type DOMElement } from "ink";
import { type ArtifactView } from "../types.js";
import { formatBytes, formatLocalTime } from "../format.js";

interface Props {
  title: string;
  artifact: ArtifactView | null;
  isFocused: boolean;
}

const AMBER = "#FFB300";
const H_STEP = 8; // columns per left/right keypress

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

/**
 * Build a vertical scrollbar as one character per content row.
 * Returns spaces when the content fits without scrolling.
 */
function buildVScrollbar(
  scrollTop: number,
  totalLines: number,
  visibleLines: number,
): string[] {
  if (totalLines <= visibleLines || visibleLines <= 0) {
    return Array(visibleLines).fill(" ");
  }
  const thumbSize = Math.max(1, Math.round((visibleLines / totalLines) * visibleLines));
  const maxThumbTop = visibleLines - thumbSize;
  const thumbTop = Math.round((scrollTop / (totalLines - visibleLines)) * maxThumbTop);

  return Array.from({ length: visibleLines }, (_, i) =>
    i >= thumbTop && i < thumbTop + thumbSize ? "█" : "░",
  );
}

export function ArtifactPane({ title, artifact, isFocused }: Props) {
  const [scrollTop, setScrollTop] = useState(0);
  const [scrollLeft, setScrollLeft] = useState(0);
  const [visibleLines, setVisibleLines] = useState(10);
  const [visibleCols, setVisibleCols] = useState(40);
  const boxRef = useRef<DOMElement>(null);

  // Re-measure every render to pick up terminal resize.
  useEffect(() => {
    if (boxRef.current) {
      const { height, width } = measureElement(boxRef.current);
      // Overhead: border×2 + title row + meta row = 4 vertical rows
      setVisibleLines(Math.max(1, height - 4));
      // Overhead: border×2 + paddingX×2 + scrollbar col = 5 horizontal chars
      setVisibleCols(Math.max(1, width - 5));
    }
  });

  const rawLines = artifact ? prettyJson(artifact.preview).split("\n") : [];
  const totalLines = rawLines.length;
  const maxLineWidth = rawLines.reduce((m, l) => Math.max(m, l.length), 0);

  const maxScrollV = Math.max(0, totalLines - visibleLines);
  const maxScrollH = Math.max(0, maxLineWidth - visibleCols);

  const safeTop = Math.min(scrollTop, maxScrollV);
  const safeLeft = Math.min(scrollLeft, maxScrollH);

  useInput(
    (_, key) => {
      if (key.upArrow)    setScrollTop((t)  => Math.max(0, t - 1));
      if (key.downArrow)  setScrollTop((t)  => Math.min(t + 1, maxScrollV));
      if (key.pageUp)     setScrollTop((t)  => Math.max(0, t - visibleLines));
      if (key.pageDown)   setScrollTop((t)  => Math.min(t + visibleLines, maxScrollV));
      if (key.leftArrow)  setScrollLeft((l) => Math.max(0, l - H_STEP));
      if (key.rightArrow) setScrollLeft((l) => Math.min(l + H_STEP, maxScrollH));
    },
    { isActive: isFocused },
  );

  // Reset horizontal scroll when new artifact arrives.
  const prevKindRef = useRef<string | null>(null);
  useEffect(() => {
    const currentKind = artifact?.kind ?? null;
    if (currentKind !== prevKindRef.current) {
      setScrollTop(0);
      setScrollLeft(0);
      prevKindRef.current = currentKind;
    }
  }, [artifact?.kind]);

  // Slice vertical viewport, then horizontal viewport per line.
  const visibleContent = rawLines
    .slice(safeTop, safeTop + visibleLines)
    .map((line) => line.slice(safeLeft, safeLeft + visibleCols));

  const scrollbar = buildVScrollbar(safeTop, totalLines, visibleLines);

  // Title-row scroll indicator.
  const vHint =
    totalLines > visibleLines
      ? ` ${safeTop > 0 ? "↑" : " "}${safeTop + visibleLines < totalLines ? "↓" : " "} ${safeTop + 1}–${Math.min(safeTop + visibleLines, totalLines)}/${totalLines}`
      : "";
  const hHint = maxScrollH > 0 ? ` ←→ col ${safeLeft + 1}` : "";

  return (
    <Box
      ref={boxRef}
      borderStyle="single"
      borderColor={isFocused ? AMBER : "gray"}
      flexDirection="column"
      paddingX={1}
      flexGrow={1}
      overflow="hidden"
    >
      {/* Title row */}
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color={isFocused ? AMBER : "white"}>
          {title}
        </Text>
        {(vHint || hHint) && (
          <Text dimColor>{vHint}{hHint}</Text>
        )}
      </Box>

      {!artifact ? (
        <Text dimColor>No {title.toLowerCase()} data yet</Text>
      ) : (
        <>
          {/* Meta row */}
          <Text dimColor>
            {formatBytes(artifact.size_bytes)}
            {" - "}
            {formatLocalTime(artifact.received_at_ms)}
          </Text>

          {/* Content + scrollbar */}
          <Box flexDirection="row" overflow="hidden">
            {/* Lines */}
            <Box flexDirection="column" flexGrow={1} overflow="hidden">
              {visibleContent.map((line, i) => (
                <Text key={safeTop + i} wrap="truncate">
                  {line || " "}
                </Text>
              ))}
            </Box>
            {/* Vertical scrollbar column */}
            <Box flexDirection="column" width={1}>
              {scrollbar.map((ch, i) => (
                <Text key={i} dimColor={ch === "░"} color={ch === "█" ? AMBER : undefined}>
                  {ch}
                </Text>
              ))}
            </Box>
          </Box>
        </>
      )}
    </Box>
  );
}
