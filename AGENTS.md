# Yeet repository instructions

Current scope is a blank native JavaScript frontend foundation. The user will create the UI in OpenPencil. Do not create screens, components, themes, navigation flows or proposed layouts until the user provides or requests the design.

Read `app/ui/ContentView.ui` and the relevant `app/ui/**/*.ui` files before changing frontend implementation. These files are the source of truth for structure and layout. Inspect the existing implementation, then update normal components to match the specifications. Modify specifications only when the user asks for layout or design changes.

The `.ui` notation is an AI-readable design document. Never add a parser, compiler, runtime interpreter, generic renderer, or primitive-per-file framework unless explicitly requested. Platform behavior, accessibility, endpoints, authentication, storage, and rendering details belong in implementation code. Preserve sensible native platform behavior.

Primary frontend: `frontend/react-native` (React Native / Expo). Desktop: React Native Web in Tauri 2. Keep `web` as the working compatibility client until migration parity is verified. Keep Rust core and existing Remote protocol authoritative; do not reimplement agent, tool, filesystem, or session business logic in TypeScript. Read `docs/FRONTEND_ARCHITECTURE.md` for migration boundaries.

Use Foundation Memory to recall and preserve durable project knowledge, with project tags and context. Use Jev for focused investigation and verification when available. Do not use Yeet-MCP for this frontend migration.

Make incremental changes, verify relevant behavior, and commit meaningful checkpoints. Preserve unrelated work in the checkout. Add only tests that validate important behavior or prevent meaningful regressions; avoid excessive compile-time tests.
