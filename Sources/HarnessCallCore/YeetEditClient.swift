import Foundation

public struct YeetReadResult: Codable, Sendable {
    public let path: String
    public let snapshot: String
    public let startLine: Int
    public let endLine: Int
    public let totalLines: Int
    public let content: String
    public let numbered: String
}

public struct YeetLineRange: Codable, Sendable {
    public let start: Int
    public let end: Int
    public init(start: Int, end: Int) {
        self.start = start
        self.end = end
    }
}

public enum YeetEdit: Codable, Sendable {
    case replace(range: YeetLineRange, text: String)
    case delete(range: YeetLineRange)
    case insertStart(text: String)
    case insertEnd(text: String)
    case insertBefore(line: Int, text: String)
    case insertAfter(line: Int, text: String)
    case replaceBlock(line: Int, text: String)
    case insertAfterBlock(line: Int, text: String)
    case deleteBlock(line: Int)

    private enum CodingKeys: String, CodingKey { case kind, range, text, at, line }
    private struct At: Codable { let kind: String; let line: Int? }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .replace(range, text):
            try c.encode("replace", forKey: .kind)
            try c.encode(range, forKey: .range)
            try c.encode(text, forKey: .text)
        case let .delete(range):
            try c.encode("delete", forKey: .kind)
            try c.encode(range, forKey: .range)
        case let .insertStart(text):
            try c.encode("insert", forKey: .kind)
            try c.encode(At(kind: "start", line: nil), forKey: .at)
            try c.encode(text, forKey: .text)
        case let .insertEnd(text):
            try c.encode("insert", forKey: .kind)
            try c.encode(At(kind: "end", line: nil), forKey: .at)
            try c.encode(text, forKey: .text)
        case let .insertBefore(line, text):
            try c.encode("insert", forKey: .kind)
            try c.encode(At(kind: "before", line: line), forKey: .at)
            try c.encode(text, forKey: .text)
        case let .insertAfter(line, text):
            try c.encode("insert", forKey: .kind)
            try c.encode(At(kind: "after", line: line), forKey: .at)
            try c.encode(text, forKey: .text)
        case let .replaceBlock(line, text):
            try c.encode("replaceBlock", forKey: .kind)
            try c.encode(line, forKey: .line)
            try c.encode(text, forKey: .text)
        case let .insertAfterBlock(line, text):
            try c.encode("insertAfterBlock", forKey: .kind)
            try c.encode(line, forKey: .line)
            try c.encode(text, forKey: .text)
        case let .deleteBlock(line):
            try c.encode("deleteBlock", forKey: .kind)
            try c.encode(line, forKey: .line)
        }
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        if kind == "replace" {
            self = .replace(range: try c.decode(YeetLineRange.self, forKey: .range), text: try c.decode(String.self, forKey: .text))
        } else if kind == "delete" {
            self = .delete(range: try c.decode(YeetLineRange.self, forKey: .range))
        } else if kind == "replaceBlock" {
            self = .replaceBlock(line: try c.decode(Int.self, forKey: .line), text: try c.decode(String.self, forKey: .text))
        } else if kind == "insertAfterBlock" {
            self = .insertAfterBlock(line: try c.decode(Int.self, forKey: .line), text: try c.decode(String.self, forKey: .text))
        } else if kind == "deleteBlock" {
            self = .deleteBlock(line: try c.decode(Int.self, forKey: .line))
        } else {
            let at = try c.decode(At.self, forKey: .at)
            let text = try c.decode(String.self, forKey: .text)
            switch at.kind {
            case "start": self = .insertStart(text: text)
            case "end": self = .insertEnd(text: text)
            case "before": self = .insertBefore(line: at.line ?? 1, text: text)
            case "after": self = .insertAfter(line: at.line ?? 1, text: text)
            default:
                throw DecodingError.dataCorruptedError(
                    forKey: .at,
                    in: c,
                    debugDescription: "Unknown insertion point: \(at.kind)"
                )
            }
        }
    }
}

