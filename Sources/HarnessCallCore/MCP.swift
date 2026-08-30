import Foundation

public enum MCPTransport: String, Sendable, Equatable, Codable {
    case stdio
    case http
}

public struct MCPServerConfiguration: Sendable, Equatable, Codable {
    public var name: String
    public var transport: MCPTransport
    public var command: String?
    public var args: [String]?
    public var env: [String: String]?
    public var cwd: String?
    public var url: String?
    public var headers: [String: String]?

    public init(
        name: String,
        transport: MCPTransport,
        command: String? = nil,
        args: [String]? = nil,
        env: [String: String]? = nil,
        cwd: String? = nil,
        url: String? = nil,
        headers: [String: String]? = nil
    ) {
        self.name = name
        self.transport = transport
        self.command = command
        self.args = args
        self.env = env
        self.cwd = cwd
        self.url = url
        self.headers = headers
    }

    public static func stdio(
        name: String,
        command: String,
        args: [String] = [],
        env: [String: String] = [:],
        cwd: String? = nil
    ) -> Self {
        .init(name: name, transport: .stdio, command: command, args: args, env: env, cwd: cwd)
    }

    public static func http(name: String, url: String, headers: [String: String] = [:]) -> Self {
        .init(name: name, transport: .http, url: url, headers: headers)
    }
}

public struct MCPServerStatus: Sendable, Equatable, Codable {
    public var name: String
    public var transport: MCPTransport
    public var command: String?
    public var args: [String]?
    public var env: [String: String]?
    public var cwd: String?
    public var url: String?
    public var headers: [String: String]?
    public var connected: Bool
    public var protocolVersion: String?
    public var era: String?

    private enum CodingKeys: String, CodingKey {
        case name, transport, command, args, env, cwd, url, headers, connected, era
        case protocolVersion = "protocol"
    }
}

public struct MCPTool: Sendable, Equatable, Codable {
    public var server: String
    public var name: String
    public var qualifiedName: String
    public var title: String?
    public var description: String?
    public var inputSchema: [String: JSONValue]
    public var outputSchema: [String: JSONValue]?
    public var annotations: [String: JSONValue]?
}

public struct MCPResource: Sendable, Equatable, Codable {
    public var server: String
    public var uri: String
    public var name: String
    public var title: String?
    public var description: String?
    public var mimeType: String?
}

public struct MCPPromptArgument: Sendable, Equatable, Codable {
    public var name: String
    public var description: String?
    public var required: Bool?
}

public struct MCPPrompt: Sendable, Equatable, Codable {
    public var server: String
    public var name: String
    public var qualifiedName: String
    public var title: String?
    public var description: String?
    public var arguments: [MCPPromptArgument]?
}

public struct MCPCallToolResult: Sendable, Equatable, Codable {
    public var content: [JSONValue]
    public var structuredContent: JSONValue?
    public var isError: Bool?
    public var metadata: [String: JSONValue]?

    private enum CodingKeys: String, CodingKey {
        case content, structuredContent, isError
        case metadata = "_meta"
    }
}

public struct MCPReadResourceResult: Sendable, Equatable, Codable {
    public var contents: [JSONValue]
    public var metadata: [String: JSONValue]?

    private enum CodingKeys: String, CodingKey {
        case contents
        case metadata = "_meta"
    }
}

public struct MCPGetPromptResult: Sendable, Equatable, Codable {
    public var description: String?
    public var messages: [JSONValue]
    public var metadata: [String: JSONValue]?

    private enum CodingKeys: String, CodingKey {
        case description, messages
        case metadata = "_meta"
    }
}
