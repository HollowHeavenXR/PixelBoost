import Foundation
import ImageIO
import Photos
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// Platform I/O glue: getting photos in (HEIC etc. decoded by ImageIO with
/// orientation applied) and results out to the photo library. No pixel
/// processing happens here.
enum PhotoIO {
    /// Write image data (any format iOS can read) as an upright PNG file the
    /// Rust core can open. Formats Rust reads natively keep their bytes.
    static func importData(_ data: Data, nameHint: String? = nil) throws -> URL {
        if let ext = nativeExtension(data), orientation(of: data) <= 1 {
            let url = Core.newFile(ext)
            try data.write(to: url)
            return url
        }
        guard let src = CGImageSourceCreateWithData(data as CFData, nil) else { throw ImportError.unreadable }
        let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any]
        let w = props?[kCGImagePropertyPixelWidth] as? Int ?? 8192
        let h = props?[kCGImagePropertyPixelHeight] as? Int ?? 8192
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: max(w, h),
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { throw ImportError.unreadable }
        let url = Core.newFile("png")
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { throw ImportError.unreadable }
        CGImageDestinationAddImage(dest, cg, nil)
        guard CGImageDestinationFinalize(dest) else { throw ImportError.unreadable }
        return url
    }

    static func importURL(_ url: URL) throws -> URL {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        return try importData(try Data(contentsOf: url), nameHint: url.lastPathComponent)
    }

    static func importItem(_ item: PhotosPickerItem) async throws -> URL {
        guard let data = try await item.loadTransferable(type: Data.self) else { throw ImportError.unreadable }
        return try importData(data)
    }

    private static func nativeExtension(_ d: Data) -> String? {
        let b = [UInt8](d.prefix(12))
        if b.count >= 8, b[0] == 0x89, b[1] == 0x50 { return "png" }
        if b.count >= 3, b[0] == 0xFF, b[1] == 0xD8 { return "jpg" }
        if b.count >= 12, b[0] == 0x52, b[1] == 0x49, b[8] == 0x57, b[9] == 0x45 { return "webp" }
        if b.count >= 4, b[0] == 0x76, b[1] == 0x2F, b[2] == 0x31, b[3] == 0x01 { return "exr" }
        return nil
    }

    private static func orientation(of d: Data) -> Int {
        guard let src = CGImageSourceCreateWithData(d as CFData, nil),
              let p = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any] else { return 1 }
        return p[kCGImagePropertyOrientation] as? Int ?? 1
    }

    static func saveToPhotos(_ url: URL) async throws {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else { throw ImportError.noPhotoAccess }
        try await PHPhotoLibrary.shared().performChanges {
            PHAssetCreationRequest.forAsset().addResource(with: .photo, fileURL: url, options: nil)
        }
    }

    enum ImportError: LocalizedError {
        case unreadable, noPhotoAccess
        var errorDescription: String? {
            switch self {
            case .unreadable: return "That image couldn't be read."
            case .noPhotoAccess: return "Photo library access is off — allow it in Settings to save."
            }
        }
    }
}
