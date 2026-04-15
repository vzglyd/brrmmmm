import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box, Text } from "ink";
export function Header({ wasmPath, abiVersion, describe }) {
    const name = describe?.name ?? wasmPath.split("/").pop() ?? wasmPath;
    const desc = describe?.description ?? "v1 sidecar (no manifest)";
    const modes = describe?.run_modes?.join(", ") ?? "legacy";
    return (_jsxs(Box, { borderStyle: "round", borderColor: "cyan", paddingX: 1, flexDirection: "row", justifyContent: "space-between", children: [_jsxs(Box, { flexDirection: "column", children: [_jsx(Text, { bold: true, color: "cyan", children: name }), _jsx(Text, { dimColor: true, children: desc })] }), _jsxs(Box, { flexDirection: "column", alignItems: "flex-end", children: [_jsxs(Text, { dimColor: true, children: ["ABI v", abiVersion, "  ", _jsx(Text, { color: "yellow", children: modes })] }), _jsx(Text, { dimColor: true, children: wasmPath })] })] }));
}
