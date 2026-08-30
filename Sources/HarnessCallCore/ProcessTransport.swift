@preconcurrency import Foundation

final class ProcessTransport: @unchecked Sendable {
    private let configuration: BridgeConfiguration
    private let process = Process()
    private let stdinPipe = Pipe()
    private let stdoutPipe = Pipe()
    private let stderrPipe = Pipe()
    private let writeLock = NSLock()
    private let stateLock = NSLock()
    private var started = false

    init(configuration: BridgeConfiguration) {
        self.configuration = configuration
    }

    func start(
        onStdout: @escaping @Sendable (Data) -> Void,
        onStderr: @escaping @Sendable (Data) -> Void,
        onExit: @escaping @Sendable (Int32) -> Void
    ) throws {
        stateLock.lock()
        defer { stateLock.unlock() }
        guard !started else { return }

        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = [configuration.command] + configuration.arguments
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
        process.currentDirectoryURL = configuration.workingDirectory

        var environment = configuration.inheritEnvironment ? ProcessInfo.processInfo.environment : [:]
        for (key, value) in configuration.environment { environment[key] = value }
        process.environment = environment

        stdoutPipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if !data.isEmpty { onStdout(data) }
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if !data.isEmpty { onStderr(data) }
        }
        process.terminationHandler = { process in
            onExit(process.terminationStatus)
        }

        do {
            try process.run()
            started = true
        } catch {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            throw CallCoreBridgeError.failedToStart(error.localizedDescription)
        }
    }

    func send(_ data: Data) throws {
        var framed = data
        framed.append(0x0A)

        writeLock.lock()
        defer { writeLock.unlock() }
        do {
            try stdinPipe.fileHandleForWriting.write(contentsOf: framed)
        } catch {
            throw CallCoreBridgeError.transport(error.localizedDescription)
        }
    }

    func stop() {
        stateLock.lock()
        let wasStarted = started
        started = false
        stateLock.unlock()
        guard wasStarted else { return }

        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil
        try? stdinPipe.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
    }
}
