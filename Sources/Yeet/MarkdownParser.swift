import Foundation

/// A deliberately small Markdown parser for terminal output.  It covers the
/// constructs most model responses use while leaving unsupported Markdown
/// literal and safe to display in a terminal.
enum MarkdownBlock: Equatable, Sendable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case unorderedList([String])
    case orderedList([String])
    case quote(String)
    case code(language: String?, text: String)
    case thematicBreak
}
struct MarkdownInlineStyle: OptionSet, Equatable, Sendable {
    let rawValue: Int

    static let bold = Self(rawValue: 1 << 0)
    static let italic = Self(rawValue: 1 << 1)
    static let code = Self(rawValue: 1 << 2)
}

struct MarkdownInlineRun: Equatable, Sendable {
    var text: String
    var style: MarkdownInlineStyle = []
    var destination: String?
}

enum MarkdownParser {
    static func parse(_ source: String) -> [MarkdownBlock] {
        let lines = source.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .components(separatedBy: "\n")

        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var unordered: [String] = []
        var ordered: [String] = []
        var quote: [String] = []
        var codeLines: [String] = []
        var codeLanguage: String?

        func flushParagraph() {
            guard !paragraph.isEmpty else { return }
            blocks.append(.paragraph(paragraph.joined(separator: "\n")))
            paragraph.removeAll(keepingCapacity: true)
        }

        func flushLists() {
            if !unordered.isEmpty {
                blocks.append(.unorderedList(unordered))
                unordered.removeAll(keepingCapacity: true)
            }
            if !ordered.isEmpty {
                blocks.append(.orderedList(ordered))
                ordered.removeAll(keepingCapacity: true)
            }
        }

        func flushQuote() {
            guard !quote.isEmpty else { return }
            blocks.append(.quote(quote.joined(separator: "\n")))
            quote.removeAll(keepingCapacity: true)
        }

        func flushCode() {
            blocks.append(.code(language: codeLanguage, text: codeLines.joined(separator: "\n")))
            codeLines.removeAll(keepingCapacity: true)
            codeLanguage = nil
        }

        var inCodeFence = false
        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if inCodeFence {
                if trimmed.hasPrefix("```") {
                    flushCode()
                    inCodeFence = false
                } else {
                    codeLines.append(line)
                }
                continue
            }

            if trimmed.hasPrefix("```") {
                flushParagraph()
                flushLists()
                flushQuote()
                inCodeFence = true
                let language = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                codeLanguage = language.isEmpty ? nil : String(language)
                continue
            }

            if trimmed.isEmpty {
                flushParagraph()
                flushLists()
                flushQuote()
                continue
            }

            if let heading = headingParts(for: line) {
                flushParagraph()
                flushLists()
                flushQuote()
                blocks.append(.heading(level: heading.level, text: heading.text))
                continue
            }

            if isThematicBreak(trimmed) {
                flushParagraph()
                flushLists()
                flushQuote()
                blocks.append(.thematicBreak)
                continue
            }

            if let item = unorderedItem(for: line) {
                flushParagraph()
                flushQuote()
                if !ordered.isEmpty {
                    blocks.append(.orderedList(ordered))
                    ordered.removeAll(keepingCapacity: true)
                }
                unordered.append(item)
                continue
            }

            if let item = orderedItem(for: line) {
                flushParagraph()
                flushQuote()
                if !unordered.isEmpty {
                    blocks.append(.unorderedList(unordered))
                    unordered.removeAll(keepingCapacity: true)
                }
                ordered.append(item)
                continue
            }

            if let quoteLine = quoteItem(for: line) {
                flushParagraph()
                flushLists()
                quote.append(quoteLine)
                continue
            }

            flushLists()
            flushQuote()
            paragraph.append(line)
        }

