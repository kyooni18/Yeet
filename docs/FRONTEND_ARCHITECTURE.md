# Frontend foundation

The current scope is a native JavaScript UI foundation, with no actual UI implementation. The user will design the application from scratch in OpenPencil. Do not invent screens, visual components, a theme, navigation flows or layouts before the user supplies or requests the design.

`frontend/react-native` is an intentionally blank Expo / React Native app for iOS, iPadOS, Android and web. `App.tsx` exports ContentView; the Expo Router entry mounts it. Native desktop application support has been removed. Desktop users use the CLI/TUI or browser Remote client.

`app/ui/ContentView.ui` is reserved for the design specification. Future `.ui` files remain human/AI-readable design intent, without any parser, compiler, runtime interpreter or generic renderer. When a design is supplied, agents read the relevant specifications and implement normal components. Platform details remain in implementation code.

`frontend/shared` reuses the existing Remote protocol, streaming store and transport logic. The blank app has an optional client adapter but no automatic connection or UI. Rust remains in the root crate and owns all business logic. Expo clients connect to a running Rust Remote server; there is no local GUI bridge or desktop runtime staging. No Yeet-MCP is used.

The existing `web` compatibility client remains available as an experimental opt-in source build. `YEET_EXPERIMENTAL_WEBUI=1` embeds `web/dist`; `YEET_FRONTEND_DIST` explicitly selects an alternate export. Standard installers and release builds omit the WebUI. This is a foundation for later user-directed design implementation, not a replacement of the working UI.

The in-progress multiplatform refactor introduces Rust `UI/` for application
state, navigation, semantic views, actions and composition, with runtime service
and resource ownership under `Harness/`. The compatibility browser shell and
TUI already consume shared view composition and workspace ordering. Remote v1
exposes per-client UI actions and projections separately
from Harness events and replay cursors. `frontend/shared` carries those contracts
and transport/store delivery; it does not define a second application reducer.
See `docs/MULTIPLATFORM_MIGRATION.md` for verified progress and remaining gaps.
The Expo app stays blank while its user-directed visual design is pending.
