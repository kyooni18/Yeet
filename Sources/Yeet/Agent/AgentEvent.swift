import Foundation
import HarnessCallCore

enum AgentEvent: Sendable {
    case start(provider: String, model: String, id: String?)
    case textDelta(String)
    case toolCallDelta(index: Int, id: String?, name: String?, argumentsDelta: String?)
    case toolCall(index: Int, call: ToolCall)
    case system(String)
    /// A model response finished. Usage is optional because some providers do
    /// not return token accounting for every response (especially streamed
    /// responses).
    case finished(reason: FinishReason, usage: Usage?)
}
