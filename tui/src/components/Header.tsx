import React from "react";
import { Box, Text } from "ink";
import { type SidecarDescribe } from "../types.js";

// ── 8×8 bitmap pixel font (half-block encoded) ───────────────────────
//
// Each glyph is 8 pixel rows × 6 pixel columns. Pairs of pixel rows
// collapse into a single terminal row using Unicode half-block chars:
//   top=1 bot=0 → ▀   top=0 bot=1 → ▄   both=1 → █   neither → space
//
// Pixel bitmaps (MSB = leftmost, 6-bit rows):
//   b: 100000 111100 100110 100110 100110 111100 000000 000000
//   r: 111000 100100 100000 100000 100000 100000 000000 000000
//   m: 110110 101010 100010 100010 100010 100010 000000 000000
//
// (verified column-by-column; each glyph is exactly 6 chars wide)

const GLYPH: Record<string, readonly string[]> = {
  //         row0      row1      row2      row3
  b: ["█▄▄▄  ", "█  ██ ", "█▄▄█▀ ", "      "],
  r: ["█▀▀▄  ", "█     ", "█     ", "      "],
  m: ["█▀▄▀█ ", "█   █ ", "█   █ ", "      "],
};

const LOGO_ROWS: readonly string[] = [0, 1, 2].map((row) =>
  "brrmmmm"
    .split("")
    .map((ch) => GLYPH[ch]?.[row] ?? "      ")
    .join(""),
);

// ── Component ─────────────────────────────────────────────────────────

interface Props {
  wasmPath: string;
  abiVersion: number;
  describe: SidecarDescribe | null;
}

const AMBER = "#FFB300";

export function Header({ wasmPath, abiVersion, describe }: Props) {
  const name = describe?.name ?? wasmPath.split("/").pop() ?? wasmPath;
  const desc = describe?.description ?? "waiting for sidecar manifest";
  const modes = describe?.run_modes?.join(", ") ?? "starting";

  return (
    <Box
      borderStyle="round"
      borderColor={AMBER}
      paddingX={1}
      flexDirection="row"
      justifyContent="space-between"
      alignItems="center"
    >
      {/* Bitmap logo */}
      <Box flexDirection="column">
        {LOGO_ROWS.map((row, i) => (
          <Text key={i} color={AMBER}>
            {row}
          </Text>
        ))}
      </Box>

      {/* Sidecar info */}
      <Box flexDirection="column" alignItems="flex-end">
        <Text bold color={AMBER}>
          {name}
        </Text>
        <Text dimColor>{desc}</Text>
        <Text dimColor>
          ABI v{abiVersion}{"  "}
          <Text color={AMBER}>{modes}</Text>
        </Text>
        <Text dimColor>{wasmPath}</Text>
      </Box>
    </Box>
  );
}
