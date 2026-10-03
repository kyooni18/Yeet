import { remoteEndpoint } from "../platform/remote";
import { isDesktopHost } from "../platform/desktop/transport";
import React, { useState } from "react";
import { ScrollView, View } from "react-native";
import { connectRemote, useRemote } from "../state/client";
import { Button, Field, Label } from "../components/Controls";
import { useTheme } from "../theme/theme";
export function RemoteView() {
  const [endpoint, setEndpoint] = useState(
    isDesktopHost() ? "" : remoteEndpoint(),
  );
  const [key, setKey] = useState("");
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const snapshot = useRemote();
  const c = useTheme();
  return (
    <ScrollView
      contentContainerStyle={{
        flexGrow: 1,
        justifyContent: "center",
        padding: 24,
        alignItems: "center",
      }}
      keyboardShouldPersistTaps="handled"
    >
      <View
        style={{
          width: "100%",
          maxWidth: 480,
          gap: 16,
          padding: 24,
          borderRadius: 16,
          backgroundColor: c.surface,
        }}
      >
        <Label>Connect to Yeet</Label>
        <Label muted>
          {isDesktopHost()
            ? "Open an absolute workspace path with the local Rust core."
            : "Enter the Remote endpoint of your Rust workspace. On a phone, use your computer’s reachable network address."}
        </Label>
        <Field
          label={
            isDesktopHost() ? "Absolute workspace path" : "Remote endpoint"
          }
          value={endpoint}
          onChange={setEndpoint}
        />
        {!isDesktopHost() && (
          <Field
            label="Remote authentication key"
            value={key}
            onChange={setKey}
            secure
          />
        )}
        <Label muted>{snapshot.connection}</Label>
        {(error || snapshot.connectionError) && (
          <Label>{error || snapshot.connectionError}</Label>
        )}
        <Button
          label={working ? "Connecting…" : "Connect"}
          disabled={working || !endpoint.trim()}
          onPress={() => {
            setWorking(true);
            setError("");
            void connectRemote(endpoint.trim(), key)
              .catch((e: unknown) =>
                setError(e instanceof Error ? e.message : String(e)),
              )
              .finally(() => setWorking(false));
          }}
        />
      </View>
    </ScrollView>
  );
}
