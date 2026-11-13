import Foundation
import Photos
import UniformTypeIdentifiers

/// Puts an upscaled result back where the photo came from in the user's
/// library. PhotoKit calls only: the replacement file itself (pixels +
/// the original's EXIF and colour profile) is built by the Rust core
/// (`prepareReplacement`).
///
/// Two methods:
/// - **edit** (default): saved as an edit of the original asset. Same
///   photo, same place in the library, albums, favourite, date, location;
///   Photos' "Revert" brings the original back.
/// - **swap**: the result is added as a new photo carrying the original's
///   date, location and favourite flag, put into the same albums, and the
///   original is deleted (iOS asks; it stays 30 days in Recently Deleted).
///   Used when the user picks it, or when Photos refuses the edit.
enum PhotoLibrary {
    enum Method: String, CaseIterable, Identifiable {
        case edit, swap
        var id: String { rawValue }
        var title: String { self == .edit ? "Edit in place (revertible)" : "New photo + delete original" }
    }

    enum Outcome {
        case edited, swapped, swappedKeptOriginal
        var message: String {
            switch self {
            case .edited: return "Replaced in Photos — Revert in Photos brings the original back"
            case .swapped: return "Replaced in Photos — original is in Recently Deleted"
            case .swappedKeptOriginal: return "Added next to the original (you kept it)"
            }
        }
    }

    enum Failure: LocalizedError {
        case noAccess, notFound, noInput
        var errorDescription: String? {
            switch self {
            case .noAccess: return "PixelBoost needs access to your photo library to replace the original. Allow it in Settings › PixelBoost › Photos."
            case .notFound: return "The original photo isn't available to PixelBoost (it may have been deleted, or limited access doesn't include it)."
            case .noInput: return "Photos couldn't open the original for editing."
            }
        }
    }

    static var method: Method {
        Method(rawValue: UserDefaults.standard.string(forKey: "pb.replaceMethod") ?? "") ?? .edit
    }

    /// Replace the library photo `assetID` with `result`.
    static func replace(assetID: String, with result: URL, note: String) async throws -> Outcome {
        let status = await PHPhotoLibrary.requestAuthorization(for: .readWrite)
        guard status == .authorized || status == .limited else { throw Failure.noAccess }
        guard let asset = PHAsset.fetchAssets(withLocalIdentifiers: [assetID], options: nil).firstObject else { throw Failure.notFound }
        let input = try await editingInput(asset)
        guard let original = input.fullSizeImageURL else { throw Failure.noInput }

        if method == .edit {
            do {
                try await saveAsEdit(asset: asset, input: input, original: original, result: result, note: note)
                return .edited
            } catch {
                // Photos rejects some edits (e.g. shared-library or
                // unusual originals): fall back to swapping.
            }
        }
        return try await swap(asset: asset, original: original, result: result)
    }

    private static func editingInput(_ asset: PHAsset) async throws -> PHContentEditingInput {
        let opts = PHContentEditingInputRequestOptions()
        opts.isNetworkAccessAllowed = true
        opts.canHandleAdjustmentData = { _ in false }
        return try await withCheckedThrowingContinuation { cont in
            asset.requestContentEditingInput(with: opts) { input, info in
                if let input {
                    cont.resume(returning: input)
                } else {
                    cont.resume(throwing: (info[PHContentEditingInputErrorKey] as? Error) ?? Failure.noInput)
                }
            }
        }
    }

    private static func saveAsEdit(asset: PHAsset, input: PHContentEditingInput, original: URL, result: URL, note: String) async throws {
        let base = Core.work.appendingPathComponent(UUID().uuidString)
        let rep = try await Task.detached(priority: .userInitiated) {
            try Core.engine.prepareReplacement(originalPath: original.path, resultPath: result.path, outputBase: base.path, quality: 95, allowPng: false)
        }.value
        defer { try? FileManager.default.removeItem(atPath: rep.path) }
        let output = PHContentEditingOutput(contentEditingInput: input)
        output.adjustmentData = PHAdjustmentData(formatIdentifier: "com.pixelboost.upscale", formatVersion: "1", data: Data(note.utf8))
        let dest = try output.renderedContentURL(for: .jpeg)
        try? FileManager.default.removeItem(at: dest)
        try FileManager.default.copyItem(at: URL(fileURLWithPath: rep.path), to: dest)
        try await PHPhotoLibrary.shared().performChanges {
            PHAssetChangeRequest(for: asset).contentEditingOutput = output
        }
    }

    private static func swap(asset: PHAsset, original: URL, result: URL) async throws -> Outcome {
        let base = Core.work.appendingPathComponent(UUID().uuidString)
        let rep = try await Task.detached(priority: .userInitiated) {
            try Core.engine.prepareReplacement(originalPath: original.path, resultPath: result.path, outputBase: base.path, quality: 95, allowPng: true)
        }.value
        defer { try? FileManager.default.removeItem(atPath: rep.path) }
        // Albums the original is in (user albums only; smart albums are
        // computed by Photos).
        let albums = PHAssetCollection.fetchAssetCollectionsContaining(asset, with: .album, options: nil)
        var collections: [PHAssetCollection] = []
        albums.enumerateObjects { c, _, _ in if c.canPerform(.addContent) { collections.append(c) } }

        let fileURL = URL(fileURLWithPath: rep.path)
        try await PHPhotoLibrary.shared().performChanges {
            let req = PHAssetCreationRequest.forAsset()
            let opts = PHAssetResourceCreationOptions()
            opts.originalFilename = PHAssetResource.assetResources(for: asset).first?.originalFilename
            req.addResource(with: .photo, fileURL: fileURL, options: opts)
            req.creationDate = asset.creationDate
            req.location = asset.location
            req.isFavorite = asset.isFavorite
            if let ph = req.placeholderForCreatedAsset {
                for c in collections {
                    PHAssetCollectionChangeRequest(for: c)?.addAssets([ph] as NSArray)
                }
            }
        }
        // Separate step so declining the delete keeps the new photo.
        do {
            try await PHPhotoLibrary.shared().performChanges {
                PHAssetChangeRequest.deleteAssets([asset] as NSArray)
            }
            return .swapped
        } catch {
            return .swappedKeptOriginal
        }
    }
}
