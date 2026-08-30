/// Parsed slash command input.
struct CommandInvocation: Equatable, Sendable {
    let name: String
    let arguments: String

    init(input: String) {
        let parts = input.split(
            maxSplits: 1,
            omittingEmptySubsequences: true,
            whereSeparator: { $0.isWhitespace }
        )
        name = parts.first.map(String.init)?.lowercased() ?? input.lowercased()
        arguments = parts.dropFirst().first
            .map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }
}
