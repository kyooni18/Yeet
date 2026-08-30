import Foundation
import HarnessCallCore

extension JSONValue {
    /// Stable, readable JSON for tool-call details shown in the transcript.
    var prettyJSONString: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(self) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }
}
