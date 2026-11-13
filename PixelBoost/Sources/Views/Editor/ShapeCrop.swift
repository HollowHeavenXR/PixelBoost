import SwiftUI
import UIKit

// Shape crops + rings (Crop tool). Geometry comes from the Rust core: the
// overlay draws outlines traced by `shapeOutline`, the panel's preview is
// the real `shape_crop` op run on a thumbnail.

/// One ring around the shape (fractions of the shape's shorter side).
struct RingSpec: Equatable, Identifiable {
    let id = UUID()
    var style = "solid"
    var color = Color.white
    var color2 = Color(red: 1, green: 0.3, blue: 0.6)
    var width = 0.04
    var gap = 0.02
    var count = 24.0

    var usesSecondColor: Bool { style == "gradient" || style == "neon" }

    var dict: [String: Any] {
        var d: [String: Any] = ["style": style, "color": rgba(color), "width": width, "gap": gap, "count": count]
        if usesSecondColor { d["color2"] = rgba(color2) }
        return d
    }

    static func == (a: RingSpec, b: RingSpec) -> Bool {
        a.style == b.style && a.color == b.color && a.color2 == b.color2 && a.width == b.width && a.gap == b.gap && a.count == b.count
    }

    /// One-tap ring looks.
    static let presets: [(String, [RingSpec])] = [
        ("Story", [RingSpec(style: "story", width: 0.045, gap: 0.03)]),
        ("White", [RingSpec(style: "solid", color: .white, width: 0.035, gap: 0)]),
        ("Gold", [RingSpec(style: "gradient", color: Color(red: 1, green: 0.85, blue: 0.4), color2: Color(red: 0.7, green: 0.45, blue: 0.1), width: 0.04, gap: 0.015)]),
        ("Neon", [RingSpec(style: "neon", color: Color(red: 1, green: 0.2, blue: 0.7), color2: .white, width: 0.025, gap: 0.04)]),
        ("Rainbow", [RingSpec(style: "rainbow", width: 0.04, gap: 0.02)]),
        ("Live", [RingSpec(style: "solid", color: Color(red: 1, green: 0.15, blue: 0.35), width: 0.03, gap: 0.02),
                  RingSpec(style: "solid", color: .white, width: 0.015, gap: 0.0)]),
        ("Dotted", [RingSpec(style: "dotted", color: Color(red: 1, green: 0.8, blue: 0.2), width: 0.035, gap: 0.025, count: 30)]),
        ("Double", [RingSpec(style: "double", color: .white, width: 0.05, gap: 0.02)]),
    ]
}

enum ShapeKit {
    static let shapes: [ShapeEntry] = Core.engine.shapes()
    static let ringStyles: [ShapeEntry] = Core.engine.ringStyles()

    static func stretches(_ id: String) -> Bool { shapes.first { $0.id == id }?.stretches ?? true }

    /// What the shape slider does (nil = no slider).
    static func paramLabel(_ id: String) -> (String, ClosedRange<Double>, Double)? {
        switch id {
        case "rounded": return ("Corners", 0.02...0.5, 0.18)
        case "squircle": return ("Squareness", 0.05...1, 0.31)
        case "star": return ("Inner points", 0.2...0.9, 0.5)
        case "flower": return ("Petal depth", 0.04...0.4, 0.16)
        case "badge": return ("Scallops", 0.02...0.2, 0.06)
        case "burst": return ("Spikes", 0.05...0.6, 0.18)
        default: return nil
        }
    }

    @MainActor private static var cache: [String: [CGPoint]] = [:]

    /// Outline normalised to the box (0...1, may go past for offsets).
    @MainActor
    static func outline(_ shape: String, w: Double, h: Double, rotation: Double, param: Double, offset: Double, points: Int = 192) -> [CGPoint] {
        let key = "\(shape)|\(Int(w))|\(Int(h))|\(Int(rotation * 10))|\(Int(param * 1000))|\(Int(offset * 10))|\(points)"
        if let c = cache[key] { return c }
        let f = (try? Core.engine.shapeOutline(shape: shape, width: Float(max(w, 4)), height: Float(max(h, 4)), rotation: Float(rotation),
                                               param: Float(param), offset: Float(offset), points: UInt32(points))) ?? []
        var pts: [CGPoint] = []
        pts.reserveCapacity(f.count / 2)
        var i = 0
        while i + 1 < f.count {
            pts.append(CGPoint(x: Double(f[i]), y: Double(f[i + 1])))
            i += 2
        }
        if cache.count > 400 { cache.removeAll() }
        cache[key] = pts
        return pts
    }

