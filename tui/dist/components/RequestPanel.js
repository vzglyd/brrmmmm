import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
function statusColor(code) {
    if (!code)
        return "white";
    if (code < 300)
        return "green";
    if (code < 400)
        return "yellow";
    return "red";
}
function formatBytes(bytes) {
    if (!bytes)
        return "";
    if (bytes < 1024)
        return `${bytes}B`;
    return `${(bytes / 1024).toFixed(1)}KB`;
}
export function RequestPanel({ request }) {
    return (_jsxs(Box, { borderStyle: "single", flexDirection: "column", paddingX: 1, children: [_jsx(Text, { bold: true, children: "Last Request" }), !request ? (_jsx(Text, { dimColor: true, children: "No requests yet" })) : (_jsxs(Box, { flexDirection: "row", gap: 1, flexWrap: "wrap", children: [request.pending ? (_jsx(Text, { color: "yellow", children: "\u283F" })) : request.status_code ? (_jsx(Box, { borderStyle: "single", borderColor: statusColor(request.status_code), paddingX: 1, children: _jsx(Text, { color: statusColor(request.status_code), bold: true, children: request.status_code }) })) : (_jsx(Box, { borderStyle: "single", borderColor: "red", paddingX: 1, children: _jsx(Text, { color: "red", children: "ERR" }) })), _jsx(Text, { bold: true, children: request.kind === "https_get" ? "GET" : request.kind.toUpperCase() }), _jsxs(Text, { children: [request.host, request.path ?? ""] }), !request.pending && request.elapsed_ms !== undefined && (_jsxs(Text, { dimColor: true, children: [request.elapsed_ms, "ms", " ", request.response_size_bytes
                                ? formatBytes(request.response_size_bytes)
                                : ""] })), request.error && (_jsx(Text, { color: "red", children: request.error }))] }))] }));
}
