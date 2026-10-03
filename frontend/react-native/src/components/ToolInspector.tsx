import React, { useState } from "react";
import { ScrollView, View } from "react-native";
import { useRemote } from "../state/client";
import { Button, Label } from "./Controls";
import { FileBrowser } from "./FileBrowser";
import { layout, useTheme } from "../theme/theme";
import type { ConversationToolCall } from "../remote/protocol";
export function ToolInspector({ onClose }: { onClose: () => void }) {
  const { entries, state } = useRemote();
  const [selected, setSelected] = useState<ConversationToolCall | null>(null);
  const [files, setFiles] = useState(false);
  const c = useTheme();
  const tools = entries.flatMap((e) =>
    e.kind.type === "toolCall"
      ? [e.kind.toolCall]
      : e.kind.type === "assistant"
        ? (e.kind.toolCalls ?? [])
        : [],
  );
  const current = tools.find((t) => t.id === selected?.id) ?? selected;
  return (
    <View style={[layout.grow, layout.panel, { backgroundColor: c.surface }]}>
      <View style={layout.row}>
        <Label>Tool inspector</Label>
        <Button label="Close inspector" onPress={onClose} />
      </View>
      <ScrollView contentContainerStyle={{ gap: 12 }}>
        {state.agent_tasks.map((t) => (
          <View key={t.id}>
            <Label>
              {t.role} · {t.status}
            </Label>
            <Label muted>{t.objective}</Label>
            {t.summary && <Label>{t.summary}</Label>}
          </View>
        ))}
        {tools.map((t) => (
          <Button
            key={t.id}
            label={`${t.label || t.name} · ${t.status}`}
            selected={t.id === current?.id}
            onPress={() => setSelected(t)}
          />
        ))}
        {!tools.length && <Label muted>No tool activity yet</Label>}
        {current && (
          <View style={{ gap: 8 }}>
            <Label>{current.name}</Label>
            <Label muted>{current.detail}</Label>
            <Label>{current.arguments}</Label>
            <Label>
              {typeof current.result === "string"
                ? current.result
                : JSON.stringify(current.result, null, 2)}
            </Label>
            {current.error && <Label>{current.error}</Label>}
          </View>
        )}
      </ScrollView>
      <Button label="Workspace files" onPress={() => setFiles(!files)} />
      {files && <FileBrowser />}
    </View>
  );
}
