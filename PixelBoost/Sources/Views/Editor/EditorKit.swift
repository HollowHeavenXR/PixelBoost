import SwiftUI
import UIKit

// MARK: - value helpers

/// SwiftUI colour -> [r, g, b, a] in 0...1 (sRGB) for the Rust ops.
func rgba(_ c: Color) -> [Double] {
    let u = UIColor(c)
    var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
    u.getRed(&r, green: &g, blue: &b, alpha: &a)
    return [Double(min(max(r, 0), 1)), Double(min(max(g, 0), 1)), Double(min(max(b, 0), 1)), Double(a)]
}

func rgb(_ c: Color) -> [Double] { Array(rgba(c).prefix(3)) }

/// A drawing on the canvas, normalised to the image.
struct DrawMark: Equatable {
    var kind: String // stroke, line, arrow, rect, ellipse
    var points: [CGPoint]
    var style: String
    var color: [Double]
    var width: Double
    var fill = false

    var dict: [String: Any] {
        let a = points.first ?? .zero, b = points.last ?? .zero
        switch kind {
        case "line", "arrow":
            return ["kind": "line", "x0": a.x, "y0": a.y, "x1": b.x, "y1": b.y, "width": width, "color": color, "arrow": kind == "arrow"]
        case "rect":
            return ["kind": "rect", "x0": a.x, "y0": a.y, "x1": b.x, "y1": b.y, "width": width, "color": color, "fill": fill, "radius": 0.08]
        case "ellipse":
            return ["kind": "ellipse", "x0": a.x, "y0": a.y, "x1": b.x, "y1": b.y, "width": width, "color": color, "fill": fill]
        default:
            return ["kind": "stroke", "points": points.map { [$0.x, $0.y] }, "width": width, "color": color, "style": style]
        }
    }
}

/// Slider values of a local adjustment (brush, gradient, radial).
struct LocalAdjust: Equatable {
    var exposure = 0.0
    var contrast = 1.0
    var highlights = 0.0
    var shadows = 0.0
    var saturation = 1.0
    var warmth = 0.0
    var tint = 0.0
    var clarity = 0.0
    var dehaze = 0.0
    var blur = 0.0

    var isNeutral: Bool { self == LocalAdjust() }

    var ops: [[String: Any]] {
        var o: [[String: Any]] = [opDict("adjust", [
            "exposure": exposure, "contrast": contrast, "highlights": highlights, "shadows": shadows,
            "saturation": saturation, "warmth": warmth, "tint": tint, "clarity": clarity, "dehaze": dehaze,
        ])]
        if blur > 0.001 { o.append(opDict("blur", ["radius": blur * 0.03])) }
        return o
    }
}

struct LocalAdjustControls: View {
    @Binding var value: LocalAdjust
    var body: some View {
        VStack(spacing: 10) {
            SliderRow(label: "Exposure", value: $value.exposure, range: -2...2)
            SliderRow(label: "Contrast", value: $value.contrast, range: 0.5...1.5)
            SliderRow(label: "Highlights", value: $value.highlights, range: -1...1)
            SliderRow(label: "Shadows", value: $value.shadows, range: -1...1)
            SliderRow(label: "Saturation", value: $value.saturation, range: 0...2)
            SliderRow(label: "Temperature", value: $value.warmth, range: -1...1)
            SliderRow(label: "Tint", value: $value.tint, range: -1...1)
            SliderRow(label: "Clarity", value: $value.clarity, range: -1...1)
            SliderRow(label: "Dehaze", value: $value.dehaze, range: -1...1)
            SliderRow(label: "Blur", value: $value.blur)
            Button("Reset sliders") { value = LocalAdjust() }.font(.caption)
        }
    }
}

// MARK: - canvas overlays

private func toView(_ p: CGPoint, _ r: CGRect) -> CGPoint {
    CGPoint(x: r.minX + p.x * r.width, y: r.minY + p.y * r.height)
}

private func toImage(_ p: CGPoint, _ r: CGRect) -> CGPoint {
    CGPoint(x: min(max((p.x - r.minX) / max(r.width, 1), 0), 1), y: min(max((p.y - r.minY) / max(r.height, 1), 0), 1))
}

struct Handle: View {
    var size: CGFloat = 22
    var body: some View {
        Circle().fill(.white).frame(width: size, height: size)
            .overlay(Circle().stroke(Theme.amber, lineWidth: 3))
            .shadow(color: .black.opacity(0.5), radius: 3)
    }
}

/// Linear gradient mask: full effect at `start`, none at `end`.
struct GradientOverlay: View {
    let rect: CGRect
    @Binding var start: CGPoint
    @Binding var end: CGPoint

