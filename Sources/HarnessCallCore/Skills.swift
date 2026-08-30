import Foundation

public struct SkillSummary: Sendable, Equatable, Codable {
    public var name: String
    public var description: String
    public var root: String
    public var entrypoint: String

    public init(name: String, description: String, root: String, entrypoint: String) {
        self.name = name
        self.description = description
        self.root = root
        self.entrypoint = entrypoint
    }
}

public struct Skill: Sendable, Equatable, Codable {
    public var name: String
    public var description: String
    public var root: String
    public var entrypoint: String
    public var instructions: String
    public var files: [String]

    public init(
        name: String,
        description: String,
        root: String,
        entrypoint: String,
        instructions: String,
        files: [String]
    ) {
        self.name = name
        self.description = description
        self.root = root
        self.entrypoint = entrypoint
        self.instructions = instructions
        self.files = files
    }
}
