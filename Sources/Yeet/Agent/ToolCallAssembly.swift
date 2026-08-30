import Foundation
import HarnessCallCore

/// Reconstructs tool calls when a provider emits only streamed deltas.
/// The lead orchestration uses this protocol-level assembly path.
struct PartialToolCall: Sendable {
    var id: String
    var name: String
    var arguments = ""

    init(id: String, name: String) {
        self.id = id
        self.name = name
    }

    mutating func apply(id: String?, name: String?, argumentsDelta: String?) {
        if let id, !id.isEmpty { self.id = id }
        if let name, !name.isEmpty { self.name = name }
        if let argumentsDelta { arguments += argumentsDelta }
    }
}

enum ToolCallAssembly {
    static func collect(
        decoded: [Int: ToolCall],
        partial: [Int: PartialToolCall]
    ) -> [ToolCall] {
        var calls = decoded
        for (index, partial) in partial where calls[index] == nil {
            let arguments = (try? JSONDecoder().decode(JSONValue.self, from: Data(partial.arguments.utf8))) ?? .object([:])
            calls[index] = ToolCall(
                id: partial.id.isEmpty ? "tool-\(index)" : partial.id,
                name: partial.name,
                arguments: arguments
            )
        }
        return calls.keys.sorted().compactMap { calls[$0] }
    }
}
