import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
import { useCountdown } from "../hooks/useCountdown.js";
function strategyLabel(s) {
    if (!s)
        return "unknown";
    switch (s.kind) {
        case "fixed_interval":
            return `every ${s.interval_secs}s`;
        case "exponential_backoff":
            return `backoff ${s.base_secs}s–${s.max_secs}s`;
        case "jittered":
            return `jittered ±${s.jitter_secs}s @ ${s.base_secs}s`;
    }
}
function phaseColor(phase) {
    switch (phase) {
        case "fetching":
            return "yellow";
        case "publishing":
            return "green";
        case "failed":
            return "red";
        case "cooling_down":
            return "magenta";
        default:
            return "white";
    }
}
function phaseLabel(phase) {
    return phase.replace(/_/g, " ");
}
export function PollStatus({ phase, sleepUntilMs, lastSuccessAt, consecutiveFailures, backoffMs, pollStrategy, persistenceAuthority, }) {
    const countdown = useCountdown(sleepUntilMs);
    const isSleeping = sleepUntilMs !== null && countdown !== "" && countdown !== "00:00";
    return (_jsxs(Box, { borderStyle: "single", flexDirection: "column", paddingX: 1, flexGrow: 1, children: [_jsx(Text, { bold: true, children: "Polling Status" }), _jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "strategy:" }), _jsx(Text, { children: strategyLabel(pollStrategy) })] }), _jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "phase:" }), _jsx(Text, { color: phaseColor(phase), children: phaseLabel(phase) })] }), isSleeping && (_jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "next poll in:" }), _jsx(Text, { color: "cyan", bold: true, children: countdown })] })), phase === "fetching" && (_jsx(Box, { flexDirection: "row", gap: 1, children: _jsx(Text, { color: "yellow", children: "\u283F fetching..." }) })), lastSuccessAt && (_jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "last success:" }), _jsx(Text, { children: lastSuccessAt.slice(11, 19) })] })), consecutiveFailures > 0 && (_jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "failures:" }), _jsx(Text, { color: "red", children: consecutiveFailures }), backoffMs !== null && (_jsxs(Text, { dimColor: true, children: [" (backoff ", Math.round(backoffMs / 1000), "s)"] }))] })), persistenceAuthority && (_jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { dimColor: true, children: "persistence:" }), _jsx(Text, { color: persistenceAuthority === "vendor_backed"
                            ? "green"
                            : persistenceAuthority === "host_persisted"
                                ? "yellow"
                                : "gray", children: persistenceAuthority.replace(/_/g, " ") })] }))] }));
}