    static func path(_ pts: [CGPoint], in box: CGRect) -> Path {
        var p = Path()
        guard let first = pts.first else { return p }
        p.move(to: CGPoint(x: box.minX + first.x * box.width, y: box.minY + first.y * box.height))
        for q in pts.dropFirst() { p.addLine(to: CGPoint(x: box.minX + q.x * box.width, y: box.minY + q.y * box.height)) }
        p.closeSubpath()
        return p
    }
}

/// The op for a shape crop, or nil for a plain rectangle.
@MainActor
func shapeCropOp(_ e: EditorState) -> [String: Any]? {
    guard e.cropShape != "rect" else { return nil }
    let c = e.crop
    var d: [String: Any] = ["shape": e.cropShape, "x": c.minX, "y": c.minY, "w": c.width, "h": c.height,
                            "rotation": e.shapeRotation, "rings": e.rings.map(\.dict)]
    if ShapeKit.paramLabel(e.cropShape) != nil { d["param"] = e.shapeParam }
    if e.shapeFilled { d["background"] = rgba(e.shapeFill) }
    return opDict("shape_crop", d)
}

// MARK: - overlay

/// Shape outline and ring preview inside the crop rectangle.
struct ShapeOverlay: View {
    let rect: CGRect
    @ObservedObject var editor: EditorState
    let imageSize: CGSize

    /// A ring's paths, traced ahead of drawing.
    private struct RingPaths {
        let ring: RingSpec
        let mid: Path
        let inner: Path
        let outer: Path
        let width: CGFloat
        let thirdWidth: CGFloat
        let perimeter: CGFloat
    }

    var body: some View {
        let c = editor.crop
        let box = CGRect(x: rect.minX + c.minX * rect.width, y: rect.minY + c.minY * rect.height,
                         width: c.width * rect.width, height: c.height * rect.height)
        // Shape geometry in preview pixels.
        let w = Double(c.width * imageSize.width), h = Double(c.height * imageSize.height)
        let size = min(w, h)
        let k = imageSize.width > 0 ? rect.width / imageSize.width : 1 // screen points per pixel
        let trace = { (offset: Double) -> Path in
            ShapeKit.path(ShapeKit.outline(editor.cropShape, w: w, h: h, rotation: editor.shapeRotation, param: editor.shapeParam, offset: offset), in: box)
        }
        let edge = trace(0)
        var cur = 0.0
        var rings: [RingPaths] = []
        for r in editor.rings {
            let start = cur + r.gap * size
            let end = start + max(r.width * size, 1)
            cur = end
            let bw = end - start
            let mid = trace((start + end) / 2)
            let double = r.style == "double"
            rings.append(RingPaths(ring: r, mid: mid,
                                   inner: double ? trace(start + bw / 6) : mid,
                                   outer: double ? trace(end - bw / 6) : mid,
                                   width: CGFloat(bw) * k, thirdWidth: CGFloat(bw / 3) * k,
                                   perimeter: Self.perimeter(mid)))
        }
        let centre = CGPoint(x: box.midX, y: box.midY)
        return Canvas { ctx, _ in
            // Dim what gets cut away.
            var cut = Path(box)
            cut.addPath(edge)
            ctx.fill(cut, with: .color(.black.opacity(0.5)), style: FillStyle(eoFill: true))
            for p in rings { Self.draw(p, centre: centre, ctx: &ctx) }
            ctx.stroke(edge, with: .color(.white.opacity(0.9)), style: StrokeStyle(lineWidth: 1, dash: [5, 4]))
        }
        .allowsHitTesting(false)
    }

    private static let story: [Color] = {
        let a = Color(red: 1, green: 0.83, blue: 0.25), b = Color(red: 0.98, green: 0.45, blue: 0.18)
        let c = Color(red: 0.86, green: 0.15, blue: 0.48), d = Color(red: 0.55, green: 0.2, blue: 0.85)
        return [a, b, c, d, c, b, a]
    }()

