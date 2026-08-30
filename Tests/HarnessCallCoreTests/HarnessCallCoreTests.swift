import Foundation
import Testing
@testable import HarnessCallCore

private func makeClient() async throws -> CallCoreClient {
    let fixture = try #require(Bundle.module.url(forResource: "fake-bridge", withExtension: "mjs", subdirectory: "Fixtures"))
    return try await CallCoreClient.start(configuration: .node(script: fixture))
}

@Test func completeRoundTrip() async throws {
    let client = try await makeClient()
    let result = try await client.complete(.init(model: "openai/test-model", messages: [.user("hi")]))
    #expect(result.text == "hello from bridge")
    #expect(result.usage?.totalTokens == 7)
    #expect(result.model == "openai/test-model")
    #expect(result.raw == ["ok": true])
    try await client.shutdown()
}

@Test func streamRoundTrip() async throws {
    let client = try await makeClient()
    var text = ""
    for try await event in client.stream(.init(model: "anthropic/test-model", messages: [.user("hi")])) {
        if case .textDelta(let delta) = event { text += delta }
    }
    #expect(text == "hello stream")
    try await client.shutdown()
}

@Test func providerRegistrationRoundTrip() async throws {
    let client = try await makeClient()
    let registered = try await client.registerOpenAICompatible(.init(id: "local", baseUrl: "http://127.0.0.1:8080/v1"))
    #expect(registered == "local")
    #expect(try await client.listProviders().contains("local"))
    #expect(try await client.listModels(for: "local") == ["local-test-model"])
    #expect(try await client.fetchAvailableModels(for: "local") == ["local-test-model"])
    #expect(try await client.unregisterProvider("local"))
    try await client.shutdown()
}

@Test func toolChoiceAndJSONValueMatchTypeScriptShape() throws {
    let encoder = JSONEncoder()
    let decoder = JSONDecoder()
    let choice = ToolChoice.named("read_file")
    let data = try encoder.encode(choice)
    #expect(String(decoding: data, as: UTF8.self) == #"{"name":"read_file"}"#)
    #expect(try decoder.decode(ToolChoice.self, from: data) == choice)

    let value: JSONValue = ["type": "object", "required": ["path"], "strict": true]
    #expect(try decoder.decode(JSONValue.self, from: encoder.encode(value)) == value)
}

@Test func editToolPayloadSupportsFileOperations() throws {
    let request = YeetApplyRequest(changes: [
        YeetFileChange(
            path: "new.txt",
            fileOp: .create(text: "hello\n", mode: 0o644)
        )
    ])
    let data = try JSONEncoder().encode(request)
    let decoded = try JSONDecoder().decode(YeetApplyRequest.self, from: data)
    #expect(decoded.changes.first?.snapshot == nil)
    #expect(decoded.changes.first?.edits.isEmpty == true)
    if case .create(let text, let mode)? = decoded.changes.first?.fileOp {
        #expect(text == "hello\n")
        #expect(mode == 0o644)
    } else {
        Issue.record("create file operation did not round-trip")
    }
}

@Test func authRoundTrip() async throws {
    let client = try await makeClient()
    let config = try await client.configDirectory()
    #expect(config.path == "/tmp/fake-yeet")

    let empty = try await client.authStatus(for: "openai")
    #expect(empty.authenticated == false)
    #expect(empty.method == .none)

    let key = try await client.setAPIKey("sk-test", for: "openai")
    #expect(key.authenticated)
    #expect(key.method == .apiKey)

    let browser = try await client.loginInBrowser("openrouter")
    #expect(browser.authenticated)
    #expect(browser.method == .browser)

    let loggedOut = try await client.logout("openai")
    #expect(loggedOut.authenticated == false)
    try await client.shutdown()
}


