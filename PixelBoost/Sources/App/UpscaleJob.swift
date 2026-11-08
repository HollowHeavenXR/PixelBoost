import SwiftUI
import UIKit

/// One upscale run: calls the Rust pipeline on a background thread and
/// paints finished tiles into a live preview as they arrive.
@MainActor
final class UpscaleJob: ObservableObject {
    enum State: Equatable { case idle, running, done, failed(String) }

    @Published private(set) var state: State = .idle
    @Published private(set) var done = 0
    @Published private(set) var total = 0
    @Published private(set) var canvas: UIImage?
    @Published private(set) var outcome: UpscaleOutcome?
    @Published private(set) var startedAt = Date()

    private var token = CancelToken()

    var progress: Double { total == 0 ? 0 : Double(done) / Double(total) }

    var eta: TimeInterval? {
        guard done > 0, total > done else { return nil }
        let elapsed = Date().timeIntervalSince(startedAt)
        return elapsed / Double(done) * Double(total - done)
    }

    func cancel() { token.cancel() }

    func reset() {
        state = .idle
        canvas = nil
        outcome = nil
        done = 0
        total = 0
    }

    func run(input: URL, settings: UpscaleSettings, base: UIImage?, outW: Int, outH: Int) async -> UpscaleOutcome? {
        token = CancelToken()
        state = .running
        done = 0
        total = 0
        outcome = nil
        startedAt = Date()
        let painter = TilePainter(outW: outW, outH: outH, base: base) { [weak self] img, d, t in
            Task { @MainActor in
                guard let self else { return }
                if let img { self.canvas = img }
                self.done = d
                self.total = t
            }
        }
        let token = self.token
        let out = Core.newFile("png")
        let result = await Task.detached(priority: .userInitiated) { () -> Result<UpscaleOutcome, Error> in
            do {
                return .success(try Core.engine.upscale(inputPath: input.path, outputPath: out.path, settings: settings, listener: painter, cancel: token))
            } catch {
                return .failure(error)
            }
        }.value
        switch result {
        case .success(let o):
            outcome = o
            state = .done
            canvas = (try? Core.engine.loadPreview(path: o.outputPath, maxSide: 2048))?.uiImage
            return o
        case .failure(let e):
            if let pe = e as? PbError, case .Cancelled = pe {
                state = .idle
            } else {
                state = .failed(describe(e))
            }
            return nil
        }
    }
}

/// Receives tiles from Rust (on its worker thread) and composites them into
/// a display-sized bitmap over a dimmed copy of the source.
final class TilePainter: UpscaleListener, @unchecked Sendable {
    private let ctx: CGContext?
    private let k: CGFloat
    private let size: CGSize
    private let publish: (UIImage?, Int, Int) -> Void
    private var lastPublish = Date.distantPast
    private let lock = NSLock()

    init(outW: Int, outH: Int, base: UIImage?, publish: @escaping (UIImage?, Int, Int) -> Void) {
        let maxSide: CGFloat = 1600
        k = min(1, maxSide / CGFloat(max(outW, outH, 1)))
        size = CGSize(width: max(1, (CGFloat(outW) * k).rounded()), height: max(1, (CGFloat(outH) * k).rounded()))
        ctx = CGContext(data: nil, width: Int(size.width), height: Int(size.height), bitsPerComponent: 8, bytesPerRow: 0,
                        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        self.publish = publish
        if let ctx, let cg = base?.cgImage {
            ctx.interpolationQuality = .medium
            ctx.draw(cg, in: CGRect(origin: .zero, size: size))
            ctx.setFillColor(UIColor.black.withAlphaComponent(0.55).cgColor)
            ctx.fill(CGRect(origin: .zero, size: size))
        }
    }

    func onTile(done: UInt32, total: UInt32, x: UInt32, y: UInt32, w: UInt32, h: UInt32, pixelW: UInt32, pixelH: UInt32, rgba: Data) {
        lock.lock()
        defer { lock.unlock() }
        if let ctx, w > 0, pixelW > 0, let tile = RgbaImage(width: pixelW, height: pixelH, rgba: rgba).uiImage?.cgImage {
            // CGContext's origin is bottom-left.
            let r = CGRect(x: CGFloat(x) * k, y: size.height - CGFloat(y + h) * k, width: CGFloat(w) * k, height: CGFloat(h) * k)
            ctx.interpolationQuality = .high
            ctx.draw(tile, in: r)
        }
        let now = Date()
        let last = done == total
        if last || now.timeIntervalSince(lastPublish) > 0.25 {
            lastPublish = now
            publish(ctx?.makeImage().map { UIImage(cgImage: $0) }, Int(done), Int(total))
        } else {
            publish(nil, Int(done), Int(total))
        }
    }
}
