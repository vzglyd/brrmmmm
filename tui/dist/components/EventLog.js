import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
export function EventLog({ logs }) {
    const recent = logs.slice(-4);
    return (_jsxs(Box, { borderStyle: "single", borderColor: "gray", flexDirection: "column", paddingX: 1, children: [_jsx(Text, { dimColor: true, bold: true, children: "Logs" }), recent.length === 0 ? (_jsx(Text, { dimColor: true, children: "No log messages yet" })) : (recent.map((line, i) => (_jsx(Text, { dimColor: true, wrap: "truncate", children: line }, i))))] }));
}