@Test func skillsRoundTrip() async throws {
    let client = try await makeClient()
    let skills = try await client.listSkills()
    #expect(skills.map(\.name) == ["review-code"])
    let skill = try await client.loadSkill("review-code")
    #expect(skill.instructions == "Inspect the diff.")
    #expect(skill.files.contains("references/checklist.md"))
    let reference = try await client.readSkillFile("review-code", path: "references/checklist.md")
    #expect(reference.contains("Check tests"))
    try await client.shutdown()
}

@Test func mcpRoundTrip() async throws {
    let client = try await makeClient()
    let server = try await client.setMCPServer(.http(name: "demo", url: "https://example.test/mcp"))
    #expect(server.name == "demo")
    #expect(try await client.listMCPServers().count == 1)

    let tools = try await client.listMCPTools(server: "demo")
    #expect(tools.first?.qualifiedName == "demo/search")
    let toolResult = try await client.callMCPTool("demo/search", arguments: ["q": "hello"])
    #expect(toolResult.isError == false)

    let resources = try await client.listMCPResources(server: "demo")
    #expect(resources.first?.uri == "file:///demo.txt")
    let resource = try await client.readMCPResource(server: "demo", uri: "file:///demo.txt")
    #expect(resource.contents.count == 1)

    let prompts = try await client.listMCPPrompts(server: "demo")
    #expect(prompts.first?.qualifiedName == "demo/summarize")
    let prompt = try await client.getMCPPrompt(server: "demo", name: "summarize")
    #expect(prompt.messages.count == 1)

    #expect(try await client.removeMCPServer("demo"))
    try await client.shutdown()
}


@Test func bundledRuntimeBootsWithoutNPMInstall() async throws {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("yeet-bundled-runtime-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    let script = try BundledBridge.scriptURL()
    #expect(FileManager.default.fileExists(atPath: script.path))

    let client = try await CallCoreClient.startBundled(
        environment: ["YEET_CONFIG_DIR": directory.path]
    )
    let providers = try await client.listProviders()
    #expect(providers.contains("openai"))
    #expect(providers.contains("anthropic"))
    #expect(providers.contains("gemini"))
    #expect(providers.contains("openrouter"))
    #expect(try await client.configDirectory().path == directory.path)
    #expect(try await client.listSkills().isEmpty)

    _ = try await client.setMCPServer(.http(name: "demo", url: "https://example.test/mcp"))
    #expect(try await client.listMCPServers().map(\.name) == ["demo"])
    #expect(try await client.removeMCPServer("demo"))

    try await client.shutdown()
}

@Test func bundledEditDaemonRoundTrip() async throws {
    let fileManager = FileManager.default
    let root = fileManager.temporaryDirectory
        .appendingPathComponent("yeet-edit-root-\(UUID().uuidString)", isDirectory: true)
    let config = fileManager.temporaryDirectory
        .appendingPathComponent("yeet-edit-config-\(UUID().uuidString)", isDirectory: true)
    defer {
        try? fileManager.removeItem(at: root)
        try? fileManager.removeItem(at: config)
    }
    try fileManager.createDirectory(at: root, withIntermediateDirectories: true)
    try fileManager.createDirectory(at: config, withIntermediateDirectories: true)
    try Data("one\ntwo\n".utf8).write(to: root.appendingPathComponent("a.txt"))

    let client = try YeetEditClient.startBundled(
        root: root,
        environment: ["YEET_CONFIG_DIR": config.path]
    )
    let read = try await client.read(path: "a.txt", startLine: 1, endLine: 2)
    let result = try await client.apply(
        YeetApplyRequest(changes: [
            YeetFileChange(
                path: "a.txt",
                snapshot: read.snapshot,
                edits: [.replace(range: .init(start: 2, end: 2), text: "TWO")]
            )
        ])
    )

    #expect(result.files.first?.operation == "update")
    #expect(result.files.first?.snapshot?.hasPrefix("s_") == true)
    #expect(String(decoding: try Data(contentsOf: root.appendingPathComponent("a.txt")), as: UTF8.self) == "one\nTWO\n")
}
