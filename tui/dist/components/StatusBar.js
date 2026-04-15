import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
export function StatusBar({ isRunning, error }) {
    return (_jsxs(Box, { borderStyle: "single", borderColor: "gray", paddingX: 1, justifyContent: "space-between", children: [_jsx(Text, { dimColor: true, children: "q quit  \u2022  Ctrl+C stop sidecar" }), error ? (_jsx(Text, { color: "red", children: error })) : !isRunning ? (_jsx(Text, { color: "yellow", children: "Sidecar stopped" })) : null] }));
}
