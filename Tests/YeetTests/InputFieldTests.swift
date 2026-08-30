import Testing
import HarnessCallCore
@testable import Yeet
@testable import SwiftTUICore
@testable import SwiftTUIRuntime
@testable import SwiftTUIViews

@MainActor
@Test
func inputFieldRendersAtItsAvailableWidth() {
  final class Box {
    var value = "hello"
  }

  let box = Box()
  let identity = Identity(components: ["InputFieldTest"])
  var environment = EnvironmentValues()
  environment.terminalSize = CellSize(width: 20, height: 10)

  let artifacts = DefaultRenderer().render(
    InputField(
      text: Binding(
        get: { box.value },
        set: { box.value = $0 }
      ),
      model: "model",
      onSubmit: {}
    )
    .frame(width: 20)
    .id(identity),
    context: .init(
      identity: Identity(components: ["Root"]),
      environmentValues: environment
    )
  )

  #expect(artifacts.rasterSurface.lines.count == 4)
  #expect(artifacts.rasterSurface.lines[0].count == 20)
}

@MainActor
@Test
func inputFieldGrowsForWrappedText() {
  final class Box {
    var value = "one two three four five"
  }

  let box = Box()
  let identity = Identity(components: ["InputFieldWrappedTest"])
  var environment = EnvironmentValues()
  environment.terminalSize = CellSize(width: 20, height: 10)

  let artifacts = DefaultRenderer().render(
    InputField(
      text: Binding(
        get: { box.value },
        set: { box.value = $0 }
      ),
      model: "model",
      onSubmit: {}
    )
    .frame(width: 20)
    .id(identity),
    context: .init(
      identity: Identity(components: ["Root"]),
      environmentValues: environment
    )
  )

  #expect(artifacts.rasterSurface.lines.count == 5)
}

@MainActor
@Test
func inputFieldReservesRoomForTheFocusedCaret() {
  final class Box {
    var value = String(repeating: "x", count: 18)
  }

  let box = Box()
  let identity = Identity(components: ["InputFieldCaretTest"])
  var environment = EnvironmentValues()
  environment.terminalSize = CellSize(width: 20, height: 10)
  environment.focusedIdentity = identity

  let artifacts = DefaultRenderer().render(
    InputField(
      text: Binding(
        get: { box.value },
        set: { box.value = $0 }
      ),
      model: "model",
      onSubmit: {}
    )
    .frame(width: 20)
    .id(identity),
    context: .init(
      identity: Identity(components: ["Root"]),
      environmentValues: environment
    )
  )

  // 18 characters fill the 18-cell editor content width. The focused
  // synthetic caret needs one additional cell, so the editor occupies two
  // content rows plus its border.
  #expect(artifacts.rasterSurface.lines.count == 5)
}

@MainActor
@Test
func inputFieldStatusBarShowsCompactUsageAndModel() {
  var environment = EnvironmentValues()
  environment.terminalSize = CellSize(width: 80, height: 4)

  let artifacts = DefaultRenderer().render(
    InputFieldStatusBar(
      model: "openai/gpt-5",
      tokenUsage: Usage(inputTokens: 10, outputTokens: 7, totalTokens: 17),
      creditUsage: 2
    )
    .frame(width: 80),
    context: .init(
      identity: Identity(components: ["InputFieldStatusBarTest"]),
      environmentValues: environment
    )
  )

  let rendered = artifacts.rasterSurface.lines.joined(separator: "\n")
  #expect(rendered.contains("↑10"))
  #expect(rendered.contains("↓7"))
  #expect(rendered.contains("0…17"))
  #expect(rendered.contains("openai/gpt-5"))
  #expect(!rendered.contains("Model:"))
  #expect(!rendered.contains("Tokens:"))
  #expect(!rendered.contains("Credits:"))
  #expect(rendered.hasSuffix("openai/gpt-5"))

  let inputIndex = rendered.range(of: "↑10")!.lowerBound
  let outputIndex = rendered.range(of: "↓7")!.lowerBound
  let contextIndex = rendered.range(of: "0…17")!.lowerBound
  let modelIndex = rendered.range(of: "openai/gpt-5")!.lowerBound
  #expect(inputIndex < outputIndex)
  #expect(outputIndex < contextIndex)
  #expect(contextIndex < modelIndex)
}
