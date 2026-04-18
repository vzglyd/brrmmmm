import { type LastRequestView, type SidecarDescribe } from "../types.js";
import { formatBytes, formatDuration } from "../format.js";

export function buildVScrollbar(
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

export function formatPollStrategy(describe: SidecarDescribe): string {
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

export function formatRequestStatus(item: LastRequestView): string {
  if (item.pending) return "pending";
  if (item.error) return `ERR ${item.error}`;
  const elapsed = item.elapsed_ms !== undefined ? ` ${item.elapsed_ms}ms` : "";
  const size = item.response_size_bytes ? ` ${formatBytes(item.response_size_bytes)}` : "";
  return `${item.status_code ?? "?"}${elapsed}${size}`;
}

export function clip(value: string, maxLength: number): string {
  if (value.length <= maxLength) return value;
  if (maxLength <= 3) return value.slice(0, maxLength);
  return `${value.slice(0, maxLength - 3)}...`;
}
