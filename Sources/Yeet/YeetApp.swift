import SwiftTUI
import Foundation
import Darwin

struct MainView: View {
    @State private var session = Session()


    var body: some View {
        ZStack(alignment: .topLeading) {
            VStack {
                ConversationView(
                    entries: session.conversation,
                    scrollRevision: session.conversationRevision
                )
                if let errorMessage = session.errorMessage {
                    Text(errorMessage)
                        .foregroundStyle(.danger)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                InputField(
                    text: $session.inputText,
                    model: session.activeModel,
                    suggestions: session.commandSuggestions,
                    selectedSuggestionIndex: session.selectedSuggestionIndex,
                    onSubmit: session.submit,
                    onSelectSuggestion: session.selectCommandSuggestion,
                    tokenUsage: session.tokenUsage,
                    creditUsage: session.creditUsage
                )
            }
            // Render after the chat stack so the panel is a true overlay. The
            // offset places it immediately below the three-row input editor,
            // leaving the editor visible and focused above it.
            if session.isModelPickerPresented {
                HStack {
                    Spacer()
                    ModelPickerView(session: session)
                        .offset(y: 6)
                    Spacer()
                }
            }
        }
    }
}

struct InteractiveApp: App {
    var body: some Scene {
        WindowGroup("Yeet") {
            MainView()
        }
    }
}

@main
struct Yeet {
    static func main() async {
        if CommandLine.arguments.count > 1 {
            do {
                try await YeetCLI().run(arguments: Array(CommandLine.arguments.dropFirst()))
            } catch {
                fputs("yeet: \(error)\n", stderr)
                exit(EXIT_FAILURE)
            }
        } else {
            await InteractiveApp.main()
        }
    }
}
