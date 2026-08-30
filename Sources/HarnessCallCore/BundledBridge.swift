import Foundation

public enum BundledBridgeError: Error, Sendable, Equatable, CustomStringConvertible {
    case runtimeNotFound(String)
    case nodeNotFound

    public var description: String {
        switch self {
        case .runtimeNotFound(let path):
            return "Bundled call-core runtime was not found at \(path)"
        case .nodeNotFound:
            return "Node.js 20 or newer was not found. Set YEET_NODE or add node to PATH."
        }
    }
}

public enum BundledBridge {
    public static func runtimeDirectory() throws -> URL {
        guard let resourceURL = Bundle.module.resourceURL else {
            throw BundledBridgeError.runtimeNotFound("<resource bundle>")
        }
        let runtime = resourceURL.appendingPathComponent("Runtime", isDirectory: true)
        let bridge = runtime.appendingPathComponent("dist/bridge.js", isDirectory: false)
        guard FileManager.default.fileExists(atPath: bridge.path) else {
            throw BundledBridgeError.runtimeNotFound(bridge.path)
        }
        return runtime
    }

    public static func scriptURL() throws -> URL {
        try runtimeDirectory().appendingPathComponent("dist/bridge.js", isDirectory: false)
    }

    /// Returns the bundled JSONL file-edit daemon script. The daemon shares
    /// the runtime resource with the call bridge but is launched separately
    /// because it owns a workspace root and snapshot session.
    public static func editDaemonScriptURL() throws -> URL {
        let script = try runtimeDirectory()
            .appendingPathComponent("dist/edit-backend/daemon.js", isDirectory: false)
        guard FileManager.default.fileExists(atPath: script.path) else {
            throw BundledBridgeError.runtimeNotFound(script.path)
        }
        return script
    }

    public static func nodeExecutable(
        override: String? = nil,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws -> String {
        let fileManager = FileManager.default

        if let override, !override.isEmpty {
            if override.contains("/") {
                guard fileManager.isExecutableFile(atPath: override) else {
                    throw BundledBridgeError.nodeNotFound
                }
                return override
            }
            if let resolved = resolveFromPath(override, environment: environment) {
                return resolved
            }
        }

        if let configured = environment["YEET_NODE"], !configured.isEmpty {
            if configured.contains("/") {
                guard fileManager.isExecutableFile(atPath: configured) else {
                    throw BundledBridgeError.nodeNotFound
                }
                return configured
            }
            if let resolved = resolveFromPath(configured, environment: environment) {
                return resolved
            }
        }

        if let resolved = resolveFromPath("node", environment: environment) {
            return resolved
        }

        for candidate in [
            "/opt/homebrew/bin/node",
            "/usr/local/bin/node",
            "/usr/bin/node"
        ] where fileManager.isExecutableFile(atPath: candidate) {
            return candidate
        }

        throw BundledBridgeError.nodeNotFound
    }

    private static func resolveFromPath(
        _ executable: String,
        environment: [String: String]
    ) -> String? {
        let fileManager = FileManager.default
        let path = environment["PATH"] ?? ""
        for directory in path.split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(directory), isDirectory: true)
                .appendingPathComponent(executable, isDirectory: false)
                .path
            if fileManager.isExecutableFile(atPath: candidate) {
                return candidate
            }
        }
        return nil
    }
}

public extension BridgeConfiguration {
    static func bundled(
        environment: [String: String] = [:],
        nodeExecutable: String? = nil
    ) throws -> Self {
        let runtime = try BundledBridge.runtimeDirectory()
        let node = try BundledBridge.nodeExecutable(override: nodeExecutable)
        return .init(
            command: node,
            arguments: [runtime.appendingPathComponent("dist/bridge.js").path],
            environment: environment,
            inheritEnvironment: true,
            workingDirectory: runtime
        )
    }
}

public extension CallCoreClient {
    static func startBundled(
        environment: [String: String] = [:],
        nodeExecutable: String? = nil
    ) async throws -> CallCoreClient {
        try await start(
            configuration: .bundled(
                environment: environment,
                nodeExecutable: nodeExecutable
            )
        )
    }
}
