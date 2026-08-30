import HarnessCallCore

extension Usage {
    /// Adds one response's usage while preserving missing provider fields.
    mutating func accumulate(_ other: Usage) {
        inputTokens = Self.add(inputTokens, other.inputTokens)
        outputTokens = Self.add(outputTokens, other.outputTokens)
        totalTokens = Self.add(totalTokens, other.totalTokens)
        cachedInputTokens = Self.add(cachedInputTokens, other.cachedInputTokens)
    }

    var summary: String {
        let total = totalTokens.map(String.init) ?? "?"
        let input = inputTokens.map(String.init) ?? "?"
        let output = outputTokens.map(String.init) ?? "?"
        return "in=\(input) out=\(output) total=\(total)"
    }

    private static func add(_ lhs: Int?, _ rhs: Int?) -> Int? {
        guard lhs != nil || rhs != nil else { return nil }
        return (lhs ?? 0) + (rhs ?? 0)
    }
}
