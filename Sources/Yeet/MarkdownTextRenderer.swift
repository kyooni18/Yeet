import SwiftTUI

/// Converts parsed Markdown runs into the terminal's rich text representation.
/// Keeping this separate from the view makes every Markdown-bearing event use
/// the same styling rules.
enum MarkdownTextRenderer {
    @MainActor
    static func text(_ source: String, prefix: String = "") -> Text {
        var interpolation = Text.StringInterpolation(
            literalCapacity: prefix.count,
            interpolationCount: 1
        )
        interpolation.appendLiteral(prefix)

        for run in MarkdownParser.inlineRuns(source) {
            var styled = Text(run.text)
                .bold(run.style.contains(.bold))
                .italic(run.style.contains(.italic))

            if run.style.contains(.code) {
                styled = styled.foregroundStyle(.cyan).cellBackground(.background)
            } else if run.destination != nil {
                styled = styled.foregroundStyle(.link).underline()
            }

            if let destination = run.destination {
                interpolation.appendInterpolation(
                    Link(styled, destination: LinkDestination(destination))
                )
            } else {
                interpolation.appendInterpolation(styled)
            }
        }

        return Text(Text.RichContent(stringInterpolation: interpolation))
    }
}
