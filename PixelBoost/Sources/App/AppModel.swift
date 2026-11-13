import SwiftUI
import UIKit

/// App-wide state: the photo being worked on (as a file), its edit history
/// for undo/redo, and a display-size preview. Pixels live in Rust; this
/// only tracks file URLs.
@MainActor
final class AppModel: ObservableObject {
    enum Mode: Hashable { case upscale, edit, cloud }

    @Published var mode: Mode = .upscale
    @Published var showSettings = false
    /// Short confirmation shown over the canvas ("Sent to Cloud").
    @Published var toast: String?
    /// Current working image file.
    @Published private(set) var current: URL?
    @Published private(set) var preview: UIImage?
    @Published private(set) var info: ImageInfo?
    @Published var message: String?
    @Published var showExport = false
    @Published var showHistory = false

    /// Every version of the photo: timeline[0] is the import, step i made
    /// timeline[i + 1]. `index` is the version on screen.
    private var timeline: [URL] = []
    private var stepList: [EditStep] = []
    @Published private(set) var index = 0
    /// The photo as first imported (Cutout touch-up, compare).
    private(set) var original: URL?
    /// Photo-library asset the photo came from (picker imports), so a
    /// result can replace it in the user's library.
    @Published private(set) var sourceAsset: String?
    /// Display-size preview of the original (press-and-hold compare).
    @Published private(set) var originalPreview: UIImage?

    let models: [ModelEntry] = Core.engine.models()

    var canUndo: Bool { index > 0 }
    var canRedo: Bool { index < timeline.count - 1 }
    /// Edits that made the current version (oldest first).
    var steps: [EditStep] { Array(stepList.prefix(index)) }
    /// Undone edits that redo would bring back.
    var undone: [EditStep] { Array(stepList.dropFirst(index)) }

    func setPhoto(_ url: URL, asset: String? = nil) {
        sourceAsset = asset
        timeline = [url]
        stepList = []
        index = 0
        original = url
        originalPreview = nil
        show(url)
        Task.detached(priority: .utility) {
            let img = (try? Core.engine.loadPreview(path: url.path, maxSide: 2048))?.uiImage
            await MainActor.run { if self.original == url { self.originalPreview = img } }
        }
    }

    /// Replace the working image with an edited version (undoable). `op`
    /// is the edit's JSON when it can be replayed (copy / presets).
    func commit(_ url: URL, step: String = "Edit", op: String? = nil) {
        if timeline.isEmpty {
            setPhoto(url)
            return
        }
        timeline.removeSubrange((index + 1)...)
        stepList.removeSubrange(index...)
        timeline.append(url)
        stepList.append(EditStep(title: step, op: op))
        index = timeline.count - 1
        if timeline.count > 61 {
            timeline.removeFirst()
            stepList.removeFirst()
            index -= 1
        }
        show(url)
    }

    func undo() { jump(to: index - 1) }
    func redo() { jump(to: index + 1) }

    /// Show any version from the history.
    func jump(to i: Int) {
        guard i >= 0, i < timeline.count, i != index else { return }
        index = i
        show(timeline[i])
    }

    /// The replayable edits so far as one chain op (nil when none).
    var recipe: String? {
        let ops = steps.compactMap { $0.op }.filter { (try? Core.engine.opInfo(opJson: $0))?.portable == true }
        return ops.isEmpty ? nil : "{\"op\":\"chain\",\"ops\":[" + ops.joined(separator: ",") + "]}"
    }

    /// Copied edits, kept across photos and launches.
    var copiedEdits: String? {
        get { UserDefaults.standard.string(forKey: "pb.copiedEdits") }
        set { UserDefaults.standard.set(newValue, forKey: "pb.copiedEdits"); objectWillChange.send() }
    }

    func copyEdits() {
        guard let r = recipe else { message = "No edits that can be copied yet (painted masks, layers and LUTs stay with this photo)."; return }
        copiedEdits = r
        flash("Edits copied")
    }

    /// Run an op on the full-size current image and commit it.
    func run(_ op: String, step: String) {
        guard let src = current else { return }
        let out = Core.newFile("png")
        busy = true
        Task.detached(priority: .userInitiated) {
            let r = Result { try Core.engine.editFile(inputPath: src.path, outputPath: out.path, opJson: op) }
            await MainActor.run {
                self.busy = false
                switch r {
                case .success: self.commit(out, step: step, op: op)
                case .failure(let e): self.message = describe(e)
                }
            }
        }
    }

    @Published var busy = false

    private func show(_ url: URL) {
        current = url
        let path = url.path
        Task.detached(priority: .userInitiated) {
            let info = try? Core.engine.imageInfo(path: path)
            let img = (try? Core.engine.loadPreview(path: path, maxSide: 2048))?.uiImage
            await MainActor.run {
                guard self.current == url else { return }
                self.info = info
                self.preview = img
            }
        }
    }

    func importData(_ data: Data) {
        do { setPhoto(try PhotoIO.importData(data)) } catch { message = describe(error) }
    }

    func importFile(_ url: URL) {
        do { setPhoto(try PhotoIO.importURL(url)) } catch { message = describe(error) }
    }

    func pasteFromClipboard() {
        if let img = UIPasteboard.general.image, let data = img.pngData() {
            importData(data)
        } else {
            message = "There's no image on the clipboard."
        }
    }

    func pickUpSharedPhoto() {
        if let img = SharedPhotoBridge.consumePendingImage(), let data = img.pngData() {
            importData(data)
            mode = .upscale
        }
    }

    /// Put `result` back in place of the photo it came from.
    func replaceOriginal(with result: URL, model: String, scale: Float) {
        guard let asset = sourceAsset else { return }
        let note = Core.engine.replacementNote(model: model, scale: scale, inWidth: info?.width ?? 0, inHeight: info?.height ?? 0)
        replace(asset: asset, with: result, note: note)
    }

    func replace(asset: String, with result: URL, note: String) {
        busy = true
        Task {
            do {
                let o = try await PhotoLibrary.replace(assetID: asset, with: result, note: note)
                flash(o.message)
                log("replace_original", ["method": PhotoLibrary.method.rawValue])
            } catch {
                message = describe(error)
            }
            busy = false
        }
    }

    func flash(_ text: String) {
        toast = text
        Task {
            try? await Task.sleep(nanoseconds: 2_600_000_000)
            if toast == text { toast = nil }
        }
    }

    func log(_ action: String, _ detail: [String: Any] = [:]) {
        guard Prefs.telemetry else { return }
        let d = (try? JSONSerialization.data(withJSONObject: detail)).map { String(decoding: $0, as: UTF8.self) } ?? "{}"
        Core.server.logAction(action: action, detailJson: d)
    }
}

struct EditStep: Identifiable, Equatable {
    let id = UUID()
    let title: String
    let op: String?
}
