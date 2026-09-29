# TUI Mockup Report

Design report for the Yeet TUI mockup (`~/Desktop/TUI-Mockup.fig`, OpenPencil, 1440×900 frames, JetBrains Mono 12 / line height 20). The mockup is design-only. No source code was changed.

## 1. Views

| View | Purpose |
|---|---|
| Home | Overview and entry point. Pinned first tab, separated by a divider. |
| Session | One conversation: messages, reasoning summaries, tool usage, composer. |
| Files | Places, recent folders, file tree. The reference for file icons. |
| Agents | Agent-group view: manage a group of cooperating agents. |
| Agent detail | One agent from a group: status, activity, gentle steering. |
| Diff | Changed-file tree, unified diff, inspector. |
| Issue | Issue view. Still uses old glyphs. |

## 2. Layout skeleton (shared by every view)

- Titlebar (36px), tab bar (34px), then rail / main / optional inspector.
- The composer is attached to the bottom in every view (36px). Only the status bar floats, and it hugs its content.
- Rail is 232px wide. Rows are 28px, with a `bg.selected` band on the selected row.
- Tab icons: home, message, folder, robot, git-pull-request, square-alert. The active tab is bold and bright.

## 3. Decisions by view

### Session
- No checkmarks on tool status.
- A row of 2–3 icons sits to the left of each reasoning summary, with tight padding, and the text follows the icons.
- Tool icons have no color. A failed tool uses a subtle muted red.
- Icons show the kind of action, not the file type: pencil for edit, book for read, magnifier for search. Web reads say `read`, not `fetch`.
- The spinner is a text glyph that cycles `| / - \`. It is a TUI spinner, not a UI one.
- Status bar is symmetrical and hugs its content.

### Agents (group view)
- The rail shows groups, with the group's agents as **direct children** and another group below. Groups collapse with a chevron.
- Selecting an agent switches to Agent detail.
- The action row is Steer, Pause, Add agent, Stop. A divider sits under it so the buttons read as a toolbar.
- One single feed lists activity and the messages between agents in the same list. Message rows use a message icon and name both agents.
- The inspector shows the group's state and a "needs you" block.
- Removed: a "how they work together" block, and separate AGENTS and MESSAGES sections.

### Agent detail
- Same layout as the group view, filtered to one agent. Actions: Steer, Pause, Reassign, Stop.
- Steering is gentle: the composer is the steering input, so the user adds information instead of issuing commands.
- The sidebar shows state, task, model, tokens, elapsed time and who the agent sends to or gets from.

### Diff
- Removed lines have a subtle red band, added lines a green band. The text keeps its red or green color.
- The file tree matches the other rails: chevron and folder icon, indented files with file-type icons, no ASCII branch lines.

## 4. Icon system

- **Chrome** (tabs, composer, chevrons, actions, message icons): `pixelarticons`, 16px. One stroke weight, and none of them is a first-letter icon.
- **File-type icons** (Files, Diff tree, Agents feed): `catppuccin`, 14px.
- The intended target is Nerd Font glyphs. OpenPencil cannot place them, so vector equivalents stand in. The real TUI should use Nerd Font codepoints.
- Colored file-type icons conflict with theming (see section 6). Prefer monochrome glyphs in the real TUI.

## 5. Copy rules

- Avoid explanatory labels and comments. Removed: subtitles, the second goal sentence, ACTIVITY and GROUPS headers, and keyboard hints.
- Kept: inspector section headers, because they separate kinds of information rather than explain anything.
- Avoid status noise such as "Healthy" or "Connected". Show a state only when it needs the user.

## 6. Color and custom themes

The mockups are based on Kanagawa (wave). Yeet supports custom themes, so the real UI must not hardcode hex values.

### Current colors and their source

| Hex | Role | Kanagawa |
|---|---|---|
| `#16161D` | main background | sumiInk0 |
| `#1F1F28` | rail, sidebar, inspector | sumiInk3 |
| `#2A2A37` | user message | sumiInk4 |
| `#54546D` | line numbers | sumiInk6 |
| `#DCD7BA` | bright text | fujiWhite |
| `#C8C093` | code identifiers | oldWhite |
| `#727169` | dim text | fujiGray |
| `#7E9CD8` | accent | crystalBlue |
| `#76946A` | added lines | autumnGreen |
| `#C34043` | removed lines | autumnRed |
| `#A6A69C`, `#252633`, `#2D2E3D`, `#2A2B38`, `#3D4470`, `#4A4C66`, `#8A5558` | normal text, hover, selected, divider, active selection, stroke, muted failure | not Kanagawa. Derived by eye. |

### Token set

**Authored by the theme (12):** `bg`, `bg.raised`, `fg`, `fg.muted`, `fg.dim`, `accent`, `add`, `remove`, `danger`, `warn`, `code.ident`, `border`.

**Derived by the app (overridable):**

- `bg.hover` = `bg.raised` mixed 6% toward `fg`.
- `bg.selected` = `bg.raised` mixed 12% toward `fg`.
- `bg.selected.active` = `accent` at 25% over `bg`.
- `bg.message` = `bg` mixed 5% toward `fg`.
- `add.bg` = `add` at 16% over `bg`.
- `remove.bg` = `remove` at 16% over `bg`.
- `danger.subtle` = `danger` mixed 40% toward `bg`.
- `fg.muted` and `fg.dim` can also be derived from `fg` and `bg`.

A three-color theme (`bg`, `fg`, `accent`) is therefore valid.

### Application rules

1. Text has three levels. `fg` marks what needs attention now, `fg.muted` is the default, and `fg.dim` is for metadata and hints.
2. Icons inherit text color and never carry their own hue. The exception is a failed tool, which uses `danger.subtle`.
3. `accent` marks one thing at a time (ctx bar, links). It is not a second selection color.
4. Only `add` and `remove` mean green and red. Failures use `danger`.
5. Change state uses tinted backgrounds computed from `bg`, not solid fills. Text on a band stays `add` or `remove`.
6. Surfaces have three steps: `bg`, `bg.raised`, `bg.selected`. Dividers are `border` at 1px.
7. No gradients on main components.

### Theme file shape

```toml
name = "kanagawa-wave"
mode = "dark"          # drives how derived tokens mix

[colors]
bg         = "#16161D"
bg_raised  = "#1F1F28"
fg         = "#DCD7BA"
fg_muted   = "#C8C093"   # optional
fg_dim     = "#727169"
accent     = "#7E9CD8"
add        = "#76946A"
remove     = "#C34043"
danger     = "#C34043"
warn       = "#DCA561"
border     = "#2A2A37"
code_ident = "#C8C093"

[overrides]              # any derived token, optional
# bg_selected = "#2D2E3D"
```

### Checks before shipping a theme

- Contrast: `fg.muted` on `bg.raised` at least 4.5:1, and `fg.dim` at least 3:1. A theme that fails either gets a warning.
- Terminals have no alpha, so derived tokens must resolve to opaque hex at load time. Do this before any 256-color fallback.
- Light themes flip the mix direction, which is why `mode` is in the file.

## 7. Open items

- Home, Issue and the Diff inspector still use old text glyphs (`?`, `●`, `@`, `$`, `Δ`) and hardcoded hues.
- The Agents feed still uses catppuccin file-type icons, not the generic pencil/book/magnifier set from Session.
- The Agents main view has an empty gap between the action row and the feed, left by the removed labels.
- The mockup was not re-verified after the duplicate frames were removed, apart from the Diff and Session renders.
- Item 10 of the earlier Session refinement list was never confirmed.
