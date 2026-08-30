import Foundation
import HarnessCallCore
import Observation

/// Presentation/session state for the interactive harness.
///
/// Model history, bridge lifetime, capability discovery, and direct tool execution
/// live behind `AgentCoordinator` and `HarnessRuntime`. This type only maps
/// domain events to observable UI state and owns slash commands.
@MainActor
@Observable
final class Session {
    var inputText = ""
    var isStreaming = false
    var streamText = ""
    var errorMessage: String?
    var debugLog: [String] = []
    /// Token accounting for all model responses in this interactive session.
    /// Providers may omit one or more fields, so the values are accumulated
    /// independently and the UI derives a best-effort total when needed.
    var tokenUsage = Usage(inputTokens: 0, outputTokens: 0, totalTokens: 0, cachedInputTokens: 0)
    /// Number of completed model responses. Each response consumes one model
    /// credit; tool rounds are counted separately as they are separate calls.
    var creditUsage = 0
    var conversation: [ConversationEntry] = []
    var conversationRevision = 0

    var isModelPickerPresented = false
    var isLoadingModels = false
    var availableModels: [String] = []
    var modelSearchText = "" {
        didSet {
            guard modelSearchText != oldValue else { return }
            reconcileModelSelection()
        }
    }
    var selectedModel = ""
    var activeModel: String

    private var commandRegistry: CommandRegistry
    private var selectedCommandIndex = 0
    private var activeAssistantEntryID: UUID?
    private var pendingToolCalls: [Int: ConversationToolCall] = [:]
    private let runtime: any HarnessRuntime
    private let coordinator: AgentCoordinator
    let workspaceRoot: URL

