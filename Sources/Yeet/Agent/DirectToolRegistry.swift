import Foundation
import HarnessCallCore

struct CapabilityDescriptor: Equatable, Sendable {
    enum Kind: String, Sendable { case workspace, skill, mcp }
    let id: String
    let kind: Kind
    let description: String
}

/// Discovers capability metadata and activates only the tools the lead needs.
/// Activation state is published after all remote schema/instruction loading
/// succeeds, so cancellation cannot leave a half-installed tool surface.
actor DirectToolRegistry {
    private let runtime: any HarnessRuntime
    private let workspaceRoot: URL
    private var descriptors: [CapabilityDescriptor] = []
    private var activeTools: [String: ToolDefinition] = [:]
    private var skillNames: Set<String> = []
    private var skillToolMap: [String: String] = [:]
    private var mcpToolMap: [String: (server: String, original: String)] = [:]
    private var activeMCPServers: Set<String> = []

    init(runtime: any HarnessRuntime, workspaceRoot: URL) {
        self.runtime = runtime
        self.workspaceRoot = workspaceRoot.standardizedFileURL
        let base = [
            ToolDefinition(name: "find_capabilities", description: "Find capabilities without loading their full schemas.", inputSchema: [
                "type": "object", "properties": ["query": ["type": "string"]], "additionalProperties": false
            ]),
            ToolDefinition(name: "activate_capability", description: "Load one Skill or MCP capability on demand.", inputSchema: [
                "type": "object", "properties": ["capability": ["type": "string"]], "required": ["capability"], "additionalProperties": false
            ])
        ]
        activeTools = Dictionary(uniqueKeysWithValues: (base + WorkspaceTools.definitions).map { ($0.name, $0) })
    }

    func refresh() async {
        var next = [CapabilityDescriptor(id: "workspace", kind: .workspace, description: "Read and apply snapshot-safe edits in \(workspaceRoot.path)")]
        if let skills = try? await runtime.listSkills() {
            next += skills.map { CapabilityDescriptor(id: "skill:\($0.name)", kind: .skill, description: $0.description) }
        }
        if let servers = try? await runtime.listMCPServers() {
            next += servers.map { CapabilityDescriptor(id: "mcp:\($0.name)", kind: .mcp, description: "MCP \($0.transport.rawValue) server\($0.connected ? " (connected)" : "")") }
        }
        descriptors = next
    }

    func find(_ query: String?) async -> [CapabilityDescriptor] {
        if descriptors.isEmpty { await refresh() }
        let normalized = query?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
        guard !normalized.isEmpty else { return descriptors }
        return descriptors.filter { $0.id.lowercased().contains(normalized) || $0.description.lowercased().contains(normalized) }
    }

    func tools() -> [ToolDefinition] { Array(activeTools.values).sorted { $0.name < $1.name } }

    func activate(_ id: String) async throws -> String {
        if id == "workspace" { return "{\"activated\":\"workspace\",\"tools\":[\"read_file\",\"apply_file_edits\"]}" }
        if descriptors.isEmpty { await refresh() }
        guard let descriptor = descriptor(for: id) else { throw CallCoreBridgeError.protocolError("Unknown capability: \(id)") }
        switch descriptor.kind {
        case .skill:
            let name = String(id.dropFirst("skill:".count))
            if skillNames.contains(name) { return "{\"activated\":\"\(id)\",\"alreadyActive\":true}" }
            let skill = try await runtime.loadSkill(name)
            let toolName = Self.allocatedName(prefix: "skill", parts: [name, "read_file"], occupied: Set(activeTools.keys))
            let definition = ToolDefinition(name: toolName, description: "Read a supporting file from Skill \(name).", inputSchema: [
                "type": "object", "properties": ["path": ["type": "string"]], "required": ["path"], "additionalProperties": false
            ])
            activeTools[definition.name] = definition
            skillNames.insert(name)
            skillToolMap[definition.name] = name
            return try encode([String: JSONValue](uniqueKeysWithValues: [("activated", .string(id)), ("instructions", .string(skill.instructions)), ("tool", .string(definition.name))]))
        case .mcp:
            let server = String(id.dropFirst("mcp:".count))
            if activeMCPServers.contains(server) { return "{\"activated\":\"\(id)\",\"alreadyActive\":true}" }
            let mcpTools = try await runtime.listMCPTools(server: server)
            var additions: [(String, (server: String, original: String), ToolDefinition)] = []
            var occupied = Set(activeTools.keys)
            for (index, tool) in mcpTools.enumerated() {
                let safe = Self.allocatedName(prefix: "mcp", parts: [server, String(index), tool.name], occupied: occupied)
                occupied.insert(safe)
                additions.append((safe, (server, tool.name), ToolDefinition(name: safe, description: tool.description, inputSchema: tool.inputSchema)))
            }
            for (safe, mapping, definition) in additions {
                mcpToolMap[safe] = mapping
                activeTools[safe] = definition
            }
            activeMCPServers.insert(server)
            return try encode([String: JSONValue](uniqueKeysWithValues: [("activated", .string(id)), ("tools", .array(additions.map { .string($0.0) }))]))
        case .workspace:
            return "{\"activated\":\"workspace\"}"
        }
    }

    func execute(_ call: ToolCall) async throws -> String {
        let object = try decodeObject(call.arguments)
        switch call.name {
        case "find_capabilities":
            return try encode(await find(object["query"]?.stringValue).map { [String: JSONValue](uniqueKeysWithValues: [("id", .string($0.id)), ("kind", .string($0.kind.rawValue)), ("description", .string($0.description))]) })
        case "activate_capability":
            guard let id = object["capability"]?.stringValue else { throw CallCoreBridgeError.protocolError("activate_capability requires capability") }
            return try await activate(id)
        case "read_file":
            return try encode(try await runtime.readFile(path: object["path"]?.stringValue ?? "", startLine: object["startLine"]?.intValue, endLine: object["endLine"]?.intValue))
        case "apply_file_edits":
            let request = try JSONDecoder().decode(YeetApplyRequest.self, from: JSONEncoder().encode(call.arguments))
            return try encode(try await runtime.applyFileEdits(request))
        default:
            if let mapping = mcpToolMap[call.name] {
                return try encode(try await runtime.callMCPTool(server: mapping.server, name: mapping.original, arguments: object))
            }
            if let skill = skillToolMap[call.name] {
                return try await runtime.readSkillFile(skill, path: object["path"]?.stringValue ?? "")
            }
            throw CallCoreBridgeError.protocolError("Unknown direct tool: \(call.name)")
        }
    }

    private func descriptor(for id: String) -> CapabilityDescriptor? {
        descriptors.first { $0.id == id }
    }

    private func decodeObject(_ value: JSONValue) throws -> [String: JSONValue] {
        guard case .object(let object) = value else { throw CallCoreBridgeError.protocolError("tool arguments must be an object") }
        return object
    }

    private func encode<T: Encodable>(_ value: T) throws -> String { String(decoding: try JSONEncoder().encode(value), as: UTF8.self) }
    private static func allocatedName(prefix: String, parts: [String], occupied: Set<String>) -> String {
        let raw = parts.joined(separator: "_")
        let slug = String(raw.unicodeScalars.map { scalar in
            (scalar.value >= 48 && scalar.value <= 57) || (scalar.value >= 65 && scalar.value <= 90) || (scalar.value >= 97 && scalar.value <= 122) || scalar.value == 95 ? Character(String(scalar)) : Character("_")
        }.prefix(32))
        var hash: UInt32 = 2166136261
        for byte in raw.utf8 { hash = (hash ^ UInt32(byte)) &* 16777619 }
        let base = "\(prefix)_\(slug.isEmpty ? "capability" : slug)_\(String(hash, radix: 16))"
        var candidate = String(base.prefix(64))
        var suffix = 2
        while occupied.contains(candidate) { candidate = String("\(base)_\(suffix)".prefix(64)); suffix += 1 }
        return candidate
    }
}

private extension JSONValue {
    var stringValue: String? { if case .string(let value) = self { return value }; return nil }
    var intValue: Int? { switch self { case .integer(let value): return Int(value); case .number(let value): return Int(value); default: return nil } }
}
