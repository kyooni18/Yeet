struct ModelCommand {
    static let definition = Command(
        name: "/model",
        description: "Show or set the active model",
        action: run
    )

    @MainActor
    private static func run(arguments: String, in session: Session) throws {
        session.presentModelPicker(
            search: arguments.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }
}
