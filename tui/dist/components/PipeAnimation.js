import { jsx as _jsx, jsxs as _jsxs } from "react/jsx-runtime";
import { useState, useEffect } from "react";
import { Box, Text } from "ink";
const HALF = "━━━━━━━━━━";
const STEP_MS = 800;
export function PipeAnimation({ publishedReceivedAt, cycleCount }) {
    const [stage, setStage] = useState("idle");
    useEffect(() => {
        if (publishedReceivedAt === null)
            return;
        setStage("ingesting");
        const t1 = setTimeout(() => setStage("processing"), STEP_MS);
        const t2 = setTimeout(() => setStage("emitting"), STEP_MS * 2);
        const t3 = setTimeout(() => setStage("idle"), STEP_MS * 3);
        return () => {
            clearTimeout(t1);
            clearTimeout(t2);
            clearTimeout(t3);
        };
    }, [publishedReceivedAt]);
    // Top-left: red when ingesting or processing; green when emitting; gray at idle.
    const tlColor = stage === "ingesting" || stage === "processing" ? "red"
        : stage === "emitting" ? "green"
            : "gray";
    // Bottom-left: red when ingesting only; green when processing or emitting; gray at idle.
    const blColor = stage === "ingesting" ? "red"
        : stage === "processing" || stage === "emitting" ? "green"
            : "gray";
    // Right side: green when emitting; gray otherwise.
    const rColor = stage === "emitting" ? "green" : "gray";
    const icon = stage === "ingesting" ? "◀"
        : stage === "processing" ? "▶"
            : stage === "emitting" ? "◆"
                : "·";
    return (_jsxs(Box, { flexDirection: "column", alignItems: "center", paddingY: 1, children: [_jsxs(Text, { children: [_jsx(Text, { color: tlColor, children: HALF }), _jsx(Text, { color: tlColor, children: icon }), _jsx(Text, { color: rColor, children: HALF })] }), _jsxs(Text, { children: [_jsx(Text, { color: blColor, children: HALF }), _jsx(Text, { color: blColor, children: icon }), _jsx(Text, { color: rColor, children: HALF })] }), cycleCount > 0 && (_jsx(Text, { dimColor: true, children: `(${cycleCount} run${cycleCount === 1 ? "" : "s"})` }))] }));
}
