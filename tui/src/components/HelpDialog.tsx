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
    "Read published_output as the payload your code consumes.",
    "raw_source_payload is the upstream response; normalized_payload is intermediate.",
    "For scripts: brrmmmm run mission-module.wasm --once > payload.json",
    "Then parse payload.json with the payload type expected by your slide or app.",
    "For hosts: implement WASI preview1 plus brrmmmm_host, run the entry point,",
    "and read channel_push or artifact_publish(\"published_output\", bytes).",
    "Do not scrape the TUI; it is a debugger for the same event stream.",
    "",
    "# Current Contract",
    `Declared modes: ${manifestModes}`,
    `Declared params: ${params}`,
    `Declared artifacts: ${payloads}`,
    "Timestamps in the TUI are local clock time.",
    "",
    "# Runtime Modes",
    "v1_legacy: no reliable manifest. Provide env/params externally.",
    "  Treat output as an opaque JSON payload and validate it in your consumer.",
    "managed_polling: the module declares params, artifacts, polling, and cooldown.",
    "  Use describe/inspect as the contract and consume published_output.",
    "  Let host sleep/force-refresh imports control the cadence.",
    "interactive: params may change while the process is alive.",
    "  Read params_len/params_read each cycle and react without restarting.",
    "",
    "# CLI Modes",
    "Default TUI: inspect params, polling state, requests, artifacts, and logs.",
    "--once: run until the first published payload, print JSON to stdout, exit.",
    "daemon run: keep the sidecar alive and let it poll until Ctrl+C.",
    "inspect: print the module contract for tooling and code review.",
    "validate: confirm the WASM loads and required exports resolve.",
    "",
    "# Keys",
    "Tab changes focus between params, pipeline, raw, and output.",
    "In params: Up/Down chooses a field; typing edits its value.",
    "Outside params: f sends params and forces a refresh.",
    "In pipeline/raw/output: Up/Down/PgUp/PgDn scrolls focused content.",
    "In raw/output: Left/Right scrolls wide lines.",
    "? opens help from anywhere; h opens help outside params; q quits outside params.",
  ];
}
