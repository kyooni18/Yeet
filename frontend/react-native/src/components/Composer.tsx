import {
  pickAttachment,
  deleteRemoteAttachment,
  type UploadedAttachment,
} from "../platform/attachments";
import { isDesktopHost } from "../platform/desktop/transport";
import React, { useEffect, useState, useRef } from "react";
import { TextInput, View } from "react-native";
import { remoteStore, useRemote } from "../state/client";
import { Button, Choice, Label } from "./Controls";
import { layout, useTheme } from "../theme/theme";
const drafts = new Map<string, string>();
const attachmentDrafts = new Map<string, UploadedAttachment[]>();
export function Composer() {
  const { state, sessionResetRevision } = useRemote();
  const key = `${state.workspace_root ?? ""}:${state.current_session_id ?? "new"}:${sessionResetRevision}`;
  return <ComposerDraft key={key} draftKey={key} />;
}
function ComposerDraft({ draftKey: key }: { draftKey: string }) {
  const { state, connection } = useRemote();
  const mounted = useRef(true);
  const [text, setText] = useState(drafts.get(key) ?? "");
  const [attachments, setAttachments] = useState<UploadedAttachment[]>(
    attachmentDrafts.get(key) ?? [],
  );
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState("");
  const c = useTheme();
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const saveAttachments = (next: UploadedAttachment[]) => {
    attachmentDrafts.set(key, next);
    setAttachments(next);
  };
  const update = (s: string) => {
    drafts.set(key, s);
    setText(s);
  };
  return (
    <View
      style={{ padding: 12, gap: 8, borderTopWidth: 1, borderColor: c.border }}
    >
      {attachments.map((a) => (
        <Button
          key={a.id}
          label={`Remove ${a.name || a.id}`}
          onPress={() => {
            void deleteRemoteAttachment(a.id)
              .then(() =>
                saveAttachments(attachments.filter((item) => item.id !== a.id)),
              )
              .catch((e: unknown) => setError(String(e)));
          }}
        />
      ))}
      {error && <Label>{error}</Label>}
      <TextInput
        accessibilityLabel="Message"
        placeholder="Ask Yeet to work on something…"
        placeholderTextColor={c.textDim}
        multiline
        value={text}
        onChangeText={update}
        style={{
          color: c.text,
          backgroundColor: c.surfaceRaised,
          padding: 12,
          borderRadius: 10,
          minHeight: 84,
          maxHeight: 180,
          textAlignVertical: "top",
        }}
      />
      <View style={layout.row}>
        <Button
          label={uploading ? "Uploading…" : "Attach file"}
          disabled={uploading || isDesktopHost() || connection !== "connected"}
          onPress={() => {
            const uploadKey = key;
            setUploading(true);
            setError("");
            void pickAttachment()
              .then((a) => {
                if (a) {
                  const next = [...(attachmentDrafts.get(uploadKey) ?? []), a];
                  attachmentDrafts.set(uploadKey, next);
                  if (mounted.current) setAttachments(next);
                }
              })
              .catch((e: unknown) => setError(String(e)))
              .finally(() => setUploading(false));
          }}
        />
        <Choice
          label="Model"
          value={state.active_model}
          options={state.available_models}
          onChange={(model) => remoteStore.selectModel(model)}
        />
        <Choice
          label="Reasoning"
          value={state.active_reasoning_level}
          options={[
            "auto",
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
          ]}
          onChange={(level) => remoteStore.selectReasoning(level)}
        />
        <View style={layout.grow} />
        <Button
          label={state.is_streaming ? "Interrupt" : "Send"}
          disabled={
            connection !== "connected" ||
            (!state.is_streaming && !text.trim() && !attachments.length) ||
            uploading
          }
          onPress={() => {
            if (state.is_streaming) remoteStore.interrupt();
            else if (
              remoteStore.submit(
                text.trim(),
                attachments.map((a) => a.id),
              )
            ) {
              update("");
              saveAttachments([]);
            }
          }}
        />
      </View>
      {isDesktopHost() && (
        <Label muted>
          Local Harness does not expose attachments. Connect through Remote to
          upload files.
        </Label>
      )}
    </View>
  );
}
