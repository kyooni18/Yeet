import Foundation

public struct BridgeConfiguration: Sendable, Equatable {
    public var command: String
    public var arguments: [String]
    public var environment: [String: String]
    public var inheritEnvironment: Bool
    public var workingDirectory: URL?

    public init(
        command: String = "harness-call-core-bridge",
        arguments: [String] = [],
        environment: [String: String] = [:],
        inheritEnvironment: Bool = true,
        workingDirectory: URL? = nil
    ) {
        self.command = command
        self.arguments = arguments
        self.environment = environment
        self.inheritEnvironment = inheritEnvironment
        self.workingDirectory = workingDirectory
    }

    public static func node(script: URL, environment: [String: String] = [:], workingDirectory: URL? = nil) -> Self {
        .init(command: "node", arguments: [script.path], environment: environment, workingDirectory: workingDirectory)
    }
}

public actor CallCoreClient {
    private enum Pending {
        case result(CheckedContinuation<CallResult, Error>)
        case providers(CheckedContinuation<[String], Error>)
        case models(CheckedContinuation<[String], Error>)
        case registered(CheckedContinuation<String, Error>)
        case boolean(CheckedContinuation<Bool, Error>)
        case authStatus(CheckedContinuation<AuthStatus, Error>)
        case configPath(CheckedContinuation<String, Error>)
        case skills(CheckedContinuation<[SkillSummary], Error>)
        case skill(CheckedContinuation<Skill, Error>)
        case string(CheckedContinuation<String, Error>)
        case mcpServers(CheckedContinuation<[MCPServerStatus], Error>)
        case mcpServer(CheckedContinuation<MCPServerConfiguration, Error>)
        case mcpTools(CheckedContinuation<[MCPTool], Error>)
        case mcpToolResult(CheckedContinuation<MCPCallToolResult, Error>)
        case mcpResources(CheckedContinuation<[MCPResource], Error>)
        case mcpResourceResult(CheckedContinuation<MCPReadResourceResult, Error>)
        case mcpPrompts(CheckedContinuation<[MCPPrompt], Error>)
        case mcpPromptResult(CheckedContinuation<MCPGetPromptResult, Error>)
        case void(CheckedContinuation<Void, Error>)

        func fail(_ error: Error) {
            switch self {
            case .result(let c): c.resume(throwing: error)
            case .providers(let c): c.resume(throwing: error)
            case .models(let c): c.resume(throwing: error)
            case .registered(let c): c.resume(throwing: error)
            case .boolean(let c): c.resume(throwing: error)
            case .authStatus(let c): c.resume(throwing: error)
            case .configPath(let c): c.resume(throwing: error)
            case .skills(let c): c.resume(throwing: error)
            case .skill(let c): c.resume(throwing: error)
            case .string(let c): c.resume(throwing: error)
            case .mcpServers(let c): c.resume(throwing: error)
            case .mcpServer(let c): c.resume(throwing: error)
            case .mcpTools(let c): c.resume(throwing: error)
            case .mcpToolResult(let c): c.resume(throwing: error)
            case .mcpResources(let c): c.resume(throwing: error)
            case .mcpResourceResult(let c): c.resume(throwing: error)
            case .mcpPrompts(let c): c.resume(throwing: error)
            case .mcpPromptResult(let c): c.resume(throwing: error)
            case .void(let c): c.resume(throwing: error)
            }
        }
    }

    private let transport: ProcessTransport
    private let encoder = JSONEncoder()
    private let decoder = JSONDecoder()
    private var pending: [String: Pending] = [:]
    private var streams: [String: AsyncThrowingStream<StreamEvent, Error>.Continuation] = [:]
    private var stdoutBuffer = Data()
    private var stderrTail = Data()
    private var isShuttingDown = false
    private var exited = false

    private init(configuration: BridgeConfiguration) {
        self.transport = ProcessTransport(configuration: configuration)
    }

    deinit { transport.stop() }

    public static func start(configuration: BridgeConfiguration = .init()) async throws -> CallCoreClient {
        let client = CallCoreClient(configuration: configuration)
        try await client.startTransport()
        try await client.ping()
        return client
    }

    private func startTransport() throws {
        try transport.start(
            onStdout: { [weak self] data in
                guard let self else { return }
                Task { await self.consumeStdout(data) }
            },
            onStderr: { [weak self] data in
                guard let self else { return }
                Task { await self.consumeStderr(data) }
            },
            onExit: { [weak self] status in
                guard let self else { return }
                Task { await self.bridgeExited(status: status) }
            }
        )
    }

    public func complete(_ request: CallRequest) async throws -> CallResult {
        let id = UUID().uuidString
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                pending[id] = .result(continuation)
                do { try transport.send(encodeRequestCommand(id: id, op: "complete", request: request, encoder: encoder)) }
                catch { pending.removeValue(forKey: id)?.fail(error) }
            }
        } onCancel: {
            Task { await self.cancelOperation(id) }
        }
    }

    public nonisolated func stream(_ request: CallRequest) -> AsyncThrowingStream<StreamEvent, Error> {
        let id = UUID().uuidString
        return AsyncThrowingStream { continuation in
            continuation.onTermination = { [weak self] _ in
                guard let self else { return }
                Task { await self.cancelOperation(id) }
            }
            Task { await self.beginStream(id: id, request: request, continuation: continuation) }
        }
    }

    public func configDirectory() async throws -> URL {
        let id = UUID().uuidString
        let path: String = try await withCheckedThrowingContinuation { continuation in
            pending[id] = .configPath(continuation)
            do { try transport.send(encodeBaseCommand(id: id, op: "config-path", encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
        return URL(fileURLWithPath: path, isDirectory: true)
    }

    public func authStatus(for provider: String) async throws -> AuthStatus {
        try await authOperation(provider: provider, op: "auth-status")
    }

    @discardableResult
    public func setAPIKey(_ apiKey: String, for provider: String) async throws -> AuthStatus {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .authStatus(continuation)
            do { try transport.send(encodeSetAPIKeyCommand(id: id, provider: provider, apiKey: apiKey, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    @discardableResult
    public func loginInBrowser(_ provider: String, options: BrowserLoginOptions? = nil) async throws -> AuthStatus {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .authStatus(continuation)
            do { try transport.send(encodeBrowserLoginCommand(id: id, provider: provider, options: options, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    @discardableResult
    public func logout(_ provider: String) async throws -> AuthStatus {
        try await authOperation(provider: provider, op: "auth-logout")
    }

    private func authOperation(provider: String, op: String) async throws -> AuthStatus {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .authStatus(continuation)
            do { try transport.send(encodeProviderCommand(id: id, op: op, provider: provider, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func listProviders() async throws -> [String] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .providers(continuation)
            do { try transport.send(encodeBaseCommand(id: id, op: "list-providers", encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    /// Fetch provider-local model identifiers from a configured provider.
    ///
    /// The provider must be built in or registered on this client. Returned
    /// names are suitable for the model portion of a `provider/model` id.
    public func listModels(for provider: String) async throws -> [String] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .models(continuation)
            do { try transport.send(encodeProviderCommand(id: id, op: "list-models", provider: provider, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func fetchAvailableModels(for provider: String) async throws -> [String] {
        try await listModels(for: provider)
    }

    @discardableResult
    public func registerOpenAICompatible(_ provider: OpenAICompatibleProviderConfiguration) async throws -> String {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .registered(continuation)
            do { try transport.send(encodeRegisterCommand(id: id, provider: provider, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    @discardableResult
    public func unregisterProvider(_ provider: String) async throws -> Bool {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .boolean(continuation)
            do { try transport.send(encodeUnregisterCommand(id: id, provider: provider, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func listSkills() async throws -> [SkillSummary] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .skills(continuation)
            do { try transport.send(encodeBaseCommand(id: id, op: "skill-list", encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func loadSkill(_ name: String) async throws -> Skill {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .skill(continuation)
            do { try transport.send(encodeSkillCommand(id: id, op: "skill-load", skill: name, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func readSkillFile(_ skill: String, path: String) async throws -> String {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .string(continuation)
            do { try transport.send(encodeSkillCommand(id: id, op: "skill-read", skill: skill, path: path, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func listMCPServers() async throws -> [MCPServerStatus] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpServers(continuation)
            do { try transport.send(encodeBaseCommand(id: id, op: "mcp-list-servers", encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    @discardableResult
    public func setMCPServer(_ server: MCPServerConfiguration) async throws -> MCPServerConfiguration {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpServer(continuation)
            do { try transport.send(encodeMCPSetServerCommand(id: id, server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    @discardableResult
    public func removeMCPServer(_ server: String) async throws -> Bool {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .boolean(continuation)
            do { try transport.send(encodeMCPServerCommand(id: id, op: "mcp-remove-server", server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func listMCPTools(server: String? = nil) async throws -> [MCPTool] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpTools(continuation)
            do { try transport.send(encodeMCPServerCommand(id: id, op: "mcp-list-tools", server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func callMCPTool(
        server: String,
        name: String,
        arguments: [String: JSONValue] = [:]
    ) async throws -> MCPCallToolResult {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpToolResult(continuation)
            do { try transport.send(encodeMCPCallToolCommand(id: id, server: server, tool: name, arguments: arguments, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func callMCPTool(_ qualifiedName: String, arguments: [String: JSONValue] = [:]) async throws -> MCPCallToolResult {
        guard let slash = qualifiedName.firstIndex(of: "/"), slash != qualifiedName.startIndex else {
            throw CallCoreBridgeError.protocolError("MCP tool must use server/tool form")
        }
        let server = String(qualifiedName[..<slash])
        let name = String(qualifiedName[qualifiedName.index(after: slash)...])
        guard !name.isEmpty else { throw CallCoreBridgeError.protocolError("MCP tool must use server/tool form") }
        return try await callMCPTool(server: server, name: name, arguments: arguments)
    }

    public func listMCPResources(server: String? = nil) async throws -> [MCPResource] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpResources(continuation)
            do { try transport.send(encodeMCPServerCommand(id: id, op: "mcp-list-resources", server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func readMCPResource(server: String, uri: String) async throws -> MCPReadResourceResult {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpResourceResult(continuation)
            do { try transport.send(encodeMCPReadResourceCommand(id: id, server: server, uri: uri, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func listMCPPrompts(server: String? = nil) async throws -> [MCPPrompt] {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpPrompts(continuation)
            do { try transport.send(encodeMCPServerCommand(id: id, op: "mcp-list-prompts", server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func getMCPPrompt(
        server: String,
        name: String,
        arguments: [String: String] = [:]
    ) async throws -> MCPGetPromptResult {
        let id = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = .mcpPromptResult(continuation)
            do { try transport.send(encodeMCPGetPromptCommand(id: id, server: server, prompt: name, arguments: arguments, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func disconnectMCPServer(_ server: String? = nil) async throws {
        let id = UUID().uuidString
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            pending[id] = .void(continuation)
            do { try transport.send(encodeMCPServerCommand(id: id, op: "mcp-disconnect", server: server, encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    public func shutdown() async throws {
        guard !isShuttingDown, !exited else { return }
        isShuttingDown = true
        let id = UUID().uuidString
        do {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                pending[id] = .void(continuation)
                do { try transport.send(encodeBaseCommand(id: id, op: "shutdown", encoder: encoder)) }
                catch { pending.removeValue(forKey: id)?.fail(error) }
            }
        } catch {
            transport.stop()
            throw error
        }
    }

    private func ping() async throws {
        let id = UUID().uuidString
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            pending[id] = .void(continuation)
            do { try transport.send(encodeBaseCommand(id: id, op: "ping", encoder: encoder)) }
            catch { pending.removeValue(forKey: id)?.fail(error) }
        }
    }

    private func beginStream(
        id: String,
        request: CallRequest,
        continuation: AsyncThrowingStream<StreamEvent, Error>.Continuation
    ) {
        guard !exited else {
            continuation.finish(throwing: CallCoreBridgeError.bridgeExited(status: -1, stderr: stderrString))
            return
        }
        streams[id] = continuation
        do { try transport.send(encodeRequestCommand(id: id, op: "stream", request: request, encoder: encoder)) }
        catch { streams.removeValue(forKey: id)?.finish(throwing: error) }
    }

    private func cancelOperation(_ id: String) {
        var hadOperation = false
        if let operation = pending.removeValue(forKey: id) {
            hadOperation = true
            operation.fail(CancellationError())
        }
        if let stream = streams.removeValue(forKey: id) {
            hadOperation = true
            stream.finish(throwing: CancellationError())
        }
        guard hadOperation, !exited else { return }
        let cancelId = UUID().uuidString
        try? transport.send(encodeCancelCommand(id: cancelId, target: id, encoder: encoder))
    }

    private func consumeStdout(_ data: Data) {
        stdoutBuffer.append(data)
        while let newline = stdoutBuffer.firstIndex(of: 0x0A) {
            let line = Data(stdoutBuffer[..<newline])
            stdoutBuffer.removeSubrange(stdoutBuffer.startIndex...newline)
            guard !line.isEmpty else { continue }
            do {
                let envelope = try decoder.decode(IncomingEnvelope.self, from: line)
                try receive(envelope)
            } catch {
                failAll(CallCoreBridgeError.protocolError(error.localizedDescription))
            }
        }
    }

    private func consumeStderr(_ data: Data) {
        stderrTail.append(data)
        let maxBytes = 16 * 1024
        if stderrTail.count > maxBytes { stderrTail.removeFirst(stderrTail.count - maxBytes) }
    }

    private func receive(_ envelope: IncomingEnvelope) throws {
        guard envelope.v == harnessCallCoreBridgeProtocolVersion else {
            throw CallCoreBridgeError.protocolError("Unsupported protocol version \(envelope.v)")
        }

        switch envelope.type {
        case "pong", "done":
            if let stream = streams.removeValue(forKey: envelope.id) {
                stream.finish()
                return
            }
            if case .void(let continuation)? = pending.removeValue(forKey: envelope.id) { continuation.resume() }

        case "providers":
            guard case .providers(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.providers ?? [])
        case "models":
            guard case .models(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.models ?? [])
        case "config-path":
            guard case .configPath(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let path = envelope.path else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("config-path missing path")) }
            continuation.resume(returning: path)
        case "auth-status":
            guard case .authStatus(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let status = envelope.status else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("auth-status missing status")) }
            continuation.resume(returning: status)
        case "registered":
            guard case .registered(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let provider = envelope.provider else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("registered missing provider")) }
            continuation.resume(returning: provider)
        case "unregistered", "mcp-removed":
            guard case .boolean(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.removed ?? false)
        case "result":
            guard case .result(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let result = envelope.result else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("result missing payload")) }
            continuation.resume(returning: result)
        case "event":
            guard let event = envelope.event else { throw CallCoreBridgeError.protocolError("event missing payload") }
            streams[envelope.id]?.yield(event)

        case "skills":
            guard case .skills(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.skills ?? [])
        case "skill":
            guard case .skill(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let skill = envelope.skill else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("skill missing payload")) }
            continuation.resume(returning: skill)
        case "skill-file":
            guard case .string(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.content ?? "")

        case "mcp-servers":
            guard case .mcpServers(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.servers ?? [])
        case "mcp-server":
            guard case .mcpServer(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let server = envelope.server else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("mcp-server missing payload")) }
            continuation.resume(returning: server)
        case "mcp-tools":
            guard case .mcpTools(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.tools ?? [])
        case "mcp-tool-result":
            guard case .mcpToolResult(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let result = envelope.toolResult else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("mcp-tool-result missing payload")) }
            continuation.resume(returning: result)
        case "mcp-resources":
            guard case .mcpResources(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.resources ?? [])
        case "mcp-resource-result":
            guard case .mcpResourceResult(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let result = envelope.resourceResult else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("mcp-resource-result missing payload")) }
            continuation.resume(returning: result)
        case "mcp-prompts":
            guard case .mcpPrompts(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            continuation.resume(returning: envelope.prompts ?? [])
        case "mcp-prompt-result":
            guard case .mcpPromptResult(let continuation)? = pending.removeValue(forKey: envelope.id) else { return }
            guard let result = envelope.promptResult else { return continuation.resume(throwing: CallCoreBridgeError.protocolError("mcp-prompt-result missing payload")) }
            continuation.resume(returning: result)

        case "error":
            let error: Error = envelope.error.map { CallCoreBridgeError.remote($0) }
                ?? CallCoreBridgeError.protocolError("error message missing payload")
            pending.removeValue(forKey: envelope.id)?.fail(error)
            streams.removeValue(forKey: envelope.id)?.finish(throwing: error)
        case "cancelled":
            return
        default:
            throw CallCoreBridgeError.protocolError("Unknown message type \(envelope.type)")
        }
    }

    private func bridgeExited(status: Int32) {
        exited = true
        let error = CallCoreBridgeError.bridgeExited(status: status, stderr: stderrString)
        if !isShuttingDown || status != 0 { failAll(error) }
    }

    private var stderrString: String { String(data: stderrTail, encoding: .utf8) ?? "" }

    private func failAll(_ error: Error) {
        let operations = pending.values
        pending.removeAll()
        for operation in operations { operation.fail(error) }
        let activeStreams = streams.values
        streams.removeAll()
        for stream in activeStreams { stream.finish(throwing: error) }
    }
}
