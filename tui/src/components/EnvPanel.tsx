import React from "react";
import { Box, Text, useInput } from "ink";
import { type MergedEnvVar, type ModuleParamsSchema } from "../types.js";

const AMBER = "#FFB300";

interface Props {
  vars: MergedEnvVar[];
  params: ModuleParamsSchema | null;
  manifestPending: boolean;
  isFocused: boolean;
  values: Record<string, string>;
  onChange: (key: string, value: string) => void;
}

export function EnvPanel({
  vars,
  params,
  manifestPending,
  isFocused,
  values,
  onChange,
}: Props) {
  // Only show manifest-declared params (those with a description or required flag set by the spec).
  // Extra --env vars not in the manifest are still shown so devs know what's active.
  const declared = vars.filter((v) => v.required || v.description !== "");
  const extras = vars.filter((v) => !v.required && v.description === "" && v.set);
  const paramFields = params?.fields ?? [];
  const [selectedIndex, setSelectedIndex] = React.useState(0);
  const selectedField = paramFields[selectedIndex] ?? null;

  React.useEffect(() => {
    if (selectedIndex >= paramFields.length) {
      setSelectedIndex(Math.max(0, paramFields.length - 1));
    }
  }, [paramFields.length, selectedIndex]);

  useInput(
    (input, key) => {
      if (!selectedField) return;
      if (key.upArrow) {
        setSelectedIndex((index) => Math.max(0, index - 1));
        return;
      }
      if (key.downArrow) {
        setSelectedIndex((index) => Math.min(paramFields.length - 1, index + 1));
        return;
      }
      if (key.backspace || key.delete) {
        onChange(selectedField.key, (values[selectedField.key] ?? "").slice(0, -1));
        return;
      }
      if (key.tab) return;
      if (key.return) return;
      if (input && !key.ctrl && !key.meta) {
        onChange(selectedField.key, `${values[selectedField.key] ?? ""}${input}`);
      }
    },
    { isActive: isFocused },
  );

  return (
    <Box
      borderStyle="single"
      borderColor={isFocused ? AMBER : "gray"}
      flexDirection="column"
      paddingX={1}
      flexGrow={1}
    >
      <Text bold color={AMBER}>Parameters</Text>
      {manifestPending ? (
        <Text dimColor>Waiting for module contract...</Text>
      ) : declared.length === 0 && extras.length === 0 && paramFields.length === 0 ? (
        <Text dimColor>No parameters declared · use --env KEY=VALUE or --params-json</Text>
      ) : (
        <>
          {paramFields.map((field) => (
            <Box key={`param:${field.key}`} flexDirection="column">
              <Text color={isFocused && selectedField?.key === field.key ? AMBER : "white"}>
                {isFocused && selectedField?.key === field.key ? ">" : field.required ? "!" : "•"}{" "}
                {field.key} ({field.type}) = {values[field.key] ?? ""}
                {isFocused && selectedField?.key === field.key ? "█" : ""}
                {field.required ? " required" : ""}
              </Text>
            </Box>
          ))}
          {declared.map((v) => (
            <Box key={v.name} flexDirection="row" gap={1}>
              <Text color={v.set ? AMBER : v.required ? "red" : "gray"}>
                {v.set ? "✓" : "✗"}
              </Text>
              <Text bold={v.required}>{v.name}</Text>
              {v.required && !v.set && <Text color="red">(required)</Text>}
              {v.description ? <Text dimColor>— {v.description}</Text> : null}
            </Box>
          ))}
          {extras.map((v) => (
            <Box key={v.name} flexDirection="row" gap={1}>
              <Text color={AMBER}>✓</Text>
              <Text dimColor>{v.name}</Text>
              <Text dimColor>— via --env</Text>
            </Box>
          ))}
        </>
      )}
    </Box>
  );
}
