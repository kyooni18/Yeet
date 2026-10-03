import React, { useState } from "react";
import { FlatList, View } from "react-native";
import { remoteStore, useRemote } from "../state/client";
import { Button, Choice, Field, Label } from "./Controls";
import { layout, useTheme } from "../theme/theme";
export function SessionList({
  onSettings,
  onSelected,
}: {
  onSettings: () => void;
  onSelected: () => void;
}) {
  const snapshot = useRemote();
  const [search, setSearch] = useState("");
  const c = useTheme();
  return (
    <View style={[layout.grow, layout.panel, { backgroundColor: c.surface }]}>
      <Label>Yeet</Label>
      <Choice
        label="Workspace"
        value={snapshot.state.workspace_root ?? ""}
        options={snapshot.state.known_workspaces.map((w) => w.path)}
        onChange={(w) => remoteStore.switchWorkspace(w)}
      />
      <Field label="Search sessions" value={search} onChange={setSearch} />
      <Button
        label="New session"
        disabled={snapshot.connection !== "connected"}
        onPress={() => {
          if (remoteStore.newSession()) onSelected();
        }}
      />
      <FlatList
        data={snapshot.currentWorkspaceSessions.filter((s) =>
          s.title.toLowerCase().includes(search.toLowerCase()),
        )}
        keyExtractor={(s) => s.id}
        ListEmptyComponent={<Label muted>No saved sessions</Label>}
        renderItem={({ item }) => (
          <View style={{ marginBottom: 8 }}>
            <Button
              label={item.title || "Untitled session"}
              selected={item.id === snapshot.state.current_session_id}
              onPress={() => {
                if (remoteStore.loadSession(item.id)) onSelected();
              }}
            />
            <Label muted>
              {item.model} · {item.message_count} messages
            </Label>
          </View>
        )}
      />
      <Button label="Settings" onPress={onSettings} />
    </View>
  );
}