    var body: some View {
        let a = toView(start, rect), b = toView(end, rect)
        let dx = b.x - a.x, dy = b.y - a.y
        let len = max(hypot(dx, dy), 1)
        // Perpendicular, long enough to cross the photo.
        let px = -dy / len * (rect.width + rect.height), py = dx / len * (rect.width + rect.height)
        ZStack {
            Path { p in
                for c in [a, CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2), b] {
                    p.move(to: CGPoint(x: c.x - px, y: c.y - py))
                    p.addLine(to: CGPoint(x: c.x + px, y: c.y + py))
                }
            }
            .stroke(.white.opacity(0.8), style: StrokeStyle(lineWidth: 1.5, dash: [6, 5]))
            .clipShape(Rectangle().path(in: rect))
            Path { p in p.move(to: a); p.addLine(to: b) }.stroke(Theme.amber, lineWidth: 2)
            Handle().position(a).gesture(DragGesture().onChanged { start = toImage($0.location, rect) })
            Handle(size: 18).position(b).gesture(DragGesture().onChanged { end = toImage($0.location, rect) })
        }
    }
}

/// Radial mask: drag inside to move, handles set the two radii.
struct RadialOverlay: View {
    let rect: CGRect
    @Binding var center: CGPoint
    @Binding var radius: CGSize
    @State private var startCenter: CGPoint?

    var body: some View {
        let c = toView(center, rect)
        let w: CGFloat = radius.width * rect.width * 2
        let h: CGFloat = radius.height * rect.height * 2
        ZStack {
            Ellipse().stroke(.white.opacity(0.9), style: StrokeStyle(lineWidth: 1.5, dash: [6, 5]))
                .frame(width: w, height: h).position(c)
            Ellipse().fill(Color.white.opacity(0.001)).frame(width: w, height: h).position(c)
                .gesture(DragGesture().onChanged { v in
                    let s = startCenter ?? center
                    if startCenter == nil { startCenter = center }
                    center = CGPoint(x: min(max(s.x + v.translation.width / rect.width, 0), 1),
                                     y: min(max(s.y + v.translation.height / rect.height, 0), 1))
                }.onEnded { _ in startCenter = nil })
            Handle(size: 18).position(x: c.x + w / 2, y: c.y)
                .gesture(DragGesture().onChanged { v in
                    radius.width = max(0.02, (v.location.x - c.x) / rect.width)
                })
            Handle(size: 18).position(x: c.x, y: c.y + h / 2)
                .gesture(DragGesture().onChanged { v in
                    radius.height = max(0.02, (v.location.y - c.y) / rect.height)
                })
            Image(systemName: "plus").font(.caption.bold()).foregroundStyle(.white).position(c)
        }
    }
}

/// Drag anywhere to move a point (text, layer, blur centre).
struct PointOverlay: View {
    let rect: CGRect
    @Binding var point: CGPoint
    var showMarker = true
    @State private var start: CGPoint?

    var body: some View {
        ZStack {
            Color.clear.contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                    let s = start ?? point
                    if start == nil { start = point }
                    point = CGPoint(x: min(max(s.x + v.translation.width / rect.width, 0), 1),
                                    y: min(max(s.y + v.translation.height / rect.height, 0), 1))
                }.onEnded { _ in start = nil })
            if showMarker {
                Image(systemName: "move.3d").font(.caption.bold()).padding(6)
                    .background(.black.opacity(0.5), in: Circle()).foregroundStyle(.white)
                    .position(toView(point, rect)).allowsHitTesting(false)
            }
        }
    }
}

/// Horizontal (optionally tilted) focus band for tilt-shift.
struct BandOverlay: View {
    let rect: CGRect
    @Binding var y: CGFloat
    let band: Double
    let angle: Double

    var body: some View {
        let cy: CGFloat = rect.minY + y * rect.height
        let half = CGFloat(band) * rect.height / 2
        let rad = CGFloat(angle * .pi / 180)
        let reach = rect.width + rect.height
        let dx: CGFloat = cos(rad) * reach
        let dy: CGFloat = sin(rad) * reach
        ZStack {
            Path { p in
                for off in [-half, 0, half] {
                    // Offset perpendicular to the band.
                    let c = CGPoint(x: rect.midX - sin(rad) * off, y: cy + cos(rad) * off)
                    p.move(to: CGPoint(x: c.x - dx, y: c.y - dy))
                    p.addLine(to: CGPoint(x: c.x + dx, y: c.y + dy))
                }
            }
            .stroke(.white.opacity(0.85), style: StrokeStyle(lineWidth: 1.5, dash: [6, 5]))
            .clipShape(Rectangle().path(in: rect))
            Color.clear.contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                    y = min(max((v.location.y - rect.minY) / rect.height, 0), 1)
                })
        }
    }
}

/// Freehand / shape drawing. Strokes are previewed by Rust (the op is
/// re-rendered as the finger moves).
struct DrawOverlay: View {
    let rect: CGRect
    @ObservedObject var editor: EditorState
    @State private var current: Int?

