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

    private var undoStack: [URL] = []
    private var redoStack: [URL] = []
    /// The photo as first imported (Cutout touch-up, compare).
    private(set) var original: URL?

    let models: [ModelEntry] = Core.engine.models()

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }

    func setPhoto(_ url: URL) {
        undoStack.removeAll()
        redoStack.removeAll()
        original = url
        show(url)
    }

    /// Replace the working image with an edited version (undoable).
    func commit(_ url: URL) {
        if let c = current { undoStack.append(c) }
        if undoStack.count > 30 { undoStack.removeFirst() }
        redoStack.removeAll()
        show(url)
    }

    func undo() {
        guard let prev = undoStack.popLast(), let c = current else { return }
        redoStack.append(c)
        show(prev)
    }

    func redo() {
        guard let next = redoStack.popLast(), let c = current else { return }
        undoStack.append(c)
        show(next)
    }

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
