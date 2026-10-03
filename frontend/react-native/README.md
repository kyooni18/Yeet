# Yeet native JavaScript foundation

This is an intentionally blank Expo / React Native application. There are no screens, visual components, theme, or proposed design. The user will create the design in OpenPencil.

`App.tsx` exports the blank `ContentView`. Expo Router’s single route mounts it. Implement visual UI only when the user supplies or requests the design. `app/ui/ContentView.ui` is reserved for the AI-readable layout specification; it is never interpreted at runtime.

## Run and check

Requires Node.js 22.13 or newer for Expo SDK 57.

```sh
npm ci
npm run ios
npm run android
npm run web
npm run typecheck
npm run lint
npm run export:web
npm run export:native
```

Expo generates iOS/Android native projects from app.json. Native exports go to dist-native; web exports go to dist. Bundling does not validate device execution or signing.

## Rust boundary

The optional `src/state/client.ts` exposes the existing shared Remote store and a subscription hook. `connectRemote(origin, accessKey)` initializes network transport; under Tauri it accepts an existing workspace path and uses the local Rust Harness. No visual entry point invokes it automatically.

The Remote adapter handles endpoint validation, session identity and cookies. Web must share the Rust Remote origin; native device authentication still needs device verification. No access key is persisted. Rust owns agent, session, tool, provider and filesystem logic. The existing `web` app remains operational and is the release default.

`../desktop` contains the optional Tauri host foundation for macOS, Windows and Linux, with the existing Rust Harness and runtime assets. It will render the blank app until a design is implemented.
