# UI layout specifications

Start with `ContentView.ui`. Edit meaningful regions here when asking an AI agent to change the design; the agent reads these files and updates normal frontend components. These files are never loaded by the application.

Indentation describes containment. `VStack`, `HStack`, `ZStack`, `Scroll`, `List`, `Split`, `Group`, and `Spacer` describe simple layout intent. Names such as `SessionHeader` refer to meaningful implementation regions, not runtime tokens. Properties such as width, height, minWidth, maxWidth, grow, padding, spacing, align, visible, and optional are plain design notes. Visibility names describe intent; frontend code supplies the actual conditions. Platform thresholds, interaction details, accessibility and network behavior belong in implementation.

Do not create files for every button or text primitive. No schema tooling, parser, code generator, or SwiftUI clone is needed.
