# Yeet desktop

Tauri 2 wraps the Expo React Native Web export and embeds the existing Rust Yeet core. The frontend is intentionally blank until the user’s OpenPencil design is implemented. Read `app/ui/ContentView.ui` before changing frontend structure.

Install dependencies in `RuntimeSource`, `frontend/react-native` and this directory, then run `npm run dev` here. `npm run build` builds and stages the existing RuntimeSource assets, exports the frontend and packages the desktop application. macOS, Windows and Linux require the normal [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Distribution signing is configured by the release environment.

The application bundles `RuntimeSource/dist`, `skills`, and `package.json`; no runtime `node_modules` directory is needed. Node.js 20 or newer must be available on the launched application's PATH, or selected by `YEET_NODE`. GUI launches may have a different PATH from your terminal. Runtime assets resolve from Tauri's resource directory; direct Cargo development builds use the same ignored staging directory. Run `npm run prepare:runtime` before invoking Cargo directly. `YEET_RUNTIME_DIR` remains an explicit override. Node is an existing Yeet core dependency and is not bundled by this shell.

The narrow local bridge is available through `window.__TAURI__`:

- `core.invoke('connect_core', { workspace })` returns `{ workspace, state }`. Workspace must resolve to an existing directory. Successful reconnect replaces and drops the previous embedded harness; failed reconnect preserves it.
- `core.invoke('send_core_command', { command })` accepts the existing Rust `FrontendCommand` JSON. `shutdown` is rejected: window lifecycle owns the harness.
- `event.listen('yeet://core-event', callback)` receives existing `BridgeEnvelope` objects (`type`, `state`, `message`, plus a workspace scope). State comes directly from Rust; the adapter retains unchanged conversation history when Rust omits it so each state payload is a full snapshot. Register the listener before connecting. Buffer events during connection, then apply the returned initial snapshot followed by buffered events, so response/event scheduling cannot overwrite a newer state.
- `core.invoke('disconnect_core')` drops the local harness.

The worker serializes lifecycle changes and commands and polls semantic events. No HTTP remote server, sidecar, UI specification parser, or alternate agent implementation is introduced. Network connections from the same frontend continue to use remote protocol v1. The desktop bridge is trusted local IPC and does not replace network authentication.
