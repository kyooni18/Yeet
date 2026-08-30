import SwiftTUI

/// Searchable model chooser shown above the chat input when `/model` runs.
struct ModelPickerView: View {
    @Bindable var session: Session
    @FocusState private var filterIsFocused: Bool
    @Namespace private var pickerFocusScope

    var body: some View {
        if session.isModelPickerPresented {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("Select model")
                        .foregroundStyle(.primary)
                    Spacer()
                    Text(session.modelPickerWindowDescription)
                        .foregroundStyle(.secondary)
                }

                TextField("Filter models...", text: $session.modelSearchText)
                    .focused($filterIsFocused)
                    .prefersDefaultFocus(in: pickerFocusScope)
                    .onSubmit { session.selectCurrentModel() }
                    .onKeyPress(.escape) { _ in
                        session.dismissModelPicker()
                        return .handled
                    }

                if session.isLoadingModels {
                    Text("Loading available models...")
                        .foregroundStyle(.secondary)
                        .frame(height: 8, alignment: .topLeading)
                } else if session.filteredModels.isEmpty {
                    Text(
                        session.availableModels.isEmpty
                            ? "No models available."
                            : "No models match the filter."
                    )
                    .foregroundStyle(.secondary)
                    .frame(height: 8, alignment: .topLeading)
                } else {
                    Picker("Models", selection: $session.selectedModel) {
                        ForEach(session.modelPickerModels, id: \.self) { model in
                            Text(model).tag(model)
                        }
                    }
                    .pickerStyle(.inline)
                    .frame(height: 8, alignment: .topLeading)
                    .onKeyPress(.arrowUp) { _ in
                        session.moveModelSelection(by: -1)
                        return .handled
                    }
                    .onKeyPress(.arrowDown) { _ in
                        session.moveModelSelection(by: 1)
                        return .handled
                    }
                    .onKeyPress(.return) { _ in
                        session.selectCurrentModel()
                        return .handled
                    }
                    .onKeyPress(.escape) { _ in
                        session.dismissModelPicker()
                        return .handled
                    }
                }

                Text("↑/↓ select  ·  Return apply  ·  Esc close")
                    .foregroundStyle(.secondary)
            }
            .frame(width: 72, alignment: .topLeading)
            .padding(1)
            .background(.windowBackground)
            .border(.separator)
            .focusScope(pickerFocusScope)
            .onAppear { filterIsFocused = true }
            .onDisappear { filterIsFocused = false }
            .onKeyPress(.escape) { _ in
                session.dismissModelPicker()
                return .handled
            }
        }
    }
}
