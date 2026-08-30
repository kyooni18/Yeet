import SwiftTUI
import HarnessCallCore

struct InputFieldStatusBar: View {
	let model: String
	let tokenUsage: Usage
	let contextRange: Range<Int>

	// Keep the old argument source-compatible; credits are intentionally not
	// part of this compact status bar.
	init(
		model: String,
		tokenUsage: Usage = Usage(inputTokens: 0, outputTokens: 0, totalTokens: 0),
		creditUsage _: Int = 0,
		contextRange: Range<Int>? = nil
	) {
		self.model = model
		self.tokenUsage = tokenUsage
		let total = tokenUsage.totalTokens
			?? ((tokenUsage.inputTokens ?? 0) + (tokenUsage.outputTokens ?? 0))
		self.contextRange = contextRange ?? 0..<max(0, total)
	}

	var body: some View {
		// Keep the status bar to one terminal row even in narrow windows. A
		// single line also lets SwiftTUI clip the trailing model instead of
		// increasing the input field's height.
		HStack(spacing: 1) {
			Text("↑\(inputTokenCount)")
			Text("↓\(outputTokenCount)")
			Text("\(contextRange.lowerBound)…\(contextRange.upperBound)")
			Spacer()
			Text(model)
		}
		.lineLimit(1)
		.frame(maxWidth: .infinity, alignment: .leading)
	}

	private var inputTokenCount: Int { tokenUsage.inputTokens ?? 0 }
	private var outputTokenCount: Int { tokenUsage.outputTokens ?? 0 }
}

struct InputField: View {
	@Binding var text: String
	let model: String
	@Environment(\.isFocused) private var isFocused
	@Environment(\.terminalSize) private var terminalSize
	var suggestions: [Command] = []
	var selectedSuggestionIndex = 0

	var onSubmit: @MainActor @Sendable () -> Void
	var onSelectSuggestion: @MainActor @Sendable () -> Void = {}
	var tokenUsage = Usage(inputTokens: 0, outputTokens: 0, totalTokens: 0)
	var creditUsage = 0

	var body: some View {
		VStack(alignment: .leading, spacing: 0) {
			TextEditor(text: $text)
				.onKeyPress(.tab) { _ in
					guard !suggestions.isEmpty else { return .ignored }
					onSelectSuggestion()
					return .handled
				}
				.onKeyPress(.return, modifiers: .shift) { _ in
					insertLineBreak()
					return .handled
				}
				.onKeyPress(.return) { _ in
					onSubmit()
					return .handled
				}
				.frame(
					height: InputFieldSizing.height(
						for: text,
						terminalWidth: terminalSize.width,
						includesCaret: isFocused
					)
				)
				.frame(maxWidth: .infinity)
				.overlay(alignment: .topLeading) {
					if text.isEmpty && !isFocused {
						Text("Yeet.")
							.foregroundStyle(.secondary)
							.padding(.init(horizontal: 1, vertical: 1))
					}
				}
			InputFieldStatusBar(model: model, tokenUsage: tokenUsage, creditUsage: creditUsage)

			if !suggestions.isEmpty {
				CommandSuggestionsView(
					suggestions: suggestions,
					selectedSuggestionIndex: selectedSuggestionIndex
				)
			}
		}
	}

	@MainActor
	private func insertLineBreak() {
		// SwiftTUI's TextEditor does not currently map Shift-Return to its
		// multiline reducer. Keep the app's Return-to-submit behavior while
		// still allowing the terminal's explicit Shift-Return chord to create a
		// new line.
		text.append("\n")
	}
}

private enum InputFieldSizing {
	static let minimumHeight = 3
	static let maximumHeight = 6
	// TextEditor reserves one cell on each horizontal and vertical side for
	// its border/padding. Keep these values aligned with SwiftTUI's editor body.
	static let horizontalPadding = 2
	static let verticalPadding = 2

	static func height(for text: String, terminalWidth: Int, includesCaret: Bool) -> Int {
		let contentWidth = max(1, terminalWidth - horizontalPadding)
		// TextEditor renders a synthetic '_' caret while focused. Include that
		// cell in the measurement so a line that is exactly full-width does not
		// wrap the caret into a clipped, extra row.
		let measuredText = includesCaret ? text + "_" : text
		let lineCount = layoutText(for: measuredText, width: contentWidth).size.height
		let contentHeight = lineCount + verticalPadding
		return min(maximumHeight, max(minimumHeight, contentHeight))
	}
}
