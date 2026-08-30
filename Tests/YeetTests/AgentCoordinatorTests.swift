import Foundation
import Testing
@testable import HarnessCallCore
@testable import Yeet

private actor RecordingRuntime: HarnessRuntime {
    var requests: [CallRequest] = []
    var events: [[StreamEvent]]
    var listSkillsCalls = 0
    var listMCPToolsCalls = 0
    var readFileCalls = 0
    var mcpCalls: [String] = []
    var cancelOnRequestNumber: Int?

    init(events: [[StreamEvent]], cancelOnRequestNumber: Int? = nil) { self.events = events; self.cancelOnRequestNumber = cancelOnRequestNumber }
    func complete(_ request: CallRequest) async throws -> CallResult { requests.append(request); return .init(provider: "fake", model: request.model, text: "", finishReason: .stop) }
    func stream(_ request: CallRequest) async throws -> AsyncThrowingStream<StreamEvent, Error> {
        requests.append(request)
        if cancelOnRequestNumber == requests.count { throw CancellationError() }
        let next = events.isEmpty ? [.finish(finishReason: .stop, usage: nil, raw: nil)] : events.removeFirst()
        return AsyncThrowingStream { continuation in next.forEach { continuation.yield($0) }; continuation.finish() }
    }
    func listProviders() async throws -> [String] { ["fake"] }
    func listModels(for provider: String) async throws -> [String] { ["model"] }
    func listSkills() async throws -> [SkillSummary] { listSkillsCalls += 1; return [SkillSummary(name: "review", description: "Review", root: "/tmp", entrypoint: "SKILL.md")] }
    func loadSkill(_ name: String) async throws -> Skill { Skill(name: name, description: "Review", root: "/tmp", entrypoint: "SKILL.md", instructions: "Review it", files: []) }
    func readSkillFile(_ skill: String, path: String) async throws -> String { "support" }
    func listMCPServers() async throws -> [MCPServerStatus] { [] }
    func listMCPTools(server: String) async throws -> [MCPTool] { listMCPToolsCalls += 1; return [] }
    func callMCPTool(server: String, name: String, arguments: [String: JSONValue]) async throws -> MCPCallToolResult { mcpCalls.append(name); return .init(content: [], isError: false) }
    func readFile(path: String, startLine: Int?, endLine: Int?) async throws -> YeetReadResult { readFileCalls += 1; return try JSONDecoder().decode(YeetReadResult.self, from: Data("{\"path\":\"\(path)\",\"snapshot\":\"s\",\"startLine\":1,\"endLine\":1,\"totalLines\":1,\"content\":\"c\",\"numbered\":\"1 | c\"}".utf8)) }
    func applyFileEdits(_ request: YeetApplyRequest) async throws -> YeetApplyResult { .init(files: [], diagnostics: []) }
    func shutdown() async {}
}

private func drain(_ stream: AsyncThrowingStream<AgentEvent, Error>) async throws { for try await _ in stream {} }

@Test func directLeadSurfaceIncludesWorkspaceAndActivation() async throws {
    let runtime = RecordingRuntime(events: [])
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    _ = try await drain(await coordinator.stream(input: "inspect", model: "fake/model"))
    let tools = await runtime.requests[0].tools?.map(\.name) ?? []
    #expect(tools == ["activate_capability", "apply_file_edits", "find_capabilities", "read_file"])
}

@Test func directReadPairsToolCallAndPreservesHistory() async throws {
    let read = ToolCall(id: "read-1", name: "read_file", arguments: .object(["path": .string("README.md")]))
    let runtime = RecordingRuntime(events: [
        [.toolCall(index: 0, toolCall: read), .finish(finishReason: .toolCall, usage: nil, raw: nil)],
        [.textDelta("done"), .finish(finishReason: .stop, usage: nil, raw: nil)]
    ])
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    _ = try await drain(await coordinator.stream(input: "read", model: "fake/model"))
    #expect(await runtime.readFileCalls == 1)
    let history = await coordinator.modelHistory()
    #expect(history[3].toolCallId == "read-1")
}

@Test func capabilityDiscoveryIsLazyUntilActivation() async throws {
    let runtime = RecordingRuntime(events: [])
    let registry = DirectToolRegistry(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    _ = await registry.find(nil)
    #expect(await runtime.listSkillsCalls == 1)
    #expect(await runtime.listMCPToolsCalls == 0)
    #expect(await registry.tools().map(\.name).contains("read_file"))
}

@Test func cancellationBeforeDirectExecutionRollsBackHistory() async throws {
    let runtime = RecordingRuntime(events: [], cancelOnRequestNumber: 1)
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    do { try await drain(await coordinator.stream(input: "cancel", model: "fake/model")) } catch is CancellationError {}
    #expect(await coordinator.modelHistory().map(\.role) == [.system])
}

@Test func cancellationAfterDirectExecutionKeepsPairedHistory() async throws {
    let read = ToolCall(id: "read-1", name: "read_file", arguments: .object(["path": .string("README.md")]))
    let runtime = RecordingRuntime(events: [[.toolCall(index: 0, toolCall: read), .finish(finishReason: .toolCall, usage: nil, raw: nil)]], cancelOnRequestNumber: 2)
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    do { try await drain(await coordinator.stream(input: "read", model: "fake/model")) } catch is CancellationError {}
    let history = await coordinator.modelHistory()
    #expect(history.map(\.role) == [.system, .user, .assistant, .tool])
    #expect(history.last?.toolCallId == "read-1")
}

@Test func deltaOnlyLeadToolCallIsAssembled() async throws {
    let runtime = RecordingRuntime(events: [[.toolCallDelta(index: 0, id: "read-1", name: "read_file", argumentsDelta: "{\"path\":\"README.md\"}"), .finish(finishReason: .toolCall, usage: nil, raw: nil)], [.textDelta("ok"), .finish(finishReason: .stop, usage: nil, raw: nil)]])
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))
    _ = try await drain(await coordinator.stream(input: "read", model: "fake/model"))
    #expect(await runtime.readFileCalls == 1)
}

@Test func finishUsageIsForwardedByTheCoordinator() async throws {
    let usage = Usage(inputTokens: 3, outputTokens: 4, totalTokens: 7)
    let runtime = RecordingRuntime(events: [[.finish(finishReason: .stop, usage: usage, raw: nil)]])
    let coordinator = AgentCoordinator(runtime: runtime, workspaceRoot: URL(fileURLWithPath: "/tmp"))

    var received: Usage?
    for try await event in await coordinator.stream(input: "hello", model: "fake/model") {
        if case .finished(_, let usage) = event { received = usage }
    }

    #expect(received == usage)
}
