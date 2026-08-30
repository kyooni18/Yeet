import Foundation
import HarnessCallCore

/// The small runtime surface used by the interactive harness.
///
/// Keeping this boundary independent from `CallCoreClient` makes the agent
/// orchestration testable and gives the UI one persistent lifecycle owner.
protocol HarnessRuntime: Sendable {
    func complete(_ request: CallRequest) async throws -> CallResult
    func stream(_ request: CallRequest) async throws -> AsyncThrowingStream<StreamEvent, Error>
    func listProviders() async throws -> [String]
    func listModels(for provider: String) async throws -> [String]
    func listSkills() async throws -> [SkillSummary]
    func loadSkill(_ name: String) async throws -> Skill
    func readSkillFile(_ skill: String, path: String) async throws -> String
    func listMCPServers() async throws -> [MCPServerStatus]
    func listMCPTools(server: String) async throws -> [MCPTool]
    func callMCPTool(server: String, name: String, arguments: [String: JSONValue]) async throws -> MCPCallToolResult
    func readFile(path: String, startLine: Int?, endLine: Int?) async throws -> YeetReadResult
    func applyFileEdits(_ request: YeetApplyRequest) async throws -> YeetApplyResult
    func shutdown() async
}

/// A persistent, lazily-started bridge and edit daemon for one interactive
/// session. The actor owns recovery and guarantees that callers share one
/// process instead of spawning a bridge for every turn.
actor LiveHarnessRuntime: HarnessRuntime {
    private let workspaceRoot: URL
    private let bridgeEnvironment: [String: String]
    private let nodeExecutable: String?
    private var core: CallCoreClient?
    private var coreStartTask: Task<CallCoreClient, Error>?
    private var coreStartGeneration = 0
    private var editClient: YeetEditClient?

    init(
        workspaceRoot: URL,
        bridgeEnvironment: [String: String] = [:],
        nodeExecutable: String? = nil
    ) {
        self.workspaceRoot = workspaceRoot.standardizedFileURL
        self.bridgeEnvironment = bridgeEnvironment
        self.nodeExecutable = nodeExecutable
    }

    private func callCore() async throws -> CallCoreClient {
        if let core { return core }
        if let coreStartTask {
            let generation = coreStartGeneration
            let created = try await coreStartTask.value
            return try await publish(created, generation: generation)
        }

        let environment = bridgeEnvironment
        let node = nodeExecutable
        coreStartGeneration &+= 1
        let generation = coreStartGeneration
        let startTask = Task {
            try await CallCoreClient.startBundled(environment: environment, nodeExecutable: node)
        }
        coreStartTask = startTask
        do {
            let created = try await startTask.value
            return try await publish(created, generation: generation)
        } catch {
            if generation == coreStartGeneration { coreStartTask = nil }
            throw error
        }
    }

    private func publish(_ created: CallCoreClient, generation: Int) async throws -> CallCoreClient {
        guard generation == coreStartGeneration else {
            try? await created.shutdown()
            throw CancellationError()
        }
        if let core {
            if ObjectIdentifier(core) == ObjectIdentifier(created) { return core }
            try? await created.shutdown()
            throw CancellationError()
        }
        guard coreStartTask != nil else {
            try? await created.shutdown()
            throw CancellationError()
        }
        coreStartTask = nil
        core = created
        return created
    }

    private func editDaemon() throws -> YeetEditClient {
        if let editClient { return editClient }
        let created = try YeetEditClient.startBundled(
            root: workspaceRoot,
            environment: bridgeEnvironment,
            nodeExecutable: nodeExecutable
        )
        editClient = created
        return created
    }

    func complete(_ request: CallRequest) async throws -> CallResult {
        try await withCore { try await $0.complete(request) }
    }

    func stream(_ request: CallRequest) async throws -> AsyncThrowingStream<StreamEvent, Error> {
        let sourceCore = try await callCore()
        let source = sourceCore.stream(request)
        return AsyncThrowingStream { continuation in
            let producer = Task {
                do {
                    for try await event in source { continuation.yield(event) }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish(throwing: CancellationError())
                } catch {
                    if Self.isFatalBridgeError(error) { await self.invalidateCore(sourceCore) }
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in producer.cancel() }
        }
    }

    func listProviders() async throws -> [String] { try await withCore { try await $0.listProviders() } }

    func listModels(for provider: String) async throws -> [String] {
        try await withCore { try await $0.listModels(for: provider) }
    }

    func listSkills() async throws -> [SkillSummary] { try await withCore { try await $0.listSkills() } }

    func loadSkill(_ name: String) async throws -> Skill { try await withCore { try await $0.loadSkill(name) } }

    func readSkillFile(_ skill: String, path: String) async throws -> String {
        try await withCore { try await $0.readSkillFile(skill, path: path) }
    }

    func listMCPServers() async throws -> [MCPServerStatus] {
        try await withCore { try await $0.listMCPServers() }
    }

    func listMCPTools(server: String) async throws -> [MCPTool] {
        try await withCore { try await $0.listMCPTools(server: server) }
    }

    func callMCPTool(server: String, name: String, arguments: [String: JSONValue]) async throws -> MCPCallToolResult {
        try await withCore { try await $0.callMCPTool(server: server, name: name, arguments: arguments) }
    }

    func readFile(path: String, startLine: Int?, endLine: Int?) async throws -> YeetReadResult {
        do {
            return try await editDaemon().read(path: path, startLine: startLine, endLine: endLine)
        } catch {
            if case YeetEditClientError.processExited = error { editClient = nil }
            throw error
        }
    }

    func applyFileEdits(_ request: YeetApplyRequest) async throws -> YeetApplyResult {
        do {
            return try await editDaemon().apply(request)
        } catch {
            if case YeetEditClientError.processExited = error { editClient = nil }
            throw error
        }
    }

    func shutdown() async {
        coreStartGeneration &+= 1
        let pendingStart = coreStartTask
        coreStartTask = nil
        let currentCore = core
        core = nil
        pendingStart?.cancel()
        if let currentCore { try? await currentCore.shutdown() }
        editClient = nil
    }

    private func invalidateCore(_ expected: CallCoreClient?) async {
        guard let currentCore = core else { return }
        if let expected, ObjectIdentifier(currentCore) != ObjectIdentifier(expected) { return }
        coreStartGeneration &+= 1
        self.core = nil
        try? await currentCore.shutdown()
    }

    private func withCore<T>(_ operation: (CallCoreClient) async throws -> T) async throws -> T {
        let client = try await callCore()
        do {
            return try await operation(client)
        } catch {
            if Self.isFatalBridgeError(error) { await invalidateCore(client) }
            throw error
        }
    }

    private nonisolated static func isFatalBridgeError(_ error: Error) -> Bool {
        guard let bridgeError = error as? CallCoreBridgeError else { return false }
        switch bridgeError {
        case .failedToStart, .bridgeExited, .protocolError, .transport: return true
        case .remote: return false
        }
    }
}
