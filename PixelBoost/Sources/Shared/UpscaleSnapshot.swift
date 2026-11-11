import UIKit

/// A small "what to show on the Home Screen widget" snapshot, written to
/// the shared App Group container by the main app after every successful
/// upscale and read by the `PixelBoostWidgets` extension — same
/// no-in-memory-link reasoning as `SharedPhotoBridge`, just app-to-widget
/// instead of extension-to-app. Compiled into both the `PixelBoost` and
/// `PixelBoostWidgets` targets (see `project.yml`).
struct UpscaleSnapshot: Codable {
    var totalUpscales: Int
    var lastUpdated: Date

    private static var appGroupID: String { AppGroup.id }
    private static let jsonFileName = "widget-snapshot.json"
    private static let thumbnailFileName = "widget-last-result.jpg"

    private static var containerURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroupID)
    }

    static func load() -> UpscaleSnapshot? {
        guard let containerURL else { return nil }
        let url = containerURL.appendingPathComponent(jsonFileName)
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(UpscaleSnapshot.self, from: data)
    }

    static func loadLastResultThumbnail() -> UIImage? {
        guard let containerURL else { return nil }
        let url = containerURL.appendingPathComponent(thumbnailFileName)
        guard let data = try? Data(contentsOf: url) else { return nil }
        return UIImage(data: data)
    }

    /// Called by the main app only — the widget extension is read-only.
    /// `totalUpscales` is a running count kept in this same file rather
    /// than re-derived from `UpscaleStatsService` (server-backed, optional,
    /// often unconfigured) so the widget has something to show even with
    /// no server set up at all.
    static func record(resultThumbnail: UIImage) {
        guard let containerURL else { return }
        let current = load()
        let snapshot = UpscaleSnapshot(totalUpscales: (current?.totalUpscales ?? 0) + 1, lastUpdated: Date())
        if let jsonData = try? JSONEncoder().encode(snapshot) {
            try? jsonData.write(to: containerURL.appendingPathComponent(jsonFileName))
        }
        if let thumbData = resultThumbnail.jpegData(compressionQuality: 0.85) {
            try? thumbData.write(to: containerURL.appendingPathComponent(thumbnailFileName))
        }
    }
}
