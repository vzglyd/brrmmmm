import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { Box } from "ink";
import { ArtifactPane } from "./ArtifactPane.js";
import { PipeAnimation } from "./PipeAnimation.js";
export function ArtifactRow({ artifacts, cycleCount }) {
    return (_jsxs(Box, { flexDirection: "row", children: [_jsx(Box, { width: "38%", children: _jsx(ArtifactPane, { title: "RAW", artifact: artifacts.raw }) }), _jsx(Box, { width: "24%", justifyContent: "center", alignItems: "center", children: _jsx(PipeAnimation, { publishedReceivedAt: artifacts.published?.received_at_ms ?? null, cycleCount: cycleCount }) }), _jsx(Box, { width: "38%", children: _jsx(ArtifactPane, { title: "OUTPUT", artifact: artifacts.published }) })] }));
}
