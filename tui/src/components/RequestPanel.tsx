import React, { useEffect, useMemo, useState } from "react";
import { Box, Text, useInput } from "ink";
import { type LastRequestView, type ArtifactView, type SidecarDescribe } from "../types.js";
import { formatBytes, formatDuration, formatLocalTime } from "../format.js";

interface Props {
  request: LastRequestView | null;
  requests: LastRequestView[];
  artifacts: {
    raw: ArtifactView | null;
    normalized: ArtifactView | null;
    published: ArtifactView | null;
  };
  describe: SidecarDescribe | null;
  isFocused: boolean;
  height: number;
}

const AMBER = "#FFB300";

interface PipelineRow {
  key: string;
  node: React.ReactNode;
}

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

export function RequestPanel({ request, requests, artifacts, describe, isFocused, height }: Props) {
  const hasAnyActivity =
    requests.length > 0 ||
    request !== null ||
    artifacts.raw !== null ||
    artifacts.normalized !== null ||
    artifacts.published !== null;
  const visibleRows = Math.max(1, height - 3);
  const [scrollTop, setScrollTop] = useState(0);

  const rows = useMemo<PipelineRow[]>(() => {
    if (!hasAnyActivity) {
      return [{ key: "waiting", node: <Text dimColor wrap="truncate">Waiting for first cycle...</Text> }];
    }

    const content: PipelineRow[] = [];
    if (describe) {
      content.push({
        key: "contract",
        node: (
          <Text dimColor wrap="truncate">
            contract: {describe.run_modes.join(", ") || "legacy"} · {formatPollStrategy(describe)} · nice sleep
          </Text>
        ),
      });
    }

    for (const item of requests) {
      content.push({
        key: `${item.request_id}:title`,
        node: (
          <Text bold color={AMBER} wrap="truncate">
            ### Request {item.sequence}: {item.kind === "https_get" ? "https_get" : item.kind} {formatRequestStatus(item)}
          </Text>
        ),
      });
      content.push({
        key: `${item.request_id}:target`,
        node: <Text dimColor wrap="truncate">{clip(`${item.host}${item.path ?? ""}`, 72)}</Text>,
      });
    }

    if (artifacts.raw) {
      content.push({
        key: "artifact:raw",
        node: (
          <Text dimColor wrap="truncate">
            {"  "}raw_source_payload {formatBytes(artifacts.raw.size_bytes)} · {formatLocalTime(artifacts.raw.received_at_ms)}
          </Text>
        ),
      });
    }

    if (artifacts.normalized) {
      content.push({
        key: "artifact:normalized",
        node: (
          <Text dimColor wrap="truncate">
            {"  "}normalized_payload {formatBytes(artifacts.normalized.size_bytes)} · {formatLocalTime(artifacts.normalized.received_at_ms)}
          </Text>
        ),
      });
    }

    if (artifacts.published) {
      content.push({
        key: "artifact:published",
        node: (
          <Text wrap="truncate">
            <Text color={AMBER} bold>publish_output</Text>
            <Text dimColor>
              {" "}({formatBytes(artifacts.published.size_bytes)}) - {formatLocalTime(artifacts.published.received_at_ms)}
            </Text>
          </Text>
        ),
      });
    }

    if (!request && describe && describe.capabilities_needed.length > 0) {
      content.push({
        key: "capabilities",
        node: <Text dimColor wrap="truncate">capabilities: {describe.capabilities_needed.join(", ")}</Text>,
      });
    }

    return content;
  }, [artifacts, describe, hasAnyActivity, request, requests]);

  const maxScroll = Math.max(0, rows.length - visibleRows);
  const safeTop = Math.min(scrollTop, maxScroll);
  const visibleContent = rows.slice(safeTop, safeTop + visibleRows);
  const scrollbar = buildVScrollbar(safeTop, rows.length, visibleRows);
  const scrollHint =
    rows.length > visibleRows
      ? ` ${safeTop > 0 ? "↑" : " "}${safeTop + visibleRows < rows.length ? "↓" : " "} ${safeTop + 1}-${Math.min(safeTop + visibleRows, rows.length)}/${rows.length}`
      : "";

  useEffect(() => {
    setScrollTop((top) => Math.min(top, maxScroll));
  }, [maxScroll]);

  useInput(
    (_, key) => {
      if (key.upArrow) setScrollTop((top) => Math.max(0, top - 1));
      if (key.downArrow) setScrollTop((top) => Math.min(maxScroll, top + 1));
      if (key.pageUp) setScrollTop((top) => Math.max(0, top - visibleRows));
      if (key.pageDown) setScrollTop((top) => Math.min(maxScroll, top + visibleRows));
    },
    { isActive: isFocused },
  );

  return (
    <Box
      borderStyle="single"
      borderColor={isFocused ? AMBER : "gray"}
      flexDirection="column"
      paddingX={1}
      height={height}
      flexShrink={0}
      overflow="hidden"
    >
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color={AMBER}>Pipeline</Text>
        {scrollHint && <Text dimColor>{scrollHint}</Text>}
      </Box>

      <Box flexDirection="row" overflow="hidden">
        <Box flexDirection="column" flexGrow={1} overflow="hidden">
          {visibleContent.map((row) => (
            <React.Fragment key={row.key}>{row.node}</React.Fragment>
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

function formatPollStrategy(describe: SidecarDescribe): string {
  const strategy = describe.poll_strategy;
  if (!strategy) return "freshness unspecified";
  switch (strategy.kind) {
    case "fixed_interval":
      return `fresh every ${formatDuration(strategy.interval_secs * 1000)}`;
    case "exponential_backoff":
      return `backoff ${formatDuration(strategy.base_secs * 1000)}-${formatDuration(strategy.max_secs * 1000)}`;
    case "jittered":
      return `fresh every ${formatDuration(strategy.base_secs * 1000)} + jitter`;
  }
}

function formatRequestStatus(item: LastRequestView): string {
  if (item.pending) return "pending";
  if (item.error) return `ERR ${item.error}`;
  const elapsed = item.elapsed_ms !== undefined ? ` ${item.elapsed_ms}ms` : "";
  const size = item.response_size_bytes ? ` ${formatBytes(item.response_size_bytes)}` : "";
  return `${item.status_code ?? "?"}${elapsed}${size}`;
}

function clip(value: string, maxLength: number): string {
  if (value.length <= maxLength) return value;
  if (maxLength <= 3) return value.slice(0, maxLength);
  return `${value.slice(0, maxLength - 3)}...`;
}
