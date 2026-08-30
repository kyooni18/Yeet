import SwiftTUI

struct ConversationView: View {
    let entries: [ConversationEntry]
    let scrollRevision: Int

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView(.vertical) {
                // Keep the transcript on the eager stack path. SwiftTUI's
                // lazy scroll estimates can diverge from its cold layout
                // shadow while streamed assistant content is growing, which
                // promotes a framework diagnostic to SIGTRAP in debug builds.
                // Conversation history is intentionally bounded to the
                // active session, so the eager stack is the safer trade-off.
                VStack(alignment: .leading, spacing: 1) {
                    if entries.isEmpty {
                        Text("Start a conversation…")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    } else {
                        ForEach(entries) { entry in
                            ConversationEntryView(entry: entry)
                                .id(entry.id)
                        }
                    }
                }
                .padding(.horizontal, 1)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .onChange(of: scrollRevision, initial: true) {
                _ = proxy.scrollTo(edge: .bottom)
            }
        }
    }
}
private struct ConversationEntryView: View {
    let entry: ConversationEntry

    @ViewBuilder
    var body: some View {
        switch entry.kind {
        case .user(let content):
            HStack(spacing: 0) {
                Spacer(minLength: 3)
                Text(content)
                    .foregroundStyle(.primary)
                    .padding(.init(horizontal: 2, vertical: 1))
                    .background(.tint.opacity(0.24))
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        case .assistant(let content, let toolCalls):
            ModelMessageView(content: content, toolCalls: toolCalls)
        case .toolCall(let call):
            ToolCallView(call: call)
        case .skill(let name, let content, let status):
            SkillEventView(name: name, content: content, status: status)
        case .mcp(let server, let name, let content, let isError):
            MCPEventView(server: server, name: name, content: content, isError: isError)
        case .system(let content):
            Text(content)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct ModelMessageView: View {
    let content: String
    let toolCalls: [ConversationToolCall]

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if !content.isEmpty {
                ForEach(Array(MarkdownParser.parse(content).enumerated()), id: \.offset) { _, block in
                    MarkdownBlockView(block: block)
                }
            } else if toolCalls.isEmpty {
                Text("▌")
                    .foregroundStyle(.tint)
            }

            ForEach(toolCalls) { call in
                ToolCallView(call: call)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 1)
    }
}

private struct MarkdownBlockView: View {
    let block: MarkdownBlock

    @ViewBuilder
    var body: some View {
        switch block {
        case .paragraph(let text):
            MarkdownTextRenderer.text(text)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .heading(let level, let text):
            MarkdownTextRenderer.text(text)
                .bold()
                .foregroundStyle(level == 1 ? .tint : .primary)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .unorderedList(let items):
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    MarkdownTextRenderer.text(item, prefix: "• ")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .orderedList(let items):
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    MarkdownTextRenderer.text(item, prefix: "\(index + 1). ")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .quote(let text):
            MarkdownTextRenderer.text(text, prefix: "│ ")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .code(let language, let text):
            VStack(alignment: .leading, spacing: 0) {
                if let language, !language.isEmpty {
                    Text(language)
                        .foregroundStyle(.cyan)
                        .bold()
                }
                Text(text.isEmpty ? " " : text)
                    .foregroundStyle(.secondary)
            }
            .padding(.init(horizontal: 1, vertical: 0))
            .background(.background)
            .border(.separator)
            .frame(maxWidth: .infinity, alignment: .leading)
        case .thematicBreak:
            Divider()
        }
    }

}

private struct ToolCallView: View {
    let call: ConversationToolCall

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 1) {
                Text("tool")
                    .foregroundStyle(.tint)
                    .bold()
                Text(call.name)
                Spacer(minLength: 1)
                Text(call.status.label)
                    .foregroundStyle(call.status == .failed ? .danger : .secondary)
            }
            if !call.arguments.isEmpty {
                Text(call.arguments)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 2)
            }
        }
        .padding(.init(horizontal: 1, vertical: 0))
        .background(.background)
        .border(.separator)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct SkillEventView: View {
    let name: String
    let content: String
    let status: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 1) {
                Text("skill")
                    .foregroundStyle(.magenta)
                    .bold()
                Text(name)
                if let status {
                    Text(status)
                        .foregroundStyle(.secondary)
                }
            }
            if !content.isEmpty {
                MarkdownTextRenderer.text(content)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.init(horizontal: 1, vertical: 0))
        .border(.separator)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct MCPEventView: View {
    let server: String
    let name: String
    let content: String
    let isError: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 1) {
                Text("MCP")
                    .foregroundStyle(isError ? .danger : .info)
                    .bold()
                Text("\(server)/\(name)")
            }
            if !content.isEmpty {
                Text(content)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 2)
            }
        }
        .padding(.init(horizontal: 1, vertical: 0))
        .border(.separator)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
