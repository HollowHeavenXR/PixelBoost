import Foundation
import Security

/// The App Group container this process was actually signed with.
///
/// Xcode/paid signing keeps the literal ID ("group.com.pixelboost.shared"). Sideloading
/// tools (SideStore, AltStore, Sideloadly, LiveContainer, free Apple IDs)
/// can't reserve that ID, so they register it with the team ID appended
/// ("group.com.pixelboost.shared.<TeamID>"). The app, widget and extensions are re-signed
/// together and get the same suffix, so each side checks the literal ID
/// first and then the suffixed one.
enum AppGroup {
    static let literal = "group.com.pixelboost.shared"

    static let id: String = {
        if FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: literal) != nil {
            return literal
        }
        if let teamID = teamIdentifier() {
            let suffixed = "\(literal).\(teamID)"
            if FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: suffixed) != nil {
                return suffixed
            }
        }
        return literal
    }()

    /// True when either form of the group is granted to this process.
    static var isAvailable: Bool {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: id) != nil
    }

    /// The team ID this build was signed with. A keychain item saved without
    /// an explicit access group is tagged "<TeamID>.<bundleID>", and
    /// kSecAttrAccessGroup is public API.
    private static func teamIdentifier() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrAccount as String: "AppGroupTeamIdentifierProbe",
            kSecReturnAttributes as String: true,
        ]
        var result: AnyObject?
        var status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            let add: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrAccount as String: "AppGroupTeamIdentifierProbe",
                kSecValueData as String: Data(),
            ]
            SecItemAdd(add as CFDictionary, nil)
            status = SecItemCopyMatching(query as CFDictionary, &result)
        }
        guard status == errSecSuccess,
              let attributes = result as? [String: Any],
              let group = attributes[kSecAttrAccessGroup as String] as? String,
              let dot = group.firstIndex(of: ".")
        else { return nil }
        return String(group[..<dot])
    }
}
