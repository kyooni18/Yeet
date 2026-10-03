import * as Clipboard from "expo-clipboard";
import React, { useRef, useState } from "react";
import { FlatList, View } from "react-native";
import { remoteStore, useRemote } from "../state/client";
import { Button, Choice, Label, Field } from "../components/Controls";
import { Composer } from "../components/Composer";
import { layout, useTheme } from "../theme/theme";
import type { ConversationEntry } from "../remote/protocol";
function Message({
  entry,
  onTools,
  isLastUser = false,
  isLastAssistant = false,
  canMutate = false,
}: {
  entry: ConversationEntry;
  onTools: () => void;
  isLastUser?: boolean;
  isLastAssistant?: boolean;
  canMutate?: boolean;
}) {
  const kind = entry.kind;
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState(
    "content" in kind ? kind.content : "",
  );
  const [copyNotice, setCopyNotice] = useState("");
  const c = useTheme();
  return (
    <View
      style={{
        padding: 14,
        gap: 8,
        borderRadius: 10,
        backgroundColor: kind.type === "user" ? c.userSurface : c.surface,
        marginBottom: 12,
      }}
    >
      {kind.type === "assistant" && !!kind.content && (
        <Button
          label="Copy response"
          onPress={() => {
            void Clipboard.setStringAsync(kind.content)
              .then(() => setCopyNotice("Copied"))
              .catch(() => setCopyNotice("Could not copy response"));
          }}
        />
      )}
      {copyNotice && <Label muted>{copyNotice}</Label>}
      {isLastAssistant && (
        <Button
          label="Regenerate response"
          disabled={!canMutate}
          onPress={() => remoteStore.regenerateLast()}
        />
      )}
      {isLastUser && (
        <Button
          label={editing ? "Cancel edit" : "Edit message"}
          disabled={!canMutate}
          onPress={() => setEditing(!editing)}
        />
      )}
      {editing && (
        <>
          <Field
            label="Edit last message"
            multiline
            value={editText}
            onChange={setEditText}
          />
          <Button
            label="Save and regenerate"
            disabled={!canMutate || !editText.trim()}
            onPress={() => {
              if (remoteStore.editLast(editText.trim())) setEditing(false);
            }}
          />
        </>
      )}
      <Label muted>
        {kind.type}
        {entry.uiStreaming ? " · streaming" : ""}
      </Label>
      {kind.type === "reasoning" ? (
        <>
          <Button
            label={expanded ? "Hide reasoning" : "Show reasoning"}
            onPress={() => setExpanded(!expanded)}
          />
          <Label>{kind.summary || ""}</Label>
          {expanded && <Label>{kind.content}</Label>}
        </>
      ) : kind.type === "activity" ? (
        <>
          <Label>{kind.activity.title}</Label>
          <Label muted>{kind.activity.detail}</Label>
        </>
      ) : kind.type === "toolCall" ? (
        <Button
          label={`${kind.toolCall.label || kind.toolCall.name} · ${kind.toolCall.status}`}
          onPress={onTools}
        />
      ) : (
        <>
          <Label>{"content" in kind ? kind.content : ""}</Label>
          {kind.type === "assistant" &&
            kind.toolCalls?.map((tool) => (
              <Button
                key={tool.id}
                label={`${tool.label || tool.name} · ${tool.status}`}
                onPress={onTools}
              />
            ))}
        </>
      )}
    </View>
  );
}
export function SessionView({ onTools }: { onTools: () => void }) {
  const snapshot = useRemote();
  const { state } = snapshot;
  const c = useTheme();
  const list = useRef<FlatList<ConversationEntry>>(null);
  const follow = useRef(true);
  const lastUser = snapshot.entries.findLast((e) => e.kind.type === "user")?.id;
  const lastAssistant = snapshot.entries.findLast(
    (e) => e.kind.type === "assistant",
  )?.id;
  const pending =
    state.pending_shell_permission ?? state.pending_native_app_permission;
  return (
    <View style={layout.grow}>
      <View
        style={{
          padding: 12,
          gap: 8,
          borderBottomWidth: 1,
          borderColor: c.border,
        }}
      >
        <Label>{snapshot.currentSession?.title || "New session"}</Label>
        <View style={layout.row}>
          <Label muted>
            {snapshot.connection}
            {snapshot.contextPercent !== null
              ? ` · Context ${snapshot.contextPercent}%`
              : ""}
          </Label>
          <Button
            label={state.goal_mode ? "Goal on" : "Goal off"}
            selected={state.goal_mode}
            onPress={() => remoteStore.setGoal(!state.goal_mode)}
          />
          <Choice
            label="Agents"
            value={state.agent_mode}
            options={["single", "adaptive"]}
            onChange={(mode) =>
              remoteStore.send({
                type: "set_agent_mode",
                mode: mode as "single" | "adaptive",
              })
            }
          />
          <Choice
            label="Autonomy"
            value={state.autonomy_mode}
            options={["manual", "goal", "autonomous"]}
            onChange={(mode) =>
              remoteStore.send({
                type: "set_autonomy_mode",
                mode: mode as "manual" | "goal" | "autonomous",
              })
            }
          />
        </View>
        {snapshot.connectionError && <Label>{snapshot.connectionError}</Label>}
        {state.error_message && <Label>{state.error_message}</Label>}
      </View>
      <FlatList
        ref={list}
        data={snapshot.entries}
        extraData={snapshot.revision}
        keyExtractor={(e) => e.id}
        contentContainerStyle={{ padding: 16, flexGrow: 1 }}
        keyboardShouldPersistTaps="handled"
        onScroll={(e) => {
          const s = e.nativeEvent;
          follow.current =
            s.contentSize.height -
              s.contentOffset.y -
              s.layoutMeasurement.height <
            100;
        }}
        scrollEventThrottle={100}
        onContentSizeChange={() => {
          if (follow.current) list.current?.scrollToEnd({ animated: false });
        }}
        ListEmptyComponent={
          <View style={{ padding: 24, gap: 10 }}>
            <Label>What would you like to work on?</Label>
            <Label muted>
              Your session runs in the connected Rust workspace.
            </Label>
          </View>
        }
        renderItem={({ item }) => (
          <Message
            entry={item}
            onTools={onTools}
            isLastUser={item.id === lastUser}
            isLastAssistant={item.id === lastAssistant}
            canMutate={
              snapshot.connection === "connected" && !state.is_streaming
            }
          />
        )}
      />
      {pending && (
        <View style={{ padding: 12, gap: 8, backgroundColor: c.surfaceRaised }}>
          <Label>Permission requested</Label>
          <Label>
            {"command" in pending
              ? pending.command
              : `${pending.server}: ${pending.tool}`}
          </Label>
          <Label muted>{pending.reason}</Label>
          <View style={layout.row}>
            <Button
              label="Allow"
              onPress={() => remoteStore.allowPermission()}
            />
            <Button label="Deny" onPress={() => remoteStore.denyPermission()} />
          </View>
        </View>
      )}
      <Composer />
    </View>
  );
}
