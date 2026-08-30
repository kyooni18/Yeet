import Testing
import HarnessCallCore
@testable import Yeet

@Test
func commandInvocationSeparatesNameAndTrimmedArguments() {
    let invocation = CommandInvocation(input: "/MODEL   openai/gpt-5  ")

    #expect(invocation.name == "/model")
    #expect(invocation.arguments == "openai/gpt-5")
}

@Test
func modelPickerLayoutKeepsSelectionVisibleInWindow() {
    let models = (1...12).map { "model-\($0)" }
    let window = ModelPickerLayout.window(
        for: models,
        selectedModel: "model-10"
    )

    #expect(Array(models[window]) == [
        "model-5", "model-6", "model-7", "model-8",
        "model-9", "model-10", "model-11", "model-12"
    ])
}

@Test
func usageAccumulationPreservesUnknownFields() {
    var total = Usage(inputTokens: 2, outputTokens: nil, totalTokens: 2)
    total.accumulate(Usage(inputTokens: nil, outputTokens: 3, totalTokens: nil))

    #expect(total.inputTokens == 2)
    #expect(total.outputTokens == 3)
    #expect(total.totalTokens == 2)
    #expect(total.summary == "in=2 out=3 total=2")
}
