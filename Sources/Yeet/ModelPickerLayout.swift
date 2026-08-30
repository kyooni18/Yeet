/// Pure selection and paging rules for the model picker.
///
/// Keeping these calculations outside `Session` makes the UI state easier to
/// read and gives the picker one place to define its behavior.
enum ModelPickerLayout {
    static let windowSize = 8

    static func filteredModels(_ models: [String], query: String) -> [String] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return models }
        return models.filter { $0.localizedCaseInsensitiveContains(query) }
    }

    static func window(
        for models: [String],
        selectedModel: String,
        size: Int = windowSize
    ) -> Range<Int> {
        guard !models.isEmpty else { return 0..<0 }
        guard models.count > size else { return 0..<models.count }

        let selectedIndex = models.firstIndex(of: selectedModel) ?? 0
        let maxStart = models.count - size
        let start = min(max(0, selectedIndex - size / 2), maxStart)
        return start..<(start + size)
    }

    static func windowDescription(for window: Range<Int>, total: Int) -> String {
        guard !window.isEmpty else { return "0/\(total)" }
        return "\(window.lowerBound + 1)-\(window.upperBound) / \(total)"
    }

    static func reconciledSelection(current: String, models: [String]) -> String {
        models.first(where: { $0 == current }) ?? models.first ?? ""
    }

    static func movedSelection(
        current: String,
        models: [String],
        offset: Int
    ) -> String {
        guard !models.isEmpty else { return "" }
        let index = models.firstIndex(of: current) ?? 0
        return models[min(max(index + offset, 0), models.count - 1)]
    }
}
