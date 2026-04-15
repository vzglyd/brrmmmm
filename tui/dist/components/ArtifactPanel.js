import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useState } from "react";
import { Box, Text, useInput } from "ink";
const TABS = [
    { key: "published", label: "OUTPUT" },
    { key: "raw", label: "RAW" },
    { key: "normalized", label: "NORM" },
];
function prettyJson(raw) {
    try {
        return JSON.stringify(JSON.parse(raw), null, 2);
    }
    catch {
        return raw;
    }
}
function truncate(text, maxLines) {
    const lines = text.split("\n");
    if (lines.length <= maxLines)
        return text;
    return lines.slice(0, maxLines).join("\n") + `\n… (${lines.length - maxLines} more lines)`;
}
export function ArtifactPanel({ artifacts }) {
    const [activeTab, setActiveTab] = useState("published");
    useInput((input) => {
        if (input === "\t") {
            setActiveTab((t) => {
                const idx = TABS.findIndex((x) => x.key === t);
                return TABS[(idx + 1) % TABS.length].key;
            });
        }
    });
    const artifact = artifacts[activeTab];
    return (_jsxs(Box, { borderStyle: "single", flexDirection: "column", paddingX: 1, flexGrow: 1, children: [_jsxs(Box, { flexDirection: "row", gap: 1, children: [TABS.map(({ key, label }) => (_jsx(Box, { borderStyle: activeTab === key ? "single" : undefined, borderColor: "cyan", paddingX: 1, children: _jsx(Text, { color: activeTab === key ? "cyan" : "gray", bold: activeTab === key, children: label }) }, key))), _jsx(Text, { dimColor: true, children: "  (Tab to switch)" })] }), !artifact ? (_jsxs(Text, { dimColor: true, children: ["No ", activeTab, " data yet"] })) : (_jsxs(Box, { flexDirection: "column", children: [_jsxs(Text, { dimColor: true, children: [artifact.size_bytes, "B \u2014 ", new Date(artifact.received_at_ms).toISOString().slice(11, 19)] }), _jsx(Text, { wrap: "truncate", children: truncate(prettyJson(artifact.preview), 12) })] }))] }));
}
