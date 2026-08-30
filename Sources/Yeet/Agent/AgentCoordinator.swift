import Foundation
import HarnessCallCore

/// Owns one model transcript and one bounded direct-tool loop.
actor AgentCoordinator {
    static let systemInstruction = "You're a helpful AI Coding Agent. Discover capabilities and execute selected tools directly in the workspace."

    private let runtime: any HarnessRuntime
    private let registry: DirectToolRegistry
    private var history: [Message] = [.system(systemInstruction)]
    private var turnInFlight = false
    private let maxToolRounds: Int

    init(runtime: any HarnessRuntime, workspaceRoot: URL, maxToolRounds: Int = 4) {
        self.runtime = runtime
        registry = DirectToolRegistry(runtime: runtime, workspaceRoot: workspaceRoot)
        self.maxToolRounds = max(1, maxToolRounds)
    }

    func stream(input: String, model: String) -> AsyncThrowingStream<AgentEvent, Error> {
        AsyncThrowingStream { continuation in
            let producer = Task {
                do { try await run(input: input, model: model, continuation: continuation); continuation.finish() }
                catch is CancellationError { continuation.finish(throwing: CancellationError()) }
                catch { continuation.finish(throwing: error) }
            }
            continuation.onTermination = { @Sendable _ in producer.cancel() }
        }
    }

    func modelHistory() -> [Message] { history }

    private func run(input: String, model: String, continuation: AsyncThrowingStream<AgentEvent, Error>.Continuation) async throws {
        guard !turnInFlight else { throw CallCoreBridgeError.protocolError("Another lead turn is already running") }
        turnInFlight = true
        defer { turnInFlight = false }
        let checkpoint = history.count
        var directExecutionStarted = false
        do {
            await registry.refresh()
            history.append(.user(input))
            for round in 0..<maxToolRounds {
                var text = ""
                var decoded: [Int: ToolCall] = [:]
                var partial: [Int: PartialToolCall] = [:]
                var finish: FinishReason = .stop
                let request = CallRequest(model: model, messages: history, tools: await registry.tools())
                for try await event in try await runtime.stream(request) {
                    switch event {
                    case .start(let provider, let model, let id): continuation.yield(.start(provider: provider, model: model, id: id))
                    case .textDelta(let delta): text += delta; continuation.yield(.textDelta(delta))
                    case .toolCallDelta(let index, let id, let name, let argumentsDelta):
                        var value = partial[index] ?? PartialToolCall(id: id ?? "tool-\(index)", name: name ?? "tool")
                        value.apply(id: id, name: name, argumentsDelta: argumentsDelta)
                        partial[index] = value
                        continuation.yield(.toolCallDelta(index: index, id: id, name: name, argumentsDelta: argumentsDelta))
                    case .toolCall(let index, let call): decoded[index] = call; continuation.yield(.toolCall(index: index, call: call))
                    case .finish(let reason, let usage, _):
                        finish = reason
                        continuation.yield(.finished(reason: reason, usage: usage))
                    }
                }
                let calls = ToolCallAssembly.collect(decoded: decoded, partial: partial)
                guard finish != .error else { if !text.isEmpty { history.append(.assistant(text)) }; return }
                history.append(.assistant(text, toolCalls: calls.isEmpty ? nil : calls))
                guard !calls.isEmpty else { return }
                guard round < maxToolRounds - 1 else {
                    for call in calls { history.append(.tool("{\"error\":\"tool-round limit reached\"}", toolCallId: call.id, name: call.name)) }
                    continuation.yield(.system("Stopped after \(maxToolRounds) tool rounds."))
                    return
                }
                for (index, call) in calls.enumerated() {
                    do {
                        directExecutionStarted = true
                        let result = try await execute(call)
                        history.append(.tool(result, toolCallId: call.id, name: call.name))
                    } catch is CancellationError {
                        history.append(.tool("{\"cancelled\":true}", toolCallId: call.id, name: call.name))
                        for remaining in calls.dropFirst(index + 1) { history.append(.tool("{\"cancelled\":true}", toolCallId: remaining.id, name: remaining.name)) }
                        throw CancellationError()
                    }
                }
            }
        } catch {
            if !directExecutionStarted { history.removeSubrange(checkpoint..<history.count) }
            throw error
        }
    }

    private func execute(_ call: ToolCall) async throws -> String {
        do { return try await registry.execute(call) }
        catch is CancellationError { throw CancellationError() }
        catch { return try encode([String: JSONValue](uniqueKeysWithValues: [("error", .string(String(describing: error)))]) ) }
    }

    private func encode<T: Encodable>(_ value: T) throws -> String { String(decoding: try JSONEncoder().encode(value), as: UTF8.self) }
}
