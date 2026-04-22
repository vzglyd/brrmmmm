import React, { useEffect, useMemo, useState } from "react";
import { Box, Text, useInput } from "ink";
import { type ModuleDescribe } from "../types.js";

interface Props {
  describe: ModuleDescribe | null;
  height: number;
}

const AMBER = "#FFB300";

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

export function HelpDialog({ describe, height }: Props) {
  const [scrollTop, setScrollTop] = useState(0);
  const visibleRows = Math.max(1, height - 4);

  const lines = useMemo(() => buildHelpLines(describe), [describe]);
  const maxScroll = Math.max(0, lines.length - visibleRows);
  const safeTop = Math.min(scrollTop, maxScroll);
  const visibleLines = lines.slice(safeTop, safeTop + visibleRows);
  const scrollbar = buildVScrollbar(safeTop, lines.length, visibleRows);

  useEffect(() => {
    setScrollTop((top) => Math.min(top, maxScroll));
  }, [maxScroll]);

  useInput((_, key) => {
    if (key.upArrow) setScrollTop((top) => Math.max(0, top - 1));
    if (key.downArrow) setScrollTop((top) => Math.min(maxScroll, top + 1));
    if (key.pageUp) setScrollTop((top) => Math.max(0, top - visibleRows));
    if (key.pageDown) setScrollTop((top) => Math.min(maxScroll, top + visibleRows));
  });

  const scrollHint =
    lines.length > visibleRows
      ? ` ${safeTop + 1}-${Math.min(safeTop + visibleRows, lines.length)}/${lines.length}`
      : "";

  return (
    <Box
      borderStyle="round"
      borderColor={AMBER}
      flexDirection="column"
      paddingX={1}
      height={height}
      overflow="hidden"
    >
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color={AMBER}>Help</Text>
        <Text dimColor>Up/Down/PgUp/PgDn scroll · h/?/Esc close{scrollHint}</Text>
      </Box>

      <Box flexDirection="row" overflow="hidden">
        <Box flexDirection="column" flexGrow={1} overflow="hidden">
          {visibleLines.map((line, index) => (
            <Text
              key={`${safeTop + index}:${line}`}
              color={line.startsWith("# ") ? AMBER : undefined}
              bold={line.startsWith("# ")}
              dimColor={line === "" || line.startsWith("  ")}
              wrap="truncate"
            >
              {line === "" ? " " : line.replace(/^# /, "")}
            </Text>
          ))}
        </Box>
        <Box flexDirection="column" width={1}>
          {scrollbar.map((ch, i) => (
            <Text key={i} dimColor={ch === "░"} color={ch === "█" ? AMBER : undefined}>
              {ch}
            </Text>
          ))}
        </Box>
      </Box>
    </Box>
  );
}

function buildHelpLines(describe: ModuleDescribe | null): string[] {
  const manifestModes = describe?.run_modes?.length
    ? describe.run_modes.join(", ")
    : "not declared yet";
  const payloads = describe?.artifact_types?.length
    ? describe.artifact_types.join(", ")
    : "published_output";
  const params = describe?.params?.fields?.length
    ? describe.params.fields.map((field) => field.key).join(", ")
    : "none declared";

  return [
    "# Use The Output",
    "brrmmmm is a runner / sidecar, not a library your app should embed.",
    "The stable consumer contract is the mission JSON written to disk.",
    "Daemon missions write ~/.brrmmmm/missions/<mission_name>/<mission_name>.status.json while running.",
    "Daemon missions write ~/.brrmmmm/missions/<mission_name>/<mission_name>.out.json for the latest finalized attempt.",
    "One-shot runs should use --result-path mission.json when another program needs the data.",
    "Downstream programs should watch or poll .status.json for progress and consume payload from .out.json.",
    "raw_source_payload is the upstream response; normalized_payload is intermediate.",
    "Do not scrape the TUI; it is an operator view over the same mission state.",
    "",
    "# Current Contract",
    `Declared modes: ${manifestModes}`,
    `Declared params: ${params}`,
    `Declared artifacts: ${payloads}`,
    "Timestamps in the TUI are local clock time.",
    "",
    "# Runtime Modes",
    "v1_legacy: no reliable manifest. Provide env/params externally.",
    "  Persist the mission record and validate payload in your watcher.",
    "managed_polling: the module declares params, artifacts, polling, and cooldown.",
    "  Use describe/inspect as the contract; consume the saved mission record, not the TUI.",
    "  Let the runner own sleep/force-refresh and file persistence.",
    "interactive: params may change while the process is alive.",
    "  The runner can update host-owned params while keeping the same mission identity.",
    "",
    "# CLI Modes",
    "Default TUI: open the daemon dashboard, select missions, and inspect live history.",
    "--once: useful for debugging; prefer --result-path when another program consumes the result.",
    "daemon run: keep the mission runner in the foreground; watchers should read .status.json and .out.json.",
    "inspect: print the module contract for tooling and code review.",
    "validate: confirm the WASM loads and required exports resolve.",
    "",
    "# Keys",
    "List view: ↑/↓ select mission · Enter open detail · l or n open arming panel.",
    "List view: f retry · Space hold/resume · x abort.",
    "Detail view: Esc or b back to list · Tab cycles pipeline/raw/output panels.",
    "Detail view: f retry · Space hold/resume · x abort · l or n open arming panel.",
    "Arming panel: Tab or f on WASM path field opens inline file browser.",
    "  File browser: ↑/↓ browse · Enter select file or enter dir · Esc cancel.",
    "  Tab moves between other fields; Left/Right switches ARM and CANCEL.",
    "In pipeline/raw/output: Up/Down/PgUp/PgDn scrolls focused content.",
    "In raw/output: Left/Right scrolls wide lines.",
    "? opens help · Esc closes dialogs · Ctrl+C or q quits.",
  ];
}
