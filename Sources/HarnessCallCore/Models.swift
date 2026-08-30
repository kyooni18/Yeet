import Foundation

public typealias ProviderID = String
public typealias ModelID = String

public enum MessageRole: String, Sendable, Codable {
    case system
    case user
    case assistant
    case tool
}

public struct ToolCall: Sendable, Equatable, Codable {
    public var id: String
    public var name: String
    public var arguments: JSONValue

    public init(id: String, name: String, arguments: JSONValue) {
        self.id = id
        self.name = name
        self.arguments = arguments
    }
}

public struct Message: Sendable, Equatable, Codable {
    public var role: MessageRole
    public var content: String?
    public var toolCalls: [ToolCall]?
    public var toolCallId: String?
    public var name: String?

    public init(
        role: MessageRole,
        content: String? = nil,
        toolCalls: [ToolCall]? = nil,
        toolCallId: String? = nil,
        name: String? = nil
    ) {
        self.role = role
        self.content = content
        self.toolCalls = toolCalls
        self.toolCallId = toolCallId
        self.name = name
    }

    public static func system(_ content: String) -> Message { Message(role: .system, content: content) }
    public static func user(_ content: String) -> Message { Message(role: .user, content: content) }
    public static func assistant(_ content: String, toolCalls: [ToolCall]? = nil) -> Message {
        Message(role: .assistant, content: content, toolCalls: toolCalls)
    }
    public static func tool(_ content: String, toolCallId: String, name: String? = nil) -> Message {
        Message(role: .tool, content: content, toolCallId: toolCallId, name: name)
    }
}

public struct ToolDefinition: Sendable, Equatable, Codable {
    public var name: String
    public var description: String?
    public var inputSchema: [String: JSONValue]

    public init(name: String, description: String? = nil, inputSchema: [String: JSONValue]) {
        self.name = name
        self.description = description
        self.inputSchema = inputSchema
    }
}

public enum ToolChoice: Sendable, Equatable, Codable {
    case auto
    case none
    case required
    case named(String)

    private enum CodingKeys: String, CodingKey { case name }

    public init(from decoder: Decoder) throws {
        if let value = try? decoder.singleValueContainer().decode(String.self) {
            switch value {
            case "auto": self = .auto
            case "none": self = .none
            case "required": self = .required
            default:
                throw DecodingError.dataCorrupted(
                    .init(codingPath: decoder.codingPath, debugDescription: "Unknown tool choice: \(value)")
                )
            }
            return
        }
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self = .named(try container.decode(String.self, forKey: .name))
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case .auto, .none, .required:
            var container = encoder.singleValueContainer()
            let value = switch self {
            case .auto: "auto"
            case .none: "none"
            case .required: "required"
            case .named: fatalError("unreachable")
            }
            try container.encode(value)
        case .named(let name):
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(name, forKey: .name)
        }
    }
}

public struct RetryPolicy: Sendable, Equatable, Codable {
    public var maxAttempts: Int?
    public var baseDelayMs: Int?
    public var maxDelayMs: Int?
    public var jitter: Double?

    public init(maxAttempts: Int? = nil, baseDelayMs: Int? = nil, maxDelayMs: Int? = nil, jitter: Double? = nil) {
        self.maxAttempts = maxAttempts
        self.baseDelayMs = baseDelayMs
        self.maxDelayMs = maxDelayMs
        self.jitter = jitter
    }
}

public struct CallRequest: Sendable, Equatable, Codable {
    public var model: ModelID
    public var messages: [Message]
    public var system: String?
    public var tools: [ToolDefinition]?
    public var toolChoice: ToolChoice?
    public var temperature: Double?
    public var maxTokens: Int?
    public var metadata: [String: String]?
    public var timeoutMs: Int?
    public var retry: RetryPolicy?
    public var providerOptions: [String: JSONValue]?

    public init(
        model: ModelID,
        messages: [Message],
        system: String? = nil,
        tools: [ToolDefinition]? = nil,
        toolChoice: ToolChoice? = nil,
        temperature: Double? = nil,
        maxTokens: Int? = nil,
        metadata: [String: String]? = nil,
        timeoutMs: Int? = nil,
        retry: RetryPolicy? = nil,
        providerOptions: [String: JSONValue]? = nil
    ) {
        self.model = model
        self.messages = messages
        self.system = system
        self.tools = tools
        self.toolChoice = toolChoice
        self.temperature = temperature
        self.maxTokens = maxTokens
        self.metadata = metadata
        self.timeoutMs = timeoutMs
        self.retry = retry
        self.providerOptions = providerOptions
    }
}