    var body: some View {
        Color.clear.contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0)
                .onChanged { v in
                    let p = toImage(v.location, rect)
                    if let i = current, i < editor.marks.count {
                        if editor.marks[i].kind == "stroke" {
                            if let last = editor.marks[i].points.last, hypot((p.x - last.x) * rect.width, (p.y - last.y) * rect.height) < 2 { return }
                            editor.marks[i].points.append(p)
                        } else {
                            editor.marks[i].points = [editor.marks[i].points[0], p]
                        }
                    } else {
                        let shape = ["line", "arrow", "rect", "ellipse"].contains(editor.drawStyle)
                        editor.marks.append(DrawMark(kind: shape ? editor.drawStyle : "stroke", points: shape ? [p, p] : [p],
                                                     style: editor.drawStyle, color: rgba(editor.drawColor),
                                                     width: editor.drawWidth, fill: editor.drawFill))
                        current = editor.marks.count - 1
                    }
                }
                .onEnded { _ in current = nil })
    }
}

/// Thirds + fine grid (straightening, perspective).
struct GridOverlay: View {
    let rect: CGRect
    var lines = 9
    var body: some View {
        Path { p in
            for i in 1..<lines {
                let x = rect.minX + rect.width * CGFloat(i) / CGFloat(lines)
                let y = rect.minY + rect.height * CGFloat(i) / CGFloat(lines)
                p.move(to: CGPoint(x: x, y: rect.minY)); p.addLine(to: CGPoint(x: x, y: rect.maxY))
                p.move(to: CGPoint(x: rect.minX, y: y)); p.addLine(to: CGPoint(x: rect.maxX, y: y))
            }
        }
        .stroke(.white.opacity(0.35), lineWidth: 0.7)
        .allowsHitTesting(false)
    }
}

// MARK: - colour wheel

/// Hue (angle) + strength (distance from the centre) picker for grading.
struct ColorWheel: View {
    @Binding var hue: Double // degrees
    @Binding var sat: Double // 0...1
    var body: some View {
        GeometryReader { geo in
            let d = min(geo.size.width, geo.size.height)
            let r = d / 2
            let c = CGPoint(x: geo.size.width / 2, y: geo.size.height / 2)
            ZStack {
                Circle().fill(AngularGradient(colors: stride(from: 0.0, through: 360.0, by: 30.0).map { Color(hue: $0 / 360, saturation: 1, brightness: 1) },
                                              center: .center))
                    .frame(width: d, height: d)
                Circle().fill(RadialGradient(colors: [Color(white: 0.5), Color(white: 0.5).opacity(0)], center: .center, startRadius: 0, endRadius: r))
                    .frame(width: d, height: d)
                let a = CGFloat(hue * .pi / 180)
                let s = CGFloat(sat) * r
                Circle().stroke(.white, lineWidth: 2.5).frame(width: 16, height: 16)
                    .shadow(radius: 2)
                    .position(x: c.x + cos(a) * s, y: c.y + sin(a) * s)
            }
            .contentShape(Circle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                let dx = v.location.x - c.x, dy = v.location.y - c.y
                var deg = Double(atan2(dy, dx)) * 180 / .pi
                if deg < 0 { deg += 360 }
                hue = deg
                sat = min(1, Double(hypot(dx, dy) / r))
            })
        }
    }
}

// MARK: - histogram

struct HistogramView: View {
    let histogram: Histogram

    var body: some View {
        Canvas { ctx, size in
            let peak = Double(max(histogram.peak, 1))
            let colors: [Color] = [.red, .green, .blue]
            for ch in 0..<3 {
                var p = Path()
                p.move(to: CGPoint(x: 0, y: size.height))
                for i in 0..<256 {
                    let v = CGFloat(min(1, Double(histogram.bins[ch * 256 + i]) / peak))
                    p.addLine(to: CGPoint(x: size.width * CGFloat(i) / 255, y: size.height * (1 - v)))
                }
                p.addLine(to: CGPoint(x: size.width, y: size.height))
                p.closeSubpath()
                ctx.blendMode = .screen
                ctx.fill(p, with: .color(colors[ch].opacity(0.55)))
            }
            ctx.blendMode = .normal
            if histogram.clippedShadows > 0.002 {
                ctx.fill(Path(CGRect(x: 0, y: 0, width: 4, height: size.height)), with: .color(.cyan))
            }
            if histogram.clippedHighlights > 0.002 {
                ctx.fill(Path(CGRect(x: size.width - 4, y: 0, width: 4, height: size.height)), with: .color(.orange))
            }
        }
        .frame(width: 150, height: 64)
        .padding(6)
        .background(.black.opacity(0.55), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .allowsHitTesting(false)
    }
}
