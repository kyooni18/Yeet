struct ModelCommand {
    static let definition = Command(
        name: "/model",
        description: "Show or set the active model",
        action: run
    )

    @MainActor
    private static func run(arguments: String, in session: Session) throws {
        let config = ConfigStore()
        let model = arguments.trimmingCharacters(in: .whitespacesAndNewlines)

        if model.isEmpty {
            session.showCommandResponse("Current model: \(try config.model() ?? "<not set>")")
        } else {
            try config.setModel(model)
            session.showCommandResponse("Model set to \(try config.model() ?? model)")
        }
    }
}
