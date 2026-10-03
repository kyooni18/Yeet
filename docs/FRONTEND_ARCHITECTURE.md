# Frontend architecture and migration

`app/ui/ContentView.ui` is the design entry point. All `app/ui/**/*.ui` files describe human/AI-readable intent. Agents read them first and implement normal components; there is no runtime format, parser or renderer. Edit specifications when requesting design changes, then ask an agent to synchronize the implementation.

`frontend/react-native` is the primary Expo / React Native application for iOS, iPadOS, Android and web. `frontend/desktop` hosts its web export in Tauri 2 for macOS, Windows and Linux. `frontend/shared` contains reusable protocol/state/transport code, rather than a second backend. Rust remains in the existing root crate; moving it into crates/ would add churn without improving the boundary.

The existing `web` application stays available during migration. Its production asset path `web/dist` remains the default until parity is verified. Existing release/install scripts continue to work. The new application is additive and does not remove the terminal UI or provider runtime.

## Boundaries

Rust owns semantic state, sessions, tools, filesystem, providers, permissions and agent execution. Network clients use Remote JSON protocol v1; the desktop host wraps the existing Rust Harness directly. TypeScript manages presentation, subscriptions, drafts and platform interactions only.

Keep snapshots, streaming reconciliation, sequence checks and reconnection in the shared client. Platform adapters own URL resolution, authentication/cookies, persistence, connectivity, timers, document picking, keyboard and clipboard behavior. Browser passkeys remain browser-specific. No filesystem browser should claim support until the core exposes browsing operations.

## Migration checkpoints

1. Establish layout specifications and repository agent rules.
2. Extract reusable client contracts without changing legacy web behavior.
3. Add Expo screens and native/web adapters, preserving semantic behavior.
4. Add Tauri Harness integration and verify web export and host builds.
5. Validate compatibility and document any platform validation limits before changing release defaults.

Older project plans using SwiftUI/Compose as the primary UI stack are superseded by this Expo direction. Preserve native UX through platform adapters, rather than describing implementation details in `.ui` files.
