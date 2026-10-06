# Theme ownership

`UI/theme.rs` owns Yeet semantic colors, built-in palettes, catalog presentation,
role mapping, harmonization, fallback palettes and effective appearance. It has
no filesystem or native toolkit APIs. Hosts supply observed system appearance
and neutral loaded resources to its pure projection functions.

`Harness/theme_resources.rs` owns reference aliases and filesystem resource
loading: home expansion, deterministic JSON/Lua directory scanning and named
color extraction. Imported colors do not select Yeet semantic roles. Configuration
validation uses this resource boundary; persistence remains in ConfigStore.

RuntimeSettingsState retains the existing palette/catalog DTO fields for wire
compatibility. Their DTO shapes are neutral data under Harness resources, but
Harness no longer computes their appearance values. Runtime defaults and settings
refreshes publish requested preferences with empty compatibility fields.

`src/theme.rs` is a host compatibility adapter. ThemeProjectionCache loads
resources through Harness and invokes shared UI palette projection. Remote,
Tauri and TUI adapters hydrate settings before publishing or applying appearance.
The cache avoids custom-theme filesystem reads on each streaming update;
accepted RequestSettings, SetTheme and SetAppearance commands invalidate it so
refreshing the same path reloads modified resources. Existing wire field names,
aliases and fallback warnings remain compatible.

TUI reports its terminal appearance observation and maps shared RGB colors to
Ratatui. Its automatic appearance keeps the existing dark fallback, with
YEET_THEME_MODE overriding the preference. Web keeps its current application
appearance styling while receiving the legacy resolved palette/catalog fields.
The blank Expo application remains unchanged.

This is an incremental ownership seam, not complete visual parity. The legacy
runtime/wire DTOs can eventually separate, and all graphical renderers still need
to consume shared appearance projections consistently. The rest of the
multiplatform migration remains tracked in MULTIPLATFORM_MIGRATION.md.

Verification for checkpoint aa5e4ad used an exact staged export. All-target
compilation passed, as did ten theme behavior tests, five Remote checks, three
Tauri checks and the native renderer check. Source-layout checks passed five of
six gates; the remaining RuntimeSource gate reported the unchanged committed
`RuntimeSource/src/auth.ts` at 1,205 lines against its 1,200-line ceiling. Both
that source and the guard matched HEAD/index/export byte for byte. The unrelated
worktree authentication edits were excluded, and no ceiling was raised.
