import SwiftTUI

struct CommandSuggestionsView: View {
    let suggestions: [Command]
    let selectedSuggestionIndex: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(suggestions.indices, id: \.self) { index in
                let suggestion = suggestions[index]
                HStack(spacing: 1) {
                    Text(index == selectedSuggestionIndex ? ">" : " ")
                    Text(suggestion.name)
                    Text("—")
                    Text(suggestion.description).foregroundStyle(.secondary)
                }
                .foregroundStyle(index == selectedSuggestionIndex ? .tint : .primary)
            }
        }
        .padding(.leading, 1)
    }
}
