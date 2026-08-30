import Foundation
import HarnessCallCore

/// A tool call as it is presented in the conversation.  The bridge streams
/// tool arguments in chunks, so the view keeps the display string separate
/// from `HarnessCallCore.ToolCall`'s decoded JSON value.
struct ConversationToolCall: Identifiable, Equatable, Sendable {
    enum Status: String, Equatable, Sendable {
        case streaming
        case completed
        case failed

        var label: String {
            switch self {
            case .streaming: return "running"
            case .completed: return "done"
            case .failed: return "failed"
            }
        }
    }

    let id: UUID
    var index: Int?
    var callID: String?
    var name: String
    var arguments: String
    var status: Status

    init(
        id: UUID = UUID(),
        index: Int? = nil,
        callID: String? = nil,
        name: String,
        arguments: String = "",
        status: Status = .streaming
    ) {
        self.id = id
        self.index = index
        self.callID = callID
        self.name = name
        self.arguments = arguments
        self.status = status
    }

    init(index: Int? = nil, toolCall: ToolCall) {
        self.init(
            index: index,
            callID: toolCall.id,
            name: toolCall.name,
            arguments: toolCall.arguments.prettyJSONString,
            status: .completed
        )
    }
}
enum ConversationEntryKind: Equatable, Sendable {
    case user(String)
    case assistant(content: String, toolCalls: [ConversationToolCall])
    case toolCall(ConversationToolCall)
    case skill(name: String, content: String, status: String?)
    case mcp(server: String, name: String, content: String, isError: Bool)
    case system(String)
}

struct ConversationEntry: Identifiable, Equatable, Sendable {
    let id: UUID
    var kind: ConversationEntryKind

    init(id: UUID = UUID(), kind: ConversationEntryKind) {
        self.id = id
        self.kind = kind
    }
}
