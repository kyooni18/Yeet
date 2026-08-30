import Foundation
import HarnessCallCore

struct YeetCLI {
    private let config = ConfigStore()

    func run(arguments: [String]) async throws {
        guard let command = arguments.first else {
            print(Self.help)
            return
        }

        let rest = Array(arguments.dropFirst())
        switch command {
        case "-h", "--help", "help":
            print(Self.help)
        case "-v", "--version", "version":
            print(YeetVersion.current)
        case "doctor":
            try await doctor()
        case "model":
            try modelCommand(rest)
        case "run":
            try await runModel(rest)
        case "auth":
            try await authCommand(rest)
        case "skill", "skills":
            try await skillCommand(rest)
        case "mcp":
            try await mcpCommand(rest)
        default:
            throw CLIError.unknownCommand(command)
        }
    }

    private func doctor() async throws {
        let runtime = try BundledBridge.runtimeDirectory()
        let node = try BundledBridge.nodeExecutable()
        try config.ensure()
        let providers = try await withCore { core in
            try await core.listProviders()
        }
        print("yeet \(YeetVersion.current)")
        print("node: \(node)")
        print("runtime: \(runtime.path)")
        print("config: \(config.directory.path)")
        print("providers: \(providers.joined(separator: ", "))")
    }

    private func modelCommand(_ args: [String]) throws {
        let subcommand = args.first ?? "get"
        switch subcommand {
        case "get":
            print(try config.model() ?? "<not set>")
        case "set":
            guard args.count >= 2 else { throw CLIError.usage("yeet model set provider/model") }
            try config.setModel(args[1])
            print(try config.model() ?? args[1])
        default:
            throw CLIError.usage("yeet model [get|set provider/model]")
        }
    }

    private func runModel(_ args: [String]) async throws {
        var modelOverride: String?
        var promptParts: [String] = []
        var index = 0
        while index < args.count {
            if args[index] == "--model" {
                guard index + 1 < args.count else { throw CLIError.usage("yeet run [--model provider/model] [prompt]") }
                modelOverride = try ConfigStore.validateModelID(args[index + 1])
                index += 2
            } else {
                promptParts.append(args[index])
                index += 1
            }
        }

        let model = try modelOverride ?? config.model() ?? { throw ConfigError.missingModel }()
        let prompt: String
        if promptParts.isEmpty {
            let data = FileHandle.standardInput.readDataToEndOfFile()
            prompt = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        } else {
            prompt = promptParts.joined(separator: " ")
        }
        guard !prompt.isEmpty else { throw CLIError.usage("yeet run [--model provider/model] prompt") }

        try await withCore { core in
            for try await event in core.stream(.init(model: model, messages: [.user(prompt)])) {
                switch event {
                case .textDelta(let text):
                    FileHandle.standardOutput.write(Data(text.utf8))
                case .toolCall(_, let toolCall):
                    print("\n[tool] \(toolCall.name)")
                default:
                    break
                }
            }
            print("")
        }
    }

    private func authCommand(_ args: [String]) async throws {
        guard let subcommand = args.first else {
            throw CLIError.usage("yeet auth [status|set-key|login|logout] ...")
        }
        switch subcommand {
        case "status":
            try await withCore { core in
                let providers = args.count >= 2 ? [args[1]] : try await core.listProviders()
                for provider in providers {
                    let status = try await core.authStatus(for: provider)
                    let state = status.authenticated ? "authenticated" : "not authenticated"
                    print("\(provider): \(state) via \(status.method.rawValue)")
                }
            }
        case "set-key":
            guard args.count >= 2 else { throw CLIError.usage("printf key | yeet auth set-key provider") }
            let provider = args[1]
            let data = FileHandle.standardInput.readDataToEndOfFile()
            let key = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            guard !key.isEmpty else { throw CLIError.usage("printf key | yeet auth set-key provider") }
            try await withCore { core in
                let status = try await core.setAPIKey(key, for: provider)
                print("\(status.provider): authenticated")
            }
        case "login":
            guard args.count >= 2 else { throw CLIError.usage("yeet auth login provider") }
            let provider = args[1]
            let env = ProcessInfo.processInfo.environment
            let options: BrowserLoginOptions?
            if provider == "gemini" {
                let clientId: String?
                if let value = env["GEMINI_OAUTH_CLIENT_ID"] { clientId = value }
                else { clientId = env["GOOGLE_CLIENT_ID"] }
                let clientSecret: String?
                if let value = env["GEMINI_OAUTH_CLIENT_SECRET"] { clientSecret = value }
                else { clientSecret = env["GOOGLE_CLIENT_SECRET"] }
                let projectId: String?
                if let value = env["GEMINI_PROJECT_ID"] { projectId = value }
                else { projectId = env["GOOGLE_CLOUD_PROJECT"] }
                options = .init(
                    clientId: clientId,
                    clientSecret: clientSecret,
                    projectId: projectId
                )
            } else {
                options = nil
            }
            try await withCore { core in
                let status = try await core.loginInBrowser(provider, options: options)
                print("\(status.provider): authenticated via \(status.method.rawValue)")
            }
        case "logout":
            guard args.count >= 2 else { throw CLIError.usage("yeet auth logout provider") }
            try await withCore { core in
                let status = try await core.logout(args[1])
                print("\(status.provider): logged out")
            }
        default:
            throw CLIError.usage("yeet auth [status|set-key|login|logout] ...")
        }
    }

