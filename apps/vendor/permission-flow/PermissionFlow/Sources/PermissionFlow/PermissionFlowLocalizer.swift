#if os(macOS)
import Foundation

@available(macOS 13.0, *)
enum PermissionFlowResourceBundle {
    private static let bundleDirectoryName = "PermissionFlow_PermissionFlow.bundle"

    static let bundle: Bundle? = {
        let candidates = resourceBundleCandidates()
        for url in candidates {
            if FileManager.default.fileExists(atPath: url.path), let bundle = Bundle(url: url) {
                TalosPermissionFlowDebugLog.write("PermissionFlowResourceBundle_resolved path=\(url.path)")
                return bundle
            }
        }

        TalosPermissionFlowDebugLog.write(
            "PermissionFlowResourceBundle_missing candidates=\(candidates.map { $0.path }.joined(separator: ","))"
        )
        return nil
    }()

    static var localizations: [String] {
        bundle?.localizations ?? ["en"]
    }

    static func path(forResource name: String?, ofType type: String?) -> String? {
        bundle?.path(forResource: name, ofType: type)
    }

    static func localizedString(
        forKey key: String,
        value defaultValue: String,
        table: String? = nil
    ) -> String {
        bundle?.localizedString(forKey: key, value: defaultValue, table: table) ?? defaultValue
    }

    private static func resourceBundleCandidates() -> [URL] {
        var candidates: [URL] = []

        if let resourceURL = Bundle.main.resourceURL {
            candidates.append(resourceURL.appendingPathComponent(bundleDirectoryName))
        }

        candidates.append(
            Bundle.main.bundleURL
                .appendingPathComponent("Contents")
                .appendingPathComponent("Resources")
                .appendingPathComponent(bundleDirectoryName)
        )

        candidates.append(Bundle.main.bundleURL.appendingPathComponent(bundleDirectoryName))

        if let executableURL = Bundle.main.executableURL {
            candidates.append(
                executableURL
                    .deletingLastPathComponent()
                    .deletingLastPathComponent()
                    .appendingPathComponent("Resources")
                    .appendingPathComponent(bundleDirectoryName)
            )
        }

        return unique(candidates)
    }

    private static func unique(_ urls: [URL]) -> [URL] {
        var seen = Set<String>()
        return urls.filter { url in
            seen.insert(url.standardizedFileURL.path).inserted
        }
    }
}

@available(macOS 13.0, *)
enum PermissionFlowLocalizer {
    /// Resolves a localized string from the best matching `.lproj` bundle for
    /// the injected locale. This keeps all custom locale selection in one
    /// place, while still letting the rest of the UI use plain localization
    /// keys and format strings.
    static func string(
        _ key: String,
        defaultValue: String,
        localeIdentifier: String?
    ) -> String {
        localizedBundle(for: localeIdentifier)?
            .localizedString(forKey: key, value: defaultValue, table: nil)
            ?? PermissionFlowResourceBundle.localizedString(forKey: key, value: defaultValue)
    }

    private static func localizedBundle(for localeIdentifier: String?) -> Bundle? {
        guard let localeIdentifier, localeIdentifier.isEmpty == false else {
            return nil
        }

        let preferences = localizationPreferences(for: localeIdentifier)
        guard let localization = Bundle.preferredLocalizations(
            from: PermissionFlowResourceBundle.localizations,
            forPreferences: preferences
        ).first,
        let path = PermissionFlowResourceBundle.path(forResource: localization, ofType: "lproj") else {
            return nil
        }

        return Bundle(path: path)
    }

    private static func localizationPreferences(for localeIdentifier: String) -> [String] {
        let normalizedIdentifier = localeIdentifier.replacingOccurrences(of: "_", with: "-")
        let locale = Locale(identifier: normalizedIdentifier)

        var preferences = [normalizedIdentifier]
        if let identifier = locale.language.languageCode?.identifier {
            if let script = locale.language.script?.identifier {
                preferences.append("\(identifier)-\(script)")
            }
            preferences.append(identifier)
        }

        return Array(NSOrderedSet(array: preferences)) as? [String] ?? preferences
    }
}
#endif