public enum YeetFileOperation: Codable, Sendable {
    case create(text: String, mode: Int?)
    case delete
    case move(destination: String)

    private enum CodingKeys: String, CodingKey { case kind, text, mode, destination }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .create(text, mode):
            try c.encode("create", forKey: .kind)
            try c.encode(text, forKey: .text)
            try c.encodeIfPresent(mode, forKey: .mode)
        case .delete:
            try c.encode("delete", forKey: .kind)
        case let .move(destination):
            try c.encode("move", forKey: .kind)
            try c.encode(destination, forKey: .destination)
        }
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "create":
            self = .create(
                text: try c.decode(String.self, forKey: .text),
                mode: try c.decodeIfPresent(Int.self, forKey: .mode)
            )
        case "delete":
            self = .delete
        case "move":
            self = .move(destination: try c.decode(String.self, forKey: .destination))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .kind,
                in: c,
                debugDescription: "Unknown file operation"
            )
        }
    }
}

public struct YeetFileChange: Codable, Sendable {
    public let path: String
    public let snapshot: String?
    public let edits: [YeetEdit]
    public let fileOp: YeetFileOperation?

    private enum CodingKeys: String, CodingKey { case path, snapshot, edits, fileOp }

    public init(
        path: String,
        snapshot: String? = nil,
        edits: [YeetEdit] = [],
        fileOp: YeetFileOperation? = nil
    ) {
        self.path = path
        self.snapshot = snapshot
        self.edits = edits
        self.fileOp = fileOp
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.path = try c.decode(String.self, forKey: .path)
        self.snapshot = try c.decodeIfPresent(String.self, forKey: .snapshot)
        self.edits = try c.decodeIfPresent([YeetEdit].self, forKey: .edits) ?? []
        self.fileOp = try c.decodeIfPresent(YeetFileOperation.self, forKey: .fileOp)
    }
}

public struct YeetApplyRequest: Codable, Sendable {
    public let changes: [YeetFileChange]
    public let diagnostics: Bool?
    public init(changes: [YeetFileChange], diagnostics: Bool? = nil) {
        self.changes = changes
        self.diagnostics = diagnostics
    }
}

public struct YeetDiagnostic: Codable, Sendable {
    public let path: String
    public let severity: String
    public let message: String
    public let line: Int?
    public let column: Int?
    public let source: String?
}

public struct YeetFileApplyResult: Codable, Sendable {
    public let path: String
    public let destination: String?
    public let operation: String
    public let snapshot: String?
    public let warnings: [String]
}

public struct YeetApplyResult: Codable, Sendable {
    public let files: [YeetFileApplyResult]
    public let diagnostics: [YeetDiagnostic]
}

private struct RPCRequest<Params: Encodable>: Encodable {
    let id: Int
    let method: String
    let params: Params
}

private struct RPCError: Codable {
    let message: String
}

private struct RPCResponse<Result: Decodable>: Decodable {
    let id: Int?
    let result: Result?
    let error: RPCError?
}

public enum YeetEditClientError: Error, LocalizedError {
    case daemon(String)
    case protocolError
    case processExited

    public var errorDescription: String? {
        switch self {
        case let .daemon(message): message
        case .protocolError: "Invalid response from yeet-editd"
        case .processExited: "yeet-editd exited"
        }
    }
}