public struct Usage: Sendable, Equatable, Codable {
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var totalTokens: Int?
    public var cachedInputTokens: Int?

    public init(inputTokens: Int? = nil, outputTokens: Int? = nil, totalTokens: Int? = nil, cachedInputTokens: Int? = nil) {
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.totalTokens = totalTokens
        self.cachedInputTokens = cachedInputTokens
    }
}

public enum FinishReason: String, Sendable, Codable {
    case stop
    case length
    case toolCall = "tool_call"
    case contentFilter = "content_filter"
    case error
    case unknown
}

public struct CallResult: Sendable, Equatable, Codable {
    public var provider: ProviderID
    public var model: String
    public var id: String?
    public var text: String
    public var toolCalls: [ToolCall]
    public var finishReason: FinishReason
    public var usage: Usage?
    public var raw: JSONValue?

    public init(
        provider: ProviderID,
        model: String,
        id: String? = nil,
        text: String,
        toolCalls: [ToolCall] = [],
        finishReason: FinishReason,
        usage: Usage? = nil,
        raw: JSONValue? = nil
    ) {
        self.provider = provider
        self.model = model
        self.id = id
        self.text = text
        self.toolCalls = toolCalls
        self.finishReason = finishReason
        self.usage = usage
        self.raw = raw
    }
}

public enum StreamEvent: Sendable, Equatable, Codable {
    case start(provider: ProviderID, model: String, id: String?)
    case textDelta(String)
    case toolCallDelta(index: Int, id: String?, name: String?, argumentsDelta: String?)
    case toolCall(index: Int, toolCall: ToolCall)
    case finish(finishReason: FinishReason, usage: Usage?, raw: JSONValue?)

    private enum CodingKeys: String, CodingKey {
        case type, provider, model, id, delta, index, name, argumentsDelta, toolCall, finishReason, usage, raw
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)
        switch type {
        case "start":
            self = .start(
                provider: try container.decode(String.self, forKey: .provider),
                model: try container.decode(String.self, forKey: .model),
                id: try container.decodeIfPresent(String.self, forKey: .id)
            )
        case "text-delta":
            self = .textDelta(try container.decode(String.self, forKey: .delta))
        case "tool-call-delta":
            self = .toolCallDelta(
                index: try container.decode(Int.self, forKey: .index),
                id: try container.decodeIfPresent(String.self, forKey: .id),
                name: try container.decodeIfPresent(String.self, forKey: .name),
                argumentsDelta: try container.decodeIfPresent(String.self, forKey: .argumentsDelta)
            )
        case "tool-call":
            self = .toolCall(
                index: try container.decode(Int.self, forKey: .index),
                toolCall: try container.decode(ToolCall.self, forKey: .toolCall)
            )
        case "finish":
            self = .finish(
                finishReason: try container.decode(FinishReason.self, forKey: .finishReason),
                usage: try container.decodeIfPresent(Usage.self, forKey: .usage),
                raw: try container.decodeIfPresent(JSONValue.self, forKey: .raw)
            )
        default:
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "Unknown stream event type: \(type)")
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .start(let provider, let model, let id):
            try container.encode("start", forKey: .type)
            try container.encode(provider, forKey: .provider)
            try container.encode(model, forKey: .model)
            try container.encodeIfPresent(id, forKey: .id)
        case .textDelta(let delta):
            try container.encode("text-delta", forKey: .type)
            try container.encode(delta, forKey: .delta)
        case .toolCallDelta(let index, let id, let name, let argumentsDelta):
            try container.encode("tool-call-delta", forKey: .type)
            try container.encode(index, forKey: .index)
            try container.encodeIfPresent(id, forKey: .id)
            try container.encodeIfPresent(name, forKey: .name)
            try container.encodeIfPresent(argumentsDelta, forKey: .argumentsDelta)
        case .toolCall(let index, let toolCall):
            try container.encode("tool-call", forKey: .type)
            try container.encode(index, forKey: .index)
            try container.encode(toolCall, forKey: .toolCall)
        case .finish(let finishReason, let usage, let raw):
            try container.encode("finish", forKey: .type)
            try container.encode(finishReason, forKey: .finishReason)
            try container.encodeIfPresent(usage, forKey: .usage)
            try container.encodeIfPresent(raw, forKey: .raw)
        }
    }
}