    private static func draw(_ p: RingPaths, centre: CGPoint, ctx: inout GraphicsContext) {
        let r = p.ring
        let conic = { (colors: [Color]) -> GraphicsContext.Shading in
            .conicGradient(Gradient(colors: colors), center: centre, angle: .degrees(-90))
        }
        switch r.style {
        case "gradient":
            ctx.stroke(p.mid, with: conic([r.color, r.color2, r.color]), lineWidth: p.width)
        case "story":
            ctx.stroke(p.mid, with: conic(story), lineWidth: p.width)
        case "rainbow":
            ctx.stroke(p.mid, with: conic((0...6).map { Color(hue: Double($0) / 6, saturation: 0.9, brightness: 1) }), lineWidth: p.width)
        case "double":
            ctx.stroke(p.inner, with: .color(r.color), lineWidth: p.thirdWidth)
            ctx.stroke(p.outer, with: .color(r.color), lineWidth: p.thirdWidth)
        case "dashed", "dotted":
            let seg = p.perimeter / CGFloat(max(r.count.rounded(), 3))
            let style = r.style == "dashed"
                ? StrokeStyle(lineWidth: p.width, dash: [seg * 0.62, seg * 0.38])
                : StrokeStyle(lineWidth: p.width, lineCap: .round, dash: [0.001, seg])
            ctx.stroke(p.mid, with: .color(r.color), style: style)
        case "neon":
            var glow = ctx
            glow.addFilter(.shadow(color: r.color, radius: p.width * 2.5))
            glow.stroke(p.mid, with: .color(r.color), lineWidth: p.width)
            ctx.stroke(p.mid, with: .color(r.color2.opacity(0.85)), lineWidth: p.width * 0.35)
        default:
            ctx.stroke(p.mid, with: .color(r.color), lineWidth: p.width)
        }
    }

    private static func perimeter(_ p: Path) -> CGFloat {
        var total: CGFloat = 0
        var last: CGPoint?
        var first: CGPoint?
        p.forEach { el in
            switch el {
            case .move(let q): last = q; first = q
            case .line(let q):
                if let l = last { total += hypot(q.x - l.x, q.y - l.y) }
                last = q
            case .closeSubpath:
                if let l = last, let f = first { total += hypot(f.x - l.x, f.y - l.y) }
            default: break
            }
        }
        return max(total, 1)
    }
}

// MARK: - controls

/// Small outline icon of a shape.
struct ShapeIcon: View {
    let id: String
    var body: some View {
        let box = CGRect(x: 3, y: 3, width: 24, height: 24)
        let path = id == "rect" ? Path(box) : ShapeKit.path(ShapeKit.outline(id, w: 100, h: 100, rotation: 0, param: 0, offset: 0, points: 96), in: box)
        return path.stroke(.white, lineWidth: 1.6).frame(width: 30, height: 30)
    }
}

