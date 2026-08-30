struct Command: Identifiable, Sendable {
    typealias Action = @MainActor @Sendable (_ arguments: String, _ session: Session) throws -> Void

    let name: String
    let description: String
    private let action: Action

    var id: String { name }

    init(
        name: String,
        description: String,
        action: @escaping Action
    ) {
        self.name = name
        self.description = description
        self.action = action
    }

    @MainActor
    func run(arguments: String, in session: Session) throws {
        try action(arguments, session)
    }
}

struct CommandRegistry: Sendable {
    private(set) var commands: [Command]

    init(commands: [Command] = CommandRegistry.defaultCommands) {
        self.commands = commands
    }

    static var defaultCommands: [Command] {
        [
            ModelCommand.definition,
            Command(name: "/help", description: "Show available commands") { _, session in
                session.showCommandResponse(session.commandHelpText)
            },
            Command(name: "/clear", description: "Clear the current response") { _, session in
                session.clearCommandResponse()
            }
        ]
    }

    mutating func register(_ command: Command) {
        let normalizedName = command.name.lowercased()
        if let index = commands.firstIndex(where: { $0.name.lowercased() == normalizedName }) {
            commands[index] = command
        } else {
            commands.append(command)
        }
    }

    func command(named name: String) -> Command? {
        let normalizedName = name.lowercased()
        return commands.first { $0.name.lowercased() == normalizedName }
    }

    func suggestions(for input: String) -> [Command] {
        guard input.first == "/", !input.contains(where: { $0.isWhitespace }) else {
            return []
        }

        let query = input.lowercased()
        return commands.filter { $0.name.lowercased().hasPrefix(query) }
    }
}