public actor YeetEditClient {
    private let process: Process
    // Keep the Pipe objects alive for the lifetime of the daemon. Retaining
    // only their file handles can leave Foundation's pipe bookkeeping without
    // a live owner, which prevents a long-lived child from observing writes
    // until EOF.
    private let stdinPipe: Pipe
    private let stdoutPipe: Pipe
    private let input: FileHandle
    private let output: FileHandle
    private var readBuffer = Data()
    private var nextID = 1

    public init(executable: URL, root: URL, environment: [String: String] = [:]) throws {
        try self.init(
            command: executable,
            arguments: ["--root", root.path],
            environment: environment
        )
    }

    /// Launches the daemon through a Node executable. This is useful with
    /// `BundledBridge.editDaemonScriptURL()` when no globally installed
    /// `yeet-editd` binary is available.
    public init(
        nodeExecutable: URL,
        script: URL,
        root: URL,
        environment: [String: String] = [:]
    ) throws {
        try self.init(
            command: nodeExecutable,
            arguments: [script.path, "--root", root.path],
            environment: environment
        )
    }

    private init(command: URL, arguments: [String], environment: [String: String]) throws {
        let process = Process()
        let stdinPipe = Pipe()
        let stdoutPipe = Pipe()
        process.executableURL = command
        var mergedEnvironment = ProcessInfo.processInfo.environment
        for (key, value) in environment { mergedEnvironment[key] = value }
        process.environment = mergedEnvironment
        process.arguments = arguments
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = FileHandle.standardError
        try process.run()
        self.process = process
        self.stdinPipe = stdinPipe
        self.stdoutPipe = stdoutPipe
        self.input = stdinPipe.fileHandleForWriting
        self.output = stdoutPipe.fileHandleForReading
    }

    deinit {
        process.terminate()
        try? input.close()
        try? output.close()
    }

    public func read(path: String, startLine: Int? = nil, endLine: Int? = nil) throws -> YeetReadResult {
        struct Params: Codable { let path: String; let startLine: Int?; let endLine: Int? }
        return try request(method: "read", params: Params(path: path, startLine: startLine, endLine: endLine))
    }

    public func apply(_ apply: YeetApplyRequest) throws -> YeetApplyResult {
        try request(method: "apply", params: apply)
    }

    public func applyDialect(
        input text: String,
        dialect: String? = nil,
        model: String? = nil,
        snapshots: [String: String]? = nil
    ) throws -> YeetApplyResult {
        struct Params: Codable {
            let input: String
            let dialect: String?
            let model: String?
            let snapshots: [String: String]?
        }
        return try request(method: "applyDialect", params: Params(input: text, dialect: dialect, model: model, snapshots: snapshots))
    }

    private func request<Params: Encodable, Result: Decodable>(method: String, params: Params) throws -> Result {
        guard process.isRunning else { throw YeetEditClientError.processExited }
        let id = nextID
        nextID += 1
        var data = try JSONEncoder().encode(RPCRequest(id: id, method: method, params: params))
        data.append(0x0A)
        try input.write(contentsOf: data)
        let line = try readLine()
        let response = try JSONDecoder().decode(RPCResponse<Result>.self, from: line)
        if let error = response.error { throw YeetEditClientError.daemon(error.message) }
        guard response.id == id, let result = response.result else { throw YeetEditClientError.protocolError }
        return result
    }

    private func readLine() throws -> Data {
        while true {
            if let newline = readBuffer.firstIndex(of: 0x0A) {
                let line = readBuffer[..<newline]
                readBuffer.removeSubrange(...newline)
                return Data(line)
            }
            // `read(upToCount:)` can wait for the entire requested size on
            // macOS pipes. `availableData` returns as soon as the daemon has
            // emitted a response, which is the framing we need here.
            let chunk = output.availableData
            if chunk.isEmpty { throw YeetEditClientError.processExited }
            readBuffer.append(chunk)
        }
    }
}

public extension YeetEditClient {
    /// Starts the file-edit daemon from the SwiftPM-bundled runtime.
    static func startBundled(
        root: URL,
        environment: [String: String] = [:],
        nodeExecutable: String? = nil
    ) throws -> YeetEditClient {
        let script = try BundledBridge.editDaemonScriptURL()
        let node = try BundledBridge.nodeExecutable(override: nodeExecutable)
        return try YeetEditClient(
            nodeExecutable: URL(fileURLWithPath: node),
            script: script,
            root: root,
            environment: environment
        )
    }
}