    init(
        commands: [Command] = CommandRegistry.defaultCommands,
        workspaceRoot: URL = URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true),
        runtime: (any HarnessRuntime)? = nil
    ) {
        commandRegistry = CommandRegistry(commands: commands)
        self.workspaceRoot = workspaceRoot.standardizedFileURL
        self.runtime = runtime ?? LiveHarnessRuntime(workspaceRoot: self.workspaceRoot)
        coordinator = AgentCoordinator(runtime: self.runtime, workspaceRoot: self.workspaceRoot)
        activeModel = (try? ConfigStore().model()) ?? ""
    }

    deinit {
        let runtime = runtime
        Task { await runtime.shutdown() }
    }

    var commandSuggestions: [Command] { commandRegistry.suggestions(for: inputText) }
    var commandHelpText: String { commandRegistry.commands.map { "\($0.name) — \($0.description)" }.joined(separator: "\n") }
    var filteredModels: [String] {
        ModelPickerLayout.filteredModels(availableModels, query: modelSearchText)
    }
    var modelPickerWindow: Range<Int> {
        ModelPickerLayout.window(for: filteredModels, selectedModel: selectedModel)
    }
    var modelPickerModels: [String] { Array(filteredModels[modelPickerWindow]) }
    var modelPickerWindowDescription: String {
        ModelPickerLayout.windowDescription(for: modelPickerWindow, total: filteredModels.count)
    }

    func moveModelSelection(by offset: Int) {
        selectedModel = ModelPickerLayout.movedSelection(
            current: selectedModel,
            models: filteredModels,
            offset: offset
        )
    }

    func presentModelPicker(search: String = "") {
        isModelPickerPresented = true
        errorMessage = nil
        selectedModel = (try? ConfigStore().model()) ?? ""
        modelSearchText = search.trimmingCharacters(in: .whitespacesAndNewlines)
        reconcileModelSelection()
        if availableModels.isEmpty && !isLoadingModels { loadAvailableModels() }
    }

    func dismissModelPicker() {
        isModelPickerPresented = false
        modelSearchText = ""
    }

    func selectCurrentModel() {
        let model = selectedModel.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !model.isEmpty else { errorMessage = "Select a model before applying."; return }
        do {
            try ConfigStore().setModel(model)
            activeModel = model
            showCommandResponse("Model set to \(model)")
            dismissModelPicker()
        } catch {
            errorMessage = String(describing: error)
            appendLog("model-picker: save failed: \(String(reflecting: error))")
        }
    }

    func reconcileModelSelection() {
        selectedModel = ModelPickerLayout.reconciledSelection(
            current: selectedModel,
            models: filteredModels
        )
    }

    private func loadAvailableModels() {
        isLoadingModels = true
        errorMessage = nil
        appendLog("model-picker: loading available models")
        let runtime = self.runtime
        Task { @MainActor in
            do {
                let providers = try await runtime.listProviders()
                var models: [String] = []
                for provider in providers.sorted() {
                    do { models += try await runtime.listModels(for: provider).map { "\(provider)/\($0)" } }
                    catch { appendLog("model-picker: provider \(provider) failed: \(String(describing: error))") }
                }
                availableModels = Array(Set(models)).sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
                reconcileModelSelection()
                appendLog("model-picker: loaded \(availableModels.count) models")
                if availableModels.isEmpty { errorMessage = "No available models could be loaded. Check provider credentials." }
            } catch {
                errorMessage = "Unable to load models: \(error)"
                appendLog("model-picker: load failed: \(String(reflecting: error))")
            }
            isLoadingModels = false
        }
    }

    func registerCommand(_ command: Command) { commandRegistry.register(command) }
    func showCommandResponse(_ text: String) { streamText = text; if !text.isEmpty { appendConversation(.system(text)) } }
    func clearCommandResponse() {
        let previous = streamText
        streamText = ""
        guard !previous.isEmpty, let index = conversation.lastIndex(where: {
            if case .system(let content) = $0.kind { return content == previous }; return false
        }) else { return }
        conversation.remove(at: index)
        conversationRevision &+= 1
    }

    var selectedSuggestionIndex: Int { commandSuggestions.isEmpty ? 0 : min(selectedCommandIndex, commandSuggestions.count - 1) }
    func selectCommandSuggestion() {
        guard !commandSuggestions.isEmpty else { return }
        inputText = commandSuggestions[selectedSuggestionIndex].name + " "
        selectedCommandIndex = 0
    }

    func submit() {
        appendLog("submit: invoked (streaming=\(isStreaming), inputLength=\(inputText.count))")
        guard !isStreaming else { return }
        let input = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !input.isEmpty else { return }
        inputText = ""
        selectedCommandIndex = 0
        streamText = ""
        errorMessage = nil
        if input.first == "/" { executeCommand(input); return }

        appendConversation(.user(input))
        activeAssistantEntryID = appendConversation(.assistant(content: "", toolCalls: []))
        pendingToolCalls.removeAll()
        isStreaming = true
        let model = activeModel.trimmingCharacters(in: .whitespacesAndNewlines)
        Task { @MainActor [weak self] in
            guard let self else { return }
            await stream(input: input, model: model)
        }
    }

    private func executeCommand(_ input: String) {
        let invocation = CommandInvocation(input: input)
        guard let command = commandRegistry.command(named: invocation.name) else {
            errorMessage = "Unknown command: \(invocation.name). Type / for suggestions."
            return
        }
        do {
            try command.run(arguments: invocation.arguments, in: self)
        } catch {
            errorMessage = String(describing: error)
        }
    }

    private func stream(input: String, model: String) async {
        defer { isStreaming = false; activeAssistantEntryID = nil; pendingToolCalls.removeAll() }
        do {
            guard !model.isEmpty else { throw ConfigError.missingModel }
            for try await event in await coordinator.stream(input: input, model: model) {
                switch event {
                case .start(let provider, let model, let id): appendLog("event: start provider=\(provider) model=\(model) id=\(id ?? "none")")
                case .textDelta(let delta): streamText += delta; appendAssistantText(delta)
                case .toolCallDelta(let index, let id, let name, let argumentsDelta): updateToolCall(index: index, id: id, name: name, argumentsDelta: argumentsDelta)
                case .toolCall(let index, let call):
                    let displayed = ConversationToolCall(index: index, toolCall: call)
                    pendingToolCalls[index] = displayed
                    upsertAssistantToolCall(displayed)
                case .system(let text): appendConversation(.system(text))
                case .finished(let reason, let usage):
                    recordUsage(usage)
                    appendLog("event: finish reason=\(reason) usage=\(usageDescription(usage))")
                }
            }
        } catch {
            let description = String(describing: error)
            errorMessage = description
            failActiveAssistant(with: description)
            appendLog("stream: error: \(description)")
        }
    }

    @discardableResult func appendToolCall(_ call: ConversationToolCall) -> UUID { appendConversation(.toolCall(call)) }
    @discardableResult private func appendConversation(_ kind: ConversationEntryKind) -> UUID {
        let entry = ConversationEntry(kind: kind)
        conversation.append(entry)
        conversationRevision &+= 1
        return entry.id
    }

    private func appendAssistantText(_ delta: String) {
        guard !delta.isEmpty, let id = activeAssistantEntryID, let index = conversation.firstIndex(where: { $0.id == id }), case .assistant(let content, let calls) = conversation[index].kind else { return }
        conversation[index].kind = .assistant(content: content + delta, toolCalls: calls)
        conversationRevision &+= 1
    }

    private func failActiveAssistant(with description: String) {
        guard let id = activeAssistantEntryID, let index = conversation.firstIndex(where: { $0.id == id }), case .assistant(let content, let calls) = conversation[index].kind, content.isEmpty else { return }
        conversation[index].kind = .assistant(content: "**Error:** \(description)", toolCalls: calls)
        conversationRevision &+= 1
    }

    private func updateToolCall(index: Int, id: String?, name: String?, argumentsDelta: String?) {
        var call = pendingToolCalls[index] ?? ConversationToolCall(index: index, callID: id, name: name ?? "tool")
        if let id, !id.isEmpty { call.callID = id }
        if let name, !name.isEmpty { call.name = name }
        if let argumentsDelta { call.arguments += argumentsDelta }
        call.status = .streaming
        pendingToolCalls[index] = call
        upsertAssistantToolCall(call)
    }

    private func upsertAssistantToolCall(_ call: ConversationToolCall) {
        guard let id = activeAssistantEntryID, let index = conversation.firstIndex(where: { $0.id == id }), case .assistant(let content, var calls) = conversation[index].kind else { return }
        if let existing = calls.firstIndex(where: { $0.index == call.index || ($0.callID != nil && $0.callID == call.callID) }) { calls[existing] = call } else { calls.append(call) }
        conversation[index].kind = .assistant(content: content, toolCalls: calls)
        conversationRevision &+= 1
    }

    private func appendLog(_ message: String) {
        debugLog.append(message)
        if debugLog.count > 24 { debugLog.removeFirst(debugLog.count - 24) }
    }

    private func recordUsage(_ usage: Usage?) {
        creditUsage += 1
        if let usage {
            tokenUsage.accumulate(usage)
        }
    }

    private func usageDescription(_ usage: Usage?) -> String { usage?.summary ?? "none" }
}
