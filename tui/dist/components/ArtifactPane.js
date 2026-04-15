import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
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
export function ArtifactPane({ title, artifact }) {
    return (_jsxs(Box, { borderStyle: "single", flexDirection: "column", paddingX: 1, flexGrow: 1, children: [_jsx(Text, { bold: true, color: "cyan", children: title }), !artifact ? (_jsxs(Text, { dimColor: true, children: ["No ", title.toLowerCase(), " data yet"] })) : (_jsxs(Box, { flexDirection: "column", children: [_jsxs(Text, { dimColor: true, children: [artifact.size_bytes, "B \u2014", " ", new Date(artifact.received_at_ms).toISOString().slice(11, 19)] }), _jsx(Text, { wrap: "truncate", children: truncate(prettyJson(artifact.preview), 10) })] }))] }));
}