    private func skillCommand(_ args: [String]) async throws {
        let subcommand = args.first ?? "list"
        switch subcommand {
        case "list":
            try await withCore { core in
                for skill in try await core.listSkills() {
                    print("\(skill.name)\t\(skill.description)")
                }
            }
        case "show":
            guard args.count >= 2 else { throw CLIError.usage("yeet skill show name") }
            try await withCore { core in
                let skill = try await core.loadSkill(args[1])
                print(skill.instructions)
            }
        case "read":
            guard args.count >= 3 else { throw CLIError.usage("yeet skill read name path") }
            try await withCore { core in
                print(try await core.readSkillFile(args[1], path: args[2]))
            }
        default:
            throw CLIError.usage("yeet skill [list|show|read] ...")
        }
    }

    private func mcpCommand(_ args: [String]) async throws {
        let subcommand = args.first ?? "list"
        switch subcommand {
        case "list":
            try await withCore { core in
                for server in try await core.listMCPServers() {
                    print("\(server.name)\t\(server.transport.rawValue)\t\(server.connected ? "connected" : "idle")")
                }
            }
        case "add-stdio":
            guard args.count >= 3 else { throw CLIError.usage("yeet mcp add-stdio name command [args...]") }
            let server = MCPServerConfiguration.stdio(
                name: args[1],
                command: args[2],
                args: Array(args.dropFirst(3))
            )
            try await withCore { core in
                let saved = try await core.setMCPServer(server)
                print(saved.name)
            }
        case "add-http":
            guard args.count >= 3 else { throw CLIError.usage("yeet mcp add-http name url") }
            let server = MCPServerConfiguration.http(name: args[1], url: args[2])
            try await withCore { core in
                let saved = try await core.setMCPServer(server)
                print(saved.name)
            }
        case "remove":
            guard args.count >= 2 else { throw CLIError.usage("yeet mcp remove name") }
            try await withCore { core in
                print(try await core.removeMCPServer(args[1]) ? "removed" : "not found")
            }
        case "tools":
            let server = args.count >= 2 ? args[1] : nil
            try await withCore { core in
                for tool in try await core.listMCPTools(server: server) {
                    print("\(tool.qualifiedName)\t\(tool.description ?? "")")
                }
            }
        case "call":
            guard args.count >= 2 else { throw CLIError.usage("yeet mcp call server/tool [json-arguments]") }
            let arguments = try decodeJSONObject(args.count >= 3 ? args[2] : "{}")
            try await withCore { core in
                try printJSON(await core.callMCPTool(args[1], arguments: arguments))
            }
        case "resources":
            let server = args.count >= 2 ? args[1] : nil
            try await withCore { core in
                for resource in try await core.listMCPResources(server: server) {
                    print("\(resource.server)\t\(resource.uri)\t\(resource.name)")
                }
            }
        case "resource":
            guard args.count >= 3 else { throw CLIError.usage("yeet mcp resource server uri") }
            try await withCore { core in
                try printJSON(await core.readMCPResource(server: args[1], uri: args[2]))
            }
        case "prompts":
            let server = args.count >= 2 ? args[1] : nil
            try await withCore { core in
                for prompt in try await core.listMCPPrompts(server: server) {
                    print("\(prompt.qualifiedName)\t\(prompt.description ?? "")")
                }
            }
        case "prompt":
            guard args.count >= 3 else { throw CLIError.usage("yeet mcp prompt server name [json-string-arguments]") }
            let arguments = try decodeStringObject(args.count >= 4 ? args[3] : "{}")
            try await withCore { core in
                try printJSON(await core.getMCPPrompt(server: args[1], name: args[2], arguments: arguments))
            }
        default:
            throw CLIError.usage("yeet mcp [list|add-stdio|add-http|remove|tools|call|resources|resource|prompts|prompt] ...")
        }
    }

    private func withCore<T>(_ body: (CallCoreClient) async throws -> T) async throws -> T {
        let core = try await CallCoreClient.startBundled()
        do {
            let result = try await body(core)
            try await core.shutdown()
            return result
        } catch {
            try? await core.shutdown()
            throw error
        }
    }

    private func decodeJSONObject(_ text: String) throws -> [String: JSONValue] {
        let data = Data(text.utf8)
        return try JSONDecoder().decode([String: JSONValue].self, from: data)
    }

    private func decodeStringObject(_ text: String) throws -> [String: String] {
        let data = Data(text.utf8)
        return try JSONDecoder().decode([String: String].self, from: data)
    }

    private func printJSON<T: Encodable>(_ value: T) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        print(String(decoding: try encoder.encode(value), as: UTF8.self))
    }

    static let help = """
    yeet 0.1.0

    Usage:
      yeet doctor
      yeet model get
      yeet model set provider/model
      yeet run [--model provider/model] [prompt]
      yeet auth status [provider]
      printf key | yeet auth set-key provider
      yeet auth login provider
      yeet auth logout provider
      yeet skill list
      yeet skill show name
      yeet skill read name path
      yeet mcp list
      yeet mcp add-stdio name command [args...]
      yeet mcp add-http name url
      yeet mcp remove name
      yeet mcp tools [server]
      yeet mcp call server/tool [json-arguments]
      yeet mcp resources [server]
      yeet mcp resource server uri
      yeet mcp prompts [server]
      yeet mcp prompt server name [json-string-arguments]

    State:
      ~/.yeet/config.json
      ~/.yeet/credentials.json
      ~/.yeet/mcp.json
      ~/.yeet/skills/

    Runtime:
      The TypeScript call core is shipped inside the SwiftPM resource bundle.
      Node.js 20+ is required at runtime. Set YEET_NODE to override its path.
    """
}

enum CLIError: Error, CustomStringConvertible {
    case unknownCommand(String)
    case usage(String)

    var description: String {
        switch self {
        case .unknownCommand(let command):
            return "Unknown command: \(command)"
        case .usage(let usage):
            return "Usage: \(usage)"
        }
    }
}
