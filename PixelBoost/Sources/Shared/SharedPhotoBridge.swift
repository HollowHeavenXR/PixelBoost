import UIKit

/// Hands a photo from the Share Extension (a separate process from the
/// main app) over to PixelBoost proper. There's no in-memory link possible
/// between an extension and its host app, so this is a "drop a file in the
/// shared App Group container, main app picks it up the next time it
/// becomes active" bridge rather than a live callback. Compiled into both
/// the `PixelBoost` and `PixelBoostShare` targets (see `project.yml`), so
/// both sides agree on the container/file name without duplicating the
/// logic.
enum SharedPhotoBridge {
    private static var appGroupID: String { AppGroup.id }
    private static let fileName = "pending-shared-photo.jpg"

    private static var containerURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroupID)
    }

    static func savePendingImage(_ image: UIImage) {
        guard let containerURL, let data = image.jpegData(compressionQuality: 0.92) else { return }
        try? data.write(to: containerURL.appendingPathComponent(fileName))
    }

    /// Call whenever the app becomes active — returns the pending image
    /// (if any) and deletes it, so it's only ever picked up once.
    static func consumePendingImage() -> UIImage? {
        guard let containerURL else { return nil }
        let fileURL = containerURL.appendingPathComponent(fileName)
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        try? FileManager.default.removeItem(at: fileURL)
        return UIImage(data: data)
    }
}