struct ShapeCropControls: View {
    @ObservedObject var editor: EditorState
    @ObservedObject var session: ToolSession
    /// Lock / unlock the crop to 1:1 when the shape needs it.
    let lockSquare: (Bool) -> Void
    @State private var selected = 0
    @State private var thumb: UIImage?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            shapePicker
            if editor.cropShape != "rect" {
                HStack(alignment: .top, spacing: 12) {
                    VStack(spacing: 8) {
                        SliderRow(label: "Turn", value: $editor.shapeRotation, range: -180...180, format: { "\(Int($0))°" })
                        if let p = ShapeKit.paramLabel(editor.cropShape) {
                            SliderRow(label: p.0, value: $editor.shapeParam, range: p.1)
                        }
                    }
                    preview
                }
                ringEditor
                HStack {
                    Toggle("Fill behind", isOn: $editor.shapeFilled).tint(Theme.amber)
                    if editor.shapeFilled { ColorPicker("", selection: $editor.shapeFill, supportsOpacity: false).labelsHidden() }
                }
                .font(.subheadline)
                if !editor.shapeFilled {
                    Text("Outside the shape is transparent — export as PNG to keep it.").font(.caption).foregroundStyle(Theme.dim)
                }
            }
        }
        .task(id: previewKey) { await renderPreview() }
    }

    private var shapePicker: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                chip("rect", "Rect")
                ForEach(ShapeKit.shapes, id: \.id) { s in chip(s.id, s.name) }
            }
        }
    }

    private func chip(_ id: String, _ name: String) -> some View {
        let on = editor.cropShape == id
        return Button {
            editor.cropShape = id
            editor.shapeParam = ShapeKit.paramLabel(id)?.2 ?? 0
            lockSquare(id != "rect" && !ShapeKit.stretches(id))
        } label: {
            VStack(spacing: 3) {
                ShapeIcon(id: id)
                Text(name).font(.caption2).lineLimit(1)
            }
            .frame(width: 62, height: 58)
            .background(on ? AnyShapeStyle(Theme.amber.opacity(0.2)) : AnyShapeStyle(Theme.card2), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Theme.amber : .clear, lineWidth: 1.5))
        }
        .buttonStyle(.plain)
    }

    // Real render of the op on a thumbnail (checkerboard = transparent).
    private var preview: some View {
        ZStack {
            Checkerboard()
            if let thumb { Image(uiImage: thumb).resizable().scaledToFit() }
        }
        .frame(width: 96, height: 96)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var previewKey: String {
        guard let op = shapeCropOp(editor) else { return "" }
        return opJSON("chain", ["ops": [op]])
    }

    private func renderPreview() async {
        guard let base = session.base, !previewKey.isEmpty else { thumb = nil; return }
        let op = previewKey
        try? await Task.sleep(nanoseconds: 60_000_000) // coalesce slider drags
        if Task.isCancelled { return }
        let img = await Task.detached(priority: .userInitiated) { (try? Core.engine.editThumbnail(image: base, opJson: op, maxSide: 360))?.uiImage }.value
        if !Task.isCancelled { thumb = img }
    }

    // MARK: rings

    private var ringEditor: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text("Rings").font(.subheadline.weight(.semibold))
                ForEach(editor.rings.indices, id: \.self) { i in
                    Button { selected = i } label: {
                        Circle().strokeBorder(editor.rings[i].color, lineWidth: 3)
                            .frame(width: 24, height: 24)
                            .overlay(Circle().stroke(selected == i ? Theme.amber : .clear, lineWidth: 2).padding(-3))
                    }
                    .buttonStyle(.plain)
                }
                if editor.rings.count < 4 {
                    Button {
                        editor.rings.append(RingSpec(color: editor.rings.isEmpty ? .white : Color(red: 1, green: 0.7, blue: 0.24), gap: editor.rings.isEmpty ? 0.02 : 0))
                        selected = editor.rings.count - 1
                    } label: { Image(systemName: "plus.circle.fill").font(.title3) }
                    .buttonStyle(.plain).foregroundStyle(Theme.amber)
                }
                Spacer()
                if selected < editor.rings.count {
                    Button { editor.rings.remove(at: selected); selected = max(0, selected - 1) } label: { Image(systemName: "trash") }
                        .buttonStyle(.plain).foregroundStyle(Theme.dim)
                }
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(RingSpec.presets.indices, id: \.self) { i in
                        let p = RingSpec.presets[i]
                        Button(p.0) { editor.rings = p.1; selected = 0 }
                            .font(.caption.weight(.medium)).padding(.horizontal, 12).padding(.vertical, 6)
                            .background(Theme.card2, in: Capsule()).buttonStyle(.plain)
                    }
                    Button("None") { editor.rings = []; selected = 0 }
                        .font(.caption.weight(.medium)).padding(.horizontal, 12).padding(.vertical, 6)
                        .background(Theme.card2, in: Capsule()).buttonStyle(.plain).foregroundStyle(Theme.dim)
                }
            }
            if selected < editor.rings.count {
                ringControls($editor.rings[selected])
            }
        }
    }

    private func ringControls(_ r: Binding<RingSpec>) -> some View {
        VStack(spacing: 8) {
            ChipPicker(options: ShapeKit.ringStyles.map { ($0.id, $0.name) }, selection: r.style)
            HStack {
                if r.wrappedValue.style != "rainbow" && r.wrappedValue.style != "story" {
                    ColorPicker("Colour", selection: r.color, supportsOpacity: true)
                }
                if r.wrappedValue.usesSecondColor {
                    ColorPicker(r.wrappedValue.style == "neon" ? "Core" : "To", selection: r.color2, supportsOpacity: true)
                }
            }
            .font(.subheadline)
            SliderRow(label: "Thickness", value: r.width, range: 0.005...0.15)
            SliderRow(label: "Gap", value: r.gap, range: -0.05...0.15)
            if r.wrappedValue.style == "dashed" || r.wrappedValue.style == "dotted" {
                SliderRow(label: r.wrappedValue.style == "dashed" ? "Dashes" : "Dots", value: r.count, range: 6...72, format: { "\(Int($0))" })
            }
        }
    }
}

struct Checkerboard: View {
    var body: some View {
        Canvas { ctx, size in
            let s: CGFloat = 8
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(Color(white: 0.32)))
            var y: CGFloat = 0, row = 0
            while y < size.height {
                var x: CGFloat = row % 2 == 0 ? 0 : s
                while x < size.width {
                    ctx.fill(Path(CGRect(x: x, y: y, width: s, height: s)), with: .color(Color(white: 0.45)))
                    x += 2 * s
                }
                y += s
                row += 1
            }
        }
    }
}
