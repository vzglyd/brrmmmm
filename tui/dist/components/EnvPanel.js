import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
export function EnvPanel({ vars }) {
    return (_jsxs(Box, { borderStyle: "single", flexDirection: "column", paddingX: 1, flexGrow: 1, children: [_jsx(Text, { bold: true, children: "Environment Variables" }), vars.length === 0 ? (_jsx(Text, { dimColor: true, children: "No env vars declared" })) : (vars.map((v) => (_jsxs(Box, { flexDirection: "row", gap: 1, children: [_jsx(Text, { color: v.set ? "green" : v.required ? "red" : "yellow", children: v.set ? "✓" : "✗" }), _jsx(Text, { bold: v.required, children: v.name }), v.required && !v.set && _jsx(Text, { color: "red", children: " (required)" }), v.description ? (_jsxs(Text, { dimColor: true, children: [" \u2014 ", v.description] })) : null] }, v.name))))] }));
}
