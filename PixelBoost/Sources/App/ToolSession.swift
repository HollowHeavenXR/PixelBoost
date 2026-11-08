import SwiftUI
import UIKit

/// Live preview + apply for one editing tool. Previews run the same Rust
/// operation on a 1280px copy; Apply runs it on the full-size file.
@MainActor
final class ToolSession: ObservableObject {
    @Published private(set) var live: UIImage?
    @Published private(set) var busy = false
    private(set) var base: RgbaImage?
    private var pending: String?
    private var running = false
    private var source: URL?

    func load(_ url: URL?) {
        guard let url, url != source else { return }
        source = url
        live = nil
        base = nil
        Task.detached(priority: .userInitiated) {
            let b = try? Core.engine.loadPreview(path: url.path, maxSide: 1280)
            await MainActor.run {
                guard self.source == url else { return }
                self.base = b
                if let p = self.pending { self.pending = nil; self.preview(p) }
            }
        }
    }

    func clear() {
        live = nil
        pending = nil
    }

    /// Latest-wins preview: while one render runs, only the newest request
    /// is kept.
    func preview(_ op: String?) {
        guard let op else { live = nil; return }
        guard let base else { pending = op; return }
        if running { pending = op; return }
        running = true
        Task.detached(priority: .userInitiated) {
            let out = try? Core.engine.editRgba(image: base, opJson: op)
            let img = out?.uiImage
            await MainActor.run {
                self.running = false
                if let img { self.live = img }
                if let next = self.pending { self.pending = nil; self.preview(next) }
            }
        }
    }

    func apply(_ op: String, to app: AppModel, name: String) {
        guard let src = app.current else { return }
        busy = true
        let out = Core.newFile("png")
        Task.detached(priority: .userInitiated) {
            let r = Result { try Core.engine.editFile(inputPath: src.path, outputPath: out.path, opJson: op) }
            await MainActor.run {
                self.busy = false
                switch r {
                case .success:
                    self.live = nil
                    app.commit(out)
                    app.log("edit", ["tool": name])
                case .failure(let e):
                    app.message = describe(e)
                }
            }
        }
    }
}

/// An image fitted into the available space, with an overlay that knows the
/// image's on-screen rectangle (for brushes, crop handles, taps).
struct ImageStage<Overlay: View>: View {
    let image: UIImage?
    @ViewBuilder var overlay: (CGRect) -> Overlay

    var body: some View {
        GeometryReader { geo in
            if let image {
                let r = fitted(image.size, geo.size)
                ZStack(alignment: .topLeading) {
                    Image(uiImage: image).resizable().interpolation(.high)
                        .frame(width: r.width, height: r.height)
                        .offset(x: r.minX, y: r.minY)
                    overlay(r)
                }
            }
        }
    }

    private func fitted(_ s: CGSize, _ box: CGSize) -> CGRect {
        let k = min(box.width / max(s.width, 1), box.height / max(s.height, 1))
        let w = s.width * k, h = s.height * k
        return CGRect(x: (box.width - w) / 2, y: (box.height - h) / 2, width: w, height: h)
    }
}

/// Freehand mask strokes in normalised image coordinates.
struct Stroke: Equatable {
    var points: [CGPoint]
    var radius: CGFloat // fraction of image width
    var erase = false
}

enum MaskRenderer {
    /// White-on-black mask PNG at `size` pixels.
    static func render(_ strokes: [Stroke], size: CGSize) -> URL? {
        let fmt = UIGraphicsImageRendererFormat()
        fmt.scale = 1
        fmt.opaque = true
        let img = UIGraphicsImageRenderer(size: size, format: fmt).image { ctx in
            UIColor.black.setFill()
            ctx.fill(CGRect(origin: .zero, size: size))
            let c = ctx.cgContext
            c.setLineCap(.round)
            c.setLineJoin(.round)
            for s in strokes {
                c.setStrokeColor(s.erase ? UIColor.black.cgColor : UIColor.white.cgColor)
                c.setFillColor(s.erase ? UIColor.black.cgColor : UIColor.white.cgColor)
                let r = s.radius * size.width
                c.setLineWidth(r * 2)
                let pts = s.points.map { CGPoint(x: $0.x * size.width, y: $0.y * size.height) }
                if pts.count == 1, let p = pts.first {
                    c.fillEllipse(in: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2))
                } else if let first = pts.first {
                    c.beginPath()
                    c.move(to: first)
                    for p in pts.dropFirst() { c.addLine(to: p) }
                    c.strokePath()
                }
            }
        }
        let url = Core.newFile("png")
        guard let d = img.pngData(), (try? d.write(to: url)) != nil else { return nil }
        return url
    }
}

/// Draws strokes over an ImageStage rectangle and records new ones.
struct BrushOverlay: View {
    let rect: CGRect
    @Binding var strokes: [Stroke]
    var radius: CGFloat
    var erase = false
    var onEnd: () -> Void = {}
    @State private var currentIndex: Int?

    var body: some View {
        Canvas { ctx, _ in
            for s in strokes {
                var path = Path()
                let pts = s.points.map { CGPoint(x: rect.minX + $0.x * rect.width, y: rect.minY + $0.y * rect.height) }
                if let f = pts.first {
                    path.move(to: f)
                    for p in pts.dropFirst() { path.addLine(to: p) }
                    if pts.count == 1 { path.addLine(to: CGPoint(x: f.x + 0.1, y: f.y)) }
                }
                ctx.stroke(path, with: .color(s.erase ? .blue.opacity(0.35) : Theme.accent.opacity(0.45)),
                           style: StrokeStyle(lineWidth: s.radius * rect.width * 2, lineCap: .round, lineJoin: .round))
            }
        }
        .contentShape(Rectangle())
        .gesture(DragGesture(minimumDistance: 0)
            .onChanged { v in
                let p = CGPoint(x: (v.location.x - rect.minX) / rect.width, y: (v.location.y - rect.minY) / rect.height)
                guard (0...1).contains(p.x), (0...1).contains(p.y) else { return }
                if let i = currentIndex {
                    strokes[i].points.append(p)
                } else {
                    strokes.append(Stroke(points: [p], radius: radius, erase: erase))
                    currentIndex = strokes.count - 1
                }
            }
            .onEnded { _ in
                currentIndex = nil
                onEnd()
            })
    }
}
