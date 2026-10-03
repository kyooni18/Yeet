import React, { useState, useContext } from "react";
import { ScrollView, Switch, View } from "react-native";
import { remoteStore, useRemote } from "../state/client";
import {
  Button,
  Choice,
  Field,
  Label,
  ControlsDisabled,
} from "../components/Controls";
import { layout, useTheme } from "../theme/theme";
function Toggle({
  label,
  value,
  onChange,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <View style={layout.row}>
      <View style={layout.grow}>
        <Label>{label}</Label>
      </View>
      <Switch
        disabled={useContext(ControlsDisabled)}
        accessibilityLabel={label}
        value={value}
        onValueChange={onChange}
      />
    </View>
  );
}
export function SettingsView({ onClose }: { onClose: () => void }) {
  const { state, connection } = useRemote();
  const c = useTheme();
  const [provider, setProvider] = useState("");
  const [url, setUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [requiresKey, setRequiresKey] = useState(true);
  const [path, setPath] = useState("");
  const [host, setHost] = useState("");
  const [envKey, setEnvKey] = useState("");
  const [envValue, setEnvValue] = useState("");
  const sandbox = state.sandbox_settings;
  return (
    <View
      style={{ flex: 1, backgroundColor: c.background, padding: 16, gap: 16 }}
    >
      <View style={layout.row}>
        <Label>Settings</Label>
        <View style={layout.grow} />
        <Button label="Close settings" onPress={onClose} />
      </View>
      {connection !== "connected" && (
        <Label muted>Reconnect to edit settings.</Label>
      )}
      <ControlsDisabled.Provider value={connection !== "connected"}>
        <ScrollView
          keyboardShouldPersistTaps="handled"
          contentContainerStyle={{ gap: 16, paddingBottom: 32 }}
        >
          <Label>Appearance</Label>
          <Choice
            label="Appearance"
            value={state.runtime_settings?.appearance ?? "auto"}
            options={["auto", "dark", "light"]}
            onChange={(v) => remoteStore.setAppearance(v)}
          />
          {(["dark", "light"] as const).map((mode) => (
            <Choice
              key={mode}
              label={`${mode} theme`}
              value={
                mode === "dark"
                  ? (state.runtime_settings?.themeDark ?? "")
                  : (state.runtime_settings?.themeLight ?? "")
              }
              options={
                state.runtime_settings?.themeCatalog
                  .filter((t) => t.appearance === mode)
                  .map((t) => t.id) ?? []
              }
              onChange={(value) =>
                remoteStore.send({ type: "set_theme", mode, value })
              }
            />
          ))}
          <Label>Model settings</Label>
          <Choice
            label="Model"
            value={state.active_model}
            options={state.available_models}
            onChange={(m) => remoteStore.selectModel(m)}
          />
          <Toggle
            label="OpenAI Flex"
            value={state.openai_flex}
            onChange={(v) => remoteStore.setOpenAiFlex(v)}
          />
          <Label>Provider authentication</Label>
          {state.auth_providers.map((p) => (
            <View key={p.provider} style={{ gap: 8 }}>
              <Label>
                {p.provider} ·{" "}
                {p.authenticated ? "Authenticated" : "Not authenticated"}
              </Label>
              {p.error && <Label>{p.error}</Label>}
              {p.usage?.windows.map((w) => (
                <Label key={w.id} muted>
                  {w.label}: {w.remainingPercent}% remaining
                </Label>
              ))}
              <View style={layout.row}>
                <Button
                  label={`Log in ${p.provider}`}
                  disabled={state.auth_working}
                  onPress={() => remoteStore.authLogin(p.provider)}
                />
                <Button
                  label={`Log out ${p.provider}`}
                  disabled={state.auth_working || !p.authenticated}
                  onPress={() => remoteStore.authLogout(p.provider)}
                />
              </View>
            </View>
          ))}
          <Field label="Provider ID" value={provider} onChange={setProvider} />
          <Field label="API key" secure value={apiKey} onChange={setApiKey} />
          <Button
            label="Save API key"
            disabled={!provider.trim() || !apiKey || state.auth_working}
            onPress={() => {
              if (remoteStore.authSetApiKey(provider.trim(), apiKey))
                setApiKey("");
            }}
          />
          {state.auth_notice && <Label>{state.auth_notice}</Label>}
          <Label>Provider configuration</Label>
          {state.provider_configurations.map((p) => (
            <View key={p.id} style={{ gap: 8 }}>
              <Label>
                {p.id} · {p.base_url}
              </Label>
              <View style={layout.row}>
                <Button
                  label={`Edit ${p.id}`}
                  onPress={() => {
                    setProvider(p.id);
                    setUrl(p.base_url);
                    setRequiresKey(p.require_api_key);
                  }}
                />
                <Button
                  label={`Remove ${p.id}`}
                  onPress={() =>
                    remoteStore.send({ type: "remove_provider", id: p.id })
                  }
                />
              </View>
            </View>
          ))}
          <Field label="Provider base URL" value={url} onChange={setUrl} />
          <Toggle
            label="Require API key"
            value={requiresKey}
            onChange={setRequiresKey}
          />
          <Button
            label="Save provider"
            disabled={
              !provider.trim() || !url.trim() || state.providers_working
            }
            onPress={() =>
              remoteStore.send({
                type: "save_provider",
                id: provider.trim(),
                base_url: url.trim(),
                require_api_key: requiresKey,
              })
            }
          />
          {state.providers_notice && <Label>{state.providers_notice}</Label>}
          <Label>Capabilities</Label>
          {state.available_capabilities.map((cap) => (
            <View key={cap.id}>
              <Toggle
                label={cap.name}
                value={cap.enabled}
                onChange={() => remoteStore.toggleCapability(cap.id)}
              />
              <Label muted>{cap.description}</Label>
            </View>
          ))}
          <Label>Memory</Label>
          <Toggle
            label="Foundation Memory"
            value={state.foundation_memory_enabled}
            onChange={(v) => remoteStore.setFoundationMemory(v)}
          />
          <Label muted>
            {state.foundation_memory_connected ? "Connected" : "Disconnected"} ·{" "}
            {state.foundation_memory_backend} / {state.foundation_memory_server}
          </Label>
          <Choice
            label="Memory backend"
            value={state.foundation_memory_backend}
            options={["builtin", "mcp"]}
            onChange={(backend) =>
              remoteStore.send({
                type: "set_service_backend",
                service: "memory",
                backend,
                server: state.foundation_memory_server,
              })
            }
          />
          <Choice
            label="Web backend"
            value={state.web_backend}
            options={["builtin", "mcp"]}
            onChange={(backend) =>
              remoteStore.send({
                type: "set_service_backend",
                service: "web",
                backend,
                server: state.web_server,
              })
            }
          />
          <Label>Sandbox</Label>
          {sandbox ? (
            <>
              <Choice
                label="Preset"
                value={sandbox.preset}
                options={["safe", "balanced", "unlimited"]}
                onChange={(preset) =>
                  remoteStore.updateSandbox({ type: "apply_preset", preset })
                }
              />
              <Choice
                label="Execution mode"
                value={sandbox.execution_mode}
                options={["sandboxed", "unlimited"]}
                onChange={(mode) =>
                  remoteStore.updateSandbox({
                    type: "set_execution_mode",
                    mode,
                  })
                }
              />
              <Choice
                label="Workspace mode"
                value={sandbox.workspace_mode}
                options={["none", "all", "paths"]}
                onChange={(mode) =>
                  remoteStore.updateSandbox({
                    type: "set_workspace_mode",
                    mode,
                  })
                }
              />
              <Toggle
                label="Automatically approve"
                value={sandbox.auto_approve}
                onChange={(enabled) =>
                  remoteStore.updateSandbox({
                    type: "set_auto_approve",
                    enabled,
                  })
                }
              />
              <Toggle
                label="Writable scratch"
                value={sandbox.scratch_writable}
                onChange={(enabled) =>
                  remoteStore.updateSandbox({
                    type: "set_scratch_writable",
                    enabled,
                  })
                }
              />
              {sandbox.workspace_paths.map((p) => (
                <Button
                  key={p}
                  label={`Remove workspace path ${p}`}
                  onPress={() =>
                    remoteStore.updateSandbox({
                      type: "remove_workspace_path",
                      path: p,
                    })
                  }
                />
              ))}
              <Field
                label="Additional workspace path"
                value={path}
                onChange={setPath}
              />
              <Button
                label="Add workspace path"
                disabled={!path.trim()}
                onPress={() =>
                  remoteStore.updateSandbox({
                    type: "add_workspace_path",
                    path: path.trim(),
                  })
                }
              />
              {sandbox.network_allow.map((n) => (
                <Button
                  key={`${n.host}:${n.port}`}
                  label={`Remove network ${n.host}${n.port ? `:${n.port}` : ""}`}
                  onPress={() =>
                    remoteStore.updateSandbox({
                      type: "remove_network",
                      host: n.host,
                      port: n.port,
                    })
                  }
                />
              ))}
              <Field
                label="Allowed network host"
                value={host}
                onChange={setHost}
              />
              <Button
                label="Allow network host"
                disabled={!host.trim()}
                onPress={() =>
                  remoteStore.updateSandbox({
                    type: "add_network",
                    host: host.trim(),
                  })
                }
              />
              {sandbox.environment.map((e) => (
                <Button
                  key={e.key}
                  label={`Remove environment ${e.key}`}
                  onPress={() =>
                    remoteStore.updateSandbox({
                      type: "remove_environment",
                      key: e.key,
                    })
                  }
                />
              ))}
              <Field
                label="Environment key"
                value={envKey}
                onChange={setEnvKey}
              />
              <Field
                label="Environment value"
                value={envValue}
                onChange={setEnvValue}
              />
              <Button
                label="Save environment variable"
                disabled={!envKey.trim()}
                onPress={() =>
                  remoteStore.updateSandbox({
                    type: "set_environment",
                    key: envKey.trim(),
                    value: envValue,
                  })
                }
              />
              {Object.entries(sandbox.limits).map(([name, value]) => (
                <Field
                  key={`${name}:${value}`}
                  label={name}
                  value={String(value)}
                  onChange={(input) => {
                    const next = Number(input);
                    if (input.trim() && Number.isFinite(next) && next >= 0)
                      remoteStore.updateSandbox({
                        type: "set_limit",
                        name,
                        value: next,
                      });
                  }}
                />
              ))}
              <Button
                label="Reset sandbox"
                onPress={() => remoteStore.updateSandbox({ type: "reset" })}
              />
            </>
          ) : (
            <Label muted>Waiting for sandbox settings from Rust core.</Label>
          )}
          {state.sandbox_notice && <Label>{state.sandbox_notice}</Label>}
          {state.settings_notice && <Label>{state.settings_notice}</Label>}
        </ScrollView>
      </ControlsDisabled.Provider>
    </View>
  );
}
