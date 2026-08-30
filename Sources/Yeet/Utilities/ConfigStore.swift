import Foundation

struct ConfigStore {
    let directory: URL

    init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        if let override = environment["YEET_CONFIG_DIR"], !override.isEmpty {
            self.directory = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            self.directory = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".yeet", isDirectory: true)
        }
    }

    var configURL: URL {
        directory.appendingPathComponent("config.json", isDirectory: false)
    }

    func ensure() throws {
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)

        if !FileManager.default.fileExists(atPath: configURL.path) {
            try writeObject(["version": 1])
        } else {
            try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configURL.path)
        }
    }

    func model() throws -> String? {
        try ensure()
        return try readObject()["model"] as? String
    }

    func setModel(_ value: String) throws {
        let model = try Self.validateModelID(value)
        try ensure()
        var object = try readObject()
        object["version"] = object["version"] ?? 1
        object["model"] = model
        try writeObject(object)
    }

    static func validateModelID(_ value: String) throws -> String {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let slash = trimmed.firstIndex(of: "/"),
              slash != trimmed.startIndex,
              trimmed.index(after: slash) != trimmed.endIndex else {
            throw ConfigError.invalidModelID(value)
        }
        let provider = trimmed[..<slash]
        guard !provider.contains(where: { $0.isWhitespace }) else {
            throw ConfigError.invalidModelID(value)
        }
        return trimmed
    }

    private func readObject() throws -> [String: Any] {
        guard FileManager.default.fileExists(atPath: configURL.path) else {
            return ["version": 1]
        }
        let data = try Data(contentsOf: configURL)
        let value = try JSONSerialization.jsonObject(with: data)
        return value as? [String: Any] ?? ["version": 1]
    }

    private func writeObject(_ object: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        var framed = data
        framed.append(0x0A)
        try framed.write(to: configURL, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configURL.path)
    }
}

enum ConfigError: Error, CustomStringConvertible {
    case invalidModelID(String)
    case missingModel

    var description: String {
        switch self {
        case .invalidModelID(let value):
            return "Invalid model id: \(value). Use provider/model."
        case .missingModel:
            return "No model is configured. Run: yeet model set provider/model"
        }
    }
}
