#if os(macOS)
import Foundation

@available(macOS 13.0, *)
enum TalosPermissionFlowDebugLog {
    private static let envName = "TALOS_PERMISSION_HELPER_LOG_PATH"

    static func write(
        _ message: String,
        file: StaticString = #fileID,
        function: StaticString = #function,
        line: UInt = #line
    ) {
        guard let path = ProcessInfo.processInfo.environment[envName], path.isEmpty == false else {
            return
        }

        let url = URL(fileURLWithPath: path)
        let timestamp = ISO8601DateFormatter().string(from: Date())
        let text = "\(timestamp) TRACE permission_flow_native file=\(file) function=\(function) line=\(line) \(message)\n"
        guard let data = text.data(using: .utf8) else { return }

        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            if FileManager.default.fileExists(atPath: url.path) == false {
                FileManager.default.createFile(atPath: url.path, contents: nil)
            }
            let handle = try FileHandle(forWritingTo: url)
            defer {
                try? handle.close()
            }
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
            try handle.synchronize()
        } catch {
            fputs("permission_flow_native_log_failed \(error)\n", stderr)
        }
    }
}
#endif
