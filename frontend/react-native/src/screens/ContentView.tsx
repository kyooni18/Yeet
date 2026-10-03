import React, { useState } from "react";
import {
  KeyboardAvoidingView,
  Modal,
  Platform,
  View,
  useWindowDimensions,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { useRemote } from "../state/client";
import { ThemeProvider, layout, useTheme } from "../theme/theme";
import { Button } from "../components/Controls";
import { SessionList } from "../components/SessionList";
import { ToolInspector } from "../components/ToolInspector";
import { SessionView } from "./SessionView";
import { SettingsView } from "./SettingsView";
import { RemoteView } from "./RemoteView";
function Workbench() {
  const { width } = useWindowDimensions();
  const desktop = width >= 1100;
  const phone = width < 700;
  const [sessions, setSessions] = useState(false);
  const [inspector, setInspector] = useState(false);
  const [settings, setSettings] = useState(false);
  const { connection, state } = useRemote();
  const c = useTheme();
  const authenticated =
    connection === "connected" ||
    ((connection === "connecting" ||
      connection === "reconnecting" ||
      connection === "offline") &&
      !!state.workspace_root);
  return (
    <SafeAreaView style={{ flex: 1, backgroundColor: c.background }}>
      <KeyboardAvoidingView
        style={layout.grow}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
      >
        {!authenticated ? (
          <RemoteView />
        ) : (
          <View style={[layout.grow, { flexDirection: "row" }]}>
            {(desktop || (!phone && sessions)) && (
              <View
                style={{
                  width: desktop ? 280 : 240,
                  borderRightWidth: 1,
                  borderColor: c.border,
                }}
              >
                <SessionList
                  onSettings={() => setSettings(true)}
                  onSelected={() => setSessions(false)}
                />
              </View>
            )}
            <View style={layout.grow}>
              <View
                style={[
                  layout.row,
                  { padding: 8, borderBottomWidth: 1, borderColor: c.border },
                ]}
              >
                {!desktop && (
                  <Button
                    label="Sessions"
                    onPress={() => setSessions(!sessions)}
                  />
                )}
                <View style={layout.grow} />
                <Button
                  label="Tools"
                  selected={inspector}
                  onPress={() => setInspector(!inspector)}
                />
                <Button label="Settings" onPress={() => setSettings(true)} />
              </View>
              <SessionView onTools={() => setInspector(true)} />
            </View>
            {!phone && inspector && (
              <View
                style={{
                  width: desktop ? 320 : 280,
                  borderLeftWidth: 1,
                  borderColor: c.border,
                }}
              >
                <ToolInspector onClose={() => setInspector(false)} />
              </View>
            )}
          </View>
        )}
      </KeyboardAvoidingView>
      <Modal
        visible={phone && sessions}
        animationType="slide"
        onRequestClose={() => setSessions(false)}
      >
        <SafeAreaView style={{ flex: 1, backgroundColor: c.surface }}>
          <Button label="Close sessions" onPress={() => setSessions(false)} />
          <SessionList
            onSettings={() => {
              setSessions(false);
              setSettings(true);
            }}
            onSelected={() => setSessions(false)}
          />
        </SafeAreaView>
      </Modal>
      <Modal
        visible={phone && inspector}
        animationType="slide"
        onRequestClose={() => setInspector(false)}
      >
        <SafeAreaView style={{ flex: 1, backgroundColor: c.surface }}>
          <ToolInspector onClose={() => setInspector(false)} />
        </SafeAreaView>
      </Modal>
      <Modal
        visible={settings}
        animationType="slide"
        onRequestClose={() => setSettings(false)}
      >
        <SafeAreaView style={{ flex: 1, backgroundColor: c.background }}>
          <SettingsView onClose={() => setSettings(false)} />
        </SafeAreaView>
      </Modal>
    </SafeAreaView>
  );
}
export function ContentView() {
  return (
    <ThemeProvider>
      <Workbench />
    </ThemeProvider>
  );
}
