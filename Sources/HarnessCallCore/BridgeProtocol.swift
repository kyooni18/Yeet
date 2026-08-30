import Foundation

public let harnessCallCoreBridgeProtocolVersion = 1

public struct OpenAICompatibleProviderConfiguration: Sendable, Equatable, Codable {
    let kind: String
    public var id: String
    public var baseUrl: String
    public var apiKey: String?
    public var headers: [String: String]?
    public var requireApiKey: Bool?

    public init(
        id: String,
        baseUrl: String,
        apiKey: String? = nil,
        headers: [String: String]? = nil,
        requireApiKey: Bool? = nil
    ) {
        self.kind = "openai-compatible"
        self.id = id
        self.baseUrl = baseUrl
        self.apiKey = apiKey
        self.headers = headers
        self.requireApiKey = requireApiKey
    }
}

public enum AuthMethod: String, Sendable, Equatable, Codable {
    case none
    case apiKey = "api-key"
    case browser
    case environment
}

public struct AuthStatus: Sendable, Equatable, Codable {
    public var provider: String
    public var authenticated: Bool
    public var method: AuthMethod
    public var expiresAt: String?
    public var configDir: String

    public init(provider: String, authenticated: Bool, method: AuthMethod, expiresAt: String? = nil, configDir: String) {
        self.provider = provider
        self.authenticated = authenticated
        self.method = method
        self.expiresAt = expiresAt
        self.configDir = configDir
    }
}

public struct BrowserLoginOptions: Sendable, Equatable, Codable {
    public var clientId: String?
    public var clientSecret: String?
    public var projectId: String?
    public var scopes: [String]?
    public var timeoutMs: Int?

    public init(
        clientId: String? = nil,
        clientSecret: String? = nil,
        projectId: String? = nil,
        scopes: [String]? = nil,
        timeoutMs: Int? = nil
    ) {
        self.clientId = clientId
        self.clientSecret = clientSecret
        self.projectId = projectId
        self.scopes = scopes
        self.timeoutMs = timeoutMs
    }
}

public struct RemoteBridgeError: Error, Sendable, Equatable, Codable, CustomStringConvertible {
    public var name: String
    public var message: String
    public var stack: String?
    public var provider: String?
    public var retryable: Bool?
    public var status: Int?
    public var responseBody: String?
    public var requestId: String?
    public var code: Int?
    public var server: String?

    public var description: String {
        if let provider { return "\(name) [\(provider)]: \(message)" }
        if let server { return "\(name) [MCP \(server)]: \(message)" }
        return "\(name): \(message)"
    }
}

public enum CallCoreBridgeError: Error, Sendable, CustomStringConvertible {
    case failedToStart(String)
    case bridgeExited(status: Int32, stderr: String)
    case protocolError(String)
    case remote(RemoteBridgeError)
    case transport(String)

    public var description: String {
        switch self {
        case .failedToStart(let message): return "Failed to start call-core bridge: \(message)"
        case .bridgeExited(let status, let stderr):
            return "Call-core bridge exited with status \(status)\(stderr.isEmpty ? "" : ": \(stderr)")"
        case .protocolError(let message): return "Call-core bridge protocol error: \(message)"
        case .remote(let error): return error.description
        case .transport(let message): return "Call-core bridge transport error: \(message)"
        }
    }
}

struct IncomingEnvelope: Decodable {
    var v: Int
    var id: String
    var type: String
    var providers: [String]?
    var models: [String]?
    var provider: String?
    var removed: Bool?
    var result: CallResult?
    var event: StreamEvent?
    var error: RemoteBridgeError?
    var target: String?
    var path: String?
    var status: AuthStatus?

    var skills: [SkillSummary]?
    var skill: Skill?
    var content: String?

    var servers: [MCPServerStatus]?
    var server: MCPServerConfiguration?
    var tools: [MCPTool]?
    var toolResult: MCPCallToolResult?
    var resources: [MCPResource]?
    var resourceResult: MCPReadResourceResult?
    var prompts: [MCPPrompt]?
    var promptResult: MCPGetPromptResult?
}

private struct BaseCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op: String
}

private struct RequestCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op: String
    let request: CallRequest
}

private struct RegisterCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "register-provider"
    let provider: OpenAICompatibleProviderConfiguration
}

private struct UnregisterCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "unregister-provider"
    let provider: String
}

private struct ProviderCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op: String
    let provider: String
}

private struct SetAPIKeyCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "auth-set-api-key"
    let provider: String
    let apiKey: String
}

private struct BrowserLoginCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "auth-login-browser"
    let provider: String
    let options: BrowserLoginOptions?
}

private struct SkillCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op: String
    let skill: String
    let path: String?
}

private struct MCPSetServerCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "mcp-set-server"
    let server: MCPServerConfiguration
}

private struct MCPServerCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op: String
    let server: String?
}

private struct MCPCallToolCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "mcp-call-tool"
    let server: String
    let tool: String
    let arguments: [String: JSONValue]?
}

private struct MCPReadResourceCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "mcp-read-resource"
    let server: String
    let uri: String
}

private struct MCPGetPromptCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "mcp-get-prompt"
    let server: String
    let prompt: String
    let arguments: [String: String]?
}

private struct CancelCommand: Encodable {
    let v = harnessCallCoreBridgeProtocolVersion
    let id: String
    let op = "cancel"
    let target: String
}

func encodeBaseCommand(id: String, op: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(BaseCommand(id: id, op: op))
}

func encodeRequestCommand(id: String, op: String, request: CallRequest, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(RequestCommand(id: id, op: op, request: request))
}

func encodeRegisterCommand(id: String, provider: OpenAICompatibleProviderConfiguration, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(RegisterCommand(id: id, provider: provider))
}

func encodeUnregisterCommand(id: String, provider: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(UnregisterCommand(id: id, provider: provider))
}

func encodeProviderCommand(id: String, op: String, provider: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(ProviderCommand(id: id, op: op, provider: provider))
}

func encodeSetAPIKeyCommand(id: String, provider: String, apiKey: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(SetAPIKeyCommand(id: id, provider: provider, apiKey: apiKey))
}

func encodeBrowserLoginCommand(id: String, provider: String, options: BrowserLoginOptions?, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(BrowserLoginCommand(id: id, provider: provider, options: options))
}

func encodeSkillCommand(id: String, op: String, skill: String, path: String? = nil, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(SkillCommand(id: id, op: op, skill: skill, path: path))
}

func encodeMCPSetServerCommand(id: String, server: MCPServerConfiguration, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(MCPSetServerCommand(id: id, server: server))
}

func encodeMCPServerCommand(id: String, op: String, server: String?, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(MCPServerCommand(id: id, op: op, server: server))
}

func encodeMCPCallToolCommand(
    id: String,
    server: String,
    tool: String,
    arguments: [String: JSONValue]?,
    encoder: JSONEncoder
) throws -> Data {
    try encoder.encode(MCPCallToolCommand(id: id, server: server, tool: tool, arguments: arguments))
}

func encodeMCPReadResourceCommand(id: String, server: String, uri: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(MCPReadResourceCommand(id: id, server: server, uri: uri))
}

func encodeMCPGetPromptCommand(
    id: String,
    server: String,
    prompt: String,
    arguments: [String: String]?,
    encoder: JSONEncoder
) throws -> Data {
    try encoder.encode(MCPGetPromptCommand(id: id, server: server, prompt: prompt, arguments: arguments))
}

func encodeCancelCommand(id: String, target: String, encoder: JSONEncoder) throws -> Data {
    try encoder.encode(CancelCommand(id: id, target: target))
}