        if inCodeFence {
            flushCode()
        }
        flushParagraph()
        flushLists()
        flushQuote()
        return blocks
    }

    static func inlineRuns(_ source: String) -> [MarkdownInlineRun] {
        parseInline(source, style: [])
    }

    private static func parseInline(_ source: String, style: MarkdownInlineStyle) -> [MarkdownInlineRun] {
        var result: [MarkdownInlineRun] = []
        var index = source.startIndex

        func append(_ text: String, style: MarkdownInlineStyle, destination: String? = nil) {
            guard !text.isEmpty else { return }
            if var previous = result.last,
               previous.style == style,
               previous.destination == destination {
                previous.text += text
                result[result.count - 1] = previous
            } else {
                result.append(.init(text: text, style: style, destination: destination))
            }
        }

        while index < source.endIndex {
            if source[index] == "\\" {
                let next = source.index(after: index)
                if next < source.endIndex {
                    append(String(source[next]), style: style)
                    index = source.index(after: next)
                } else {
                    append("\\", style: style)
                    index = next
                }
                continue
            }

            if source[index] == "[",
               let labelEnd = source[index...].firstIndex(of: "]"),
               labelEnd < source.endIndex,
               source.index(after: labelEnd) < source.endIndex,
               source[source.index(after: labelEnd)] == "(",
               let destinationEnd = source[source.index(after: labelEnd)...].firstIndex(of: ")") {
                let destinationStart = source.index(labelEnd, offsetBy: 2)
                let label = String(source[source.index(after: index)..<labelEnd])
                let destination = String(source[destinationStart..<destinationEnd])
                if !label.isEmpty, !destination.isEmpty {
                    append(label, style: style, destination: destination)
                    index = source.index(after: destinationEnd)
                    continue
                }
            }

            if source[index] == "`" {
                let marker = source[index]
                if let end = source[source.index(after: index)...].firstIndex(of: marker) {
                    let text = String(source[source.index(after: index)..<end])
                    append(text, style: style.union(.code))
                    index = source.index(after: end)
                    continue
                }
            }

            let marker: Character?
            let markerStyle: MarkdownInlineStyle
            if source[index...].hasPrefix("**") {
                marker = "*"
                markerStyle = .bold
            } else if source[index...].hasPrefix("__") {
                marker = "_"
                markerStyle = .bold
            } else if source[index] == "*" || source[index] == "_" {
                marker = source[index]
                markerStyle = .italic
            } else {
                marker = nil
                markerStyle = []
            }

            if marker != nil {
                let width = markerStyle == .bold ? 2 : 1
                let contentStart = source.index(index, offsetBy: width)
                let token = String(source[index..<contentStart])
                if let end = source[contentStart...].range(of: token)?.lowerBound,
                   end > contentStart {
                    let inner = String(source[contentStart..<end])
                    result.append(contentsOf: parseInline(inner, style: style.union(markerStyle)))
                    index = source.index(end, offsetBy: width)
                    continue
                }
            }

            append(String(source[index]), style: style)
            index = source.index(after: index)
        }
        return result
    }

    private static func headingParts(for line: String) -> (level: Int, text: String)? {
        let prefix = line.prefix { $0 == "#" }
        guard !prefix.isEmpty, prefix.count <= 6 else { return nil }
        let remainder = line.dropFirst(prefix.count)
        guard remainder.first == " " || remainder.first == "\t" else { return nil }
        return (prefix.count, remainder.drop(while: { $0 == " " || $0 == "\t" }).description)
    }

    private static func unorderedItem(for line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        for marker in ["- ", "* ", "+ "] where trimmed.hasPrefix(marker) {
            return String(trimmed.dropFirst(marker.count))
        }
        return nil
    }

    private static func orderedItem(for line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard let separator = trimmed.firstIndex(of: ".") else { return nil }
        let number = trimmed[..<separator]
        guard !number.isEmpty, number.allSatisfy(\.isNumber) else { return nil }
        let contentStart = trimmed.index(after: separator)
        guard contentStart < trimmed.endIndex, trimmed[contentStart] == " " else { return nil }
        return String(trimmed[trimmed.index(after: contentStart)...])
    }

    private static func quoteItem(for line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix(">") else { return nil }
        return String(trimmed.dropFirst().drop(while: { $0 == " " || $0 == "\t" }))
    }

    private static func isThematicBreak(_ line: String) -> Bool {
        guard line.count >= 3 else { return false }
        let characters = line.filter { !$0.isWhitespace }
        guard let first = characters.first, ["-", "*", "_"].contains(first) else { return false }
        return characters.allSatisfy { $0 == first }
    }
}
