# Frontend foundation

The current scope is a native JavaScript UI foundation, with no actual UI implementation. The user will design the application from scratch in OpenPencil. Do not invent screens, visual components, a theme, navigation flows or layouts before the user supplies or requests the design.

`frontend/react-native` is an intentionally blank Expo / React Native app for iOS, iPadOS, Android and web. `App.tsx` exports ContentView; the Expo Router entry mounts it. `frontend/desktop` is the optional Tauri 2 host for its web export on macOS, Windows and Linux.

`app/ui/ContentView.ui` is reserved for the design specification. Future `.ui` files remain human/AI-readable design intent, without any parser, compiler, runtime interpreter or generic renderer. When a design is supplied, agents read the relevant specifications and implement normal components. Platform details remain in implementation code.

`frontend/shared` reuses the existing Remote protocol, streaming store and transport logic. The blank app has an optional client adapter but no automatic connection or UI. Rust remains in the root crate and owns all business logic. Desktop uses the existing Harness through narrow commands/events, stages existing RuntimeSource resources, and requires Node for the provider bridge. No Yeet-MCP is used.

The existing `web` client and release/install workflows remain operational. `web/dist` remains the default embedded frontend; YEET_FRONTEND_DIST allows an explicit alternate export at build time. This is a foundation for later user-directed design implementation, not a replacement of the working UI.

The in-progress multiplatform refactor introduces Rust `UI/` for application
state, navigation, semantic views, actions and composition, with runtime service
and resource ownership under `Harness/`. The compatibility browser shell and
TUI already consume shared view composition and workspace ordering. Remote v1
and the Tauri bridge expose per-client/host UI actions and projections separately
from Harness events and replay cursors. `frontend/shared` carries those contracts
and transport/store delivery; it does not define a second application reducer.
See `docs/MULTIPLATFORM_MIGRATION.md` for verified progress and remaining gaps.
The Expo app stays blank while its user-directed visual design is pending.
