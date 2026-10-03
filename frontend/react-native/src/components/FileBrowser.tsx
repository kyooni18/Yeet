import React from "react";
import { View } from "react-native";
import { useRemote } from "../state/client";
import { Label } from "./Controls";
export function FileBrowser() {
  const { state } = useRemote();
  return (
    <View style={{ padding: 12, gap: 8 }}>
      <Label>{state.workspace_root || "No workspace selected"}</Label>
      <Label muted>
        The Remote protocol does not expose directory browsing. Workspace files
        are accessed by the Rust agent; file operations appear in tool activity.
      </Label>
    </View>
  );
}
