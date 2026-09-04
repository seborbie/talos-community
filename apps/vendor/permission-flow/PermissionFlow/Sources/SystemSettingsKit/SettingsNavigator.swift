#if os(macOS)
import AppKit

@available(macOS 13.0, *)
@MainActor
final class SettingsNavigator {
    private let bundleIdentifier = "com.apple.systempreferences"
    private let applicationURL = URL(fileURLWithPath: "/System/Applications/System Settings.app")

    /// Opens System Settings with a generic deeplink URL.
    @discardableResult
    func openSettings(at url: URL) -> Bool {
        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_entered url=\(url.absoluteString)")
        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_before_openApplication")
        NSWorkspace.shared.openApplication(
            at: applicationURL,
            configuration: NSWorkspace.OpenConfiguration()
        ) { application, error in
            TalosPermissionFlowDebugLog.write(
                "SettingsNavigator_openApplication_callback application=\(application?.bundleIdentifier ?? "nil") error=\(String(describing: error))"
            )
        }

        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_before_open_url")
        let didOpen = NSWorkspace.shared.open(url)
        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_after_open_url didOpen=\(didOpen)")
        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_before_activate")
        activateSettings()
        TalosPermissionFlowDebugLog.write("SettingsNavigator_openSettings_completed didOpen=\(didOpen)")
        return didOpen
    }

    /// Re-activates the running System Settings process if it already exists.
    func activateSettings() {
        TalosPermissionFlowDebugLog.write("SettingsNavigator_activateSettings_entered")
        NSRunningApplication.runningApplications(withBundleIdentifier: bundleIdentifier)
            .first?
            .activate(options: [.activateIgnoringOtherApps])
        TalosPermissionFlowDebugLog.write("SettingsNavigator_activateSettings_completed")
    }
}
#elseif os(iOS)
import UIKit

@available(iOS 16.0, *)
@MainActor
final class SettingsNavigator {
    /// Opens the destination URL through UIKit. iOS support is intentionally
    /// limited to URLs that the platform publicly allows.
    @discardableResult
    func openSettings(at url: URL) -> Bool {
        guard UIApplication.shared.canOpenURL(url) else { return false }
        UIApplication.shared.open(url)
        return true
    }
}
#endif
