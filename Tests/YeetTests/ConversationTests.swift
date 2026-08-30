import Testing
@testable import Yeet
@testable import SwiftTUICore
@testable import SwiftTUIRuntime
@testable import SwiftTUIViews

@Test
func markdownParserRecognizesCommonModelOutput() {
    let blocks = MarkdownParser.parse(
        "# Heading\n\n- **bold** item\n- `code` item\n\n```swift\nlet answer = 42\n```"
    )

    #expect(blocks == [
        .heading(level: 1, text: "Heading"),
        .unorderedList(["**bold** item", "`code` item"]),
        .code(language: "swift", text: "let answer = 42")
    ])

    let runs = MarkdownParser.inlineRuns("**bold** and *italic* with `code`")
    #expect(runs.map(\.text) == ["bold", " and ", "italic", " with ", "code"])
    #expect(runs[0].style == .bold)
    #expect(runs[2].style == .italic)
    #expect(runs[4].style == .code)
}

@Test
func markdownParserUnescapesMarkersOnce() {
    let runs = MarkdownParser.inlineRuns(#"literal \* marker"#)

    #expect(runs.map(\.text).joined() == "literal * marker")
}

@MainActor
@Test
func slashCommandsDoNotBecomeUserMessages() {
    let session = Session()
    session.inputText = "/help"
    session.submit()

    #expect(session.conversation.allSatisfy {
        if case .user = $0.kind { return false }
        return true
    })
    #expect(session.conversation.contains {
        if case .system = $0.kind { return true }
        return false
    })
}

@MainActor
@Test
func conversationRendersUserOnTheRightAndModelOnTheLeft() {
    var environment = EnvironmentValues()
    environment.terminalSize = CellSize(width: 42, height: 14)
    let entries = [
        ConversationEntry(kind: .user("hello")),
        ConversationEntry(kind: .assistant(content: "**Welcome**\n\n- markdown", toolCalls: []))
    ]

    let artifacts = DefaultRenderer().render(
        ConversationView(entries: entries, scrollRevision: entries.count),
        context: .init(
            identity: Identity(components: ["ConversationTest"]),
            environmentValues: environment
        )
    )

    print(artifacts.rasterSurface.lines)
    #expect(artifacts.rasterSurface.lines.contains { $0.contains("hello") })
    #expect(artifacts.rasterSurface.lines.contains { $0.contains("Welcome") })
}
