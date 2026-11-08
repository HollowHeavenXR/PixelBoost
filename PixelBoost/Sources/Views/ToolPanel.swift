import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Controls for the selected tool. Each tool describes its edit as one JSON
/// op; the panel previews it live and applies it at full size.
struct ToolPanel: View {
    let tool: EditTool
    @ObservedObject var session: ToolSession
    @ObservedObject var editor: EditorState
    let close: () -> Void
    @EnvironmentObject var app: AppModel

    init(tool: EditTool, session: ToolSession, editor: EditorState, close: @escaping () -> Void) {
        self.tool = tool
        self.session = session
        self.editor = editor
        self.close = close
    }

    // Adjust
    @State private var brightness = 0.0
    @State private var contrast = 1.0
    @State private var saturation = 1.0
    @State private var exposure = 0.0
    @State private var warmth = 0.0
    @State private var highlights = 0.0
    @State private var shadows = 0.0
    @State private var vignette = 0.0
    @State private var curve: [Double] = [0, 0.25, 0.5, 0.75, 1]
    // Looks
    @State private var look = "vivid"
    @State private var amount = 1.0
    // Crop
    @State private var straighten = 0.0
    // Restore
    @State private var denoise = 0.0
    @State private var sharpen = 0.0
    // Pixel art
    @State private var cell = 8.0
    @State private var colors = 16.0
    @State private var dither = false
    @State private var outline = false
    // LUT / maps
    @State private var lutURL: URL?
    @State private var mapURL: URL?
    @State private var showImporter = false
    @State private var fogDensity = 0.5
    @State private var fogStart = 0.2
    @State private var fogColor = Color(white: 0.85)
    @State private var mapMode = "ao"
    // Texture
    @State private var textureMode = "seamless"
    @State private var strength = 2.0
    @State private var blend = 0.25
    // Cutout
    @State private var maskURL: URL?
    @State private var cutBusy = false
    @State private var background = "none"

    var body: some View {
        VStack(spacing: 12) {
            ScrollView { controls.frame(maxWidth: .infinity) }
                .frame(maxHeight: 250)
            HStack {
                Button("Cancel") { close() }.buttonStyle(.bordered)
                Spacer()
                if needsApply {
                    Button { applyNow() } label: { Text("Apply").fontWeight(.semibold).padding(.horizontal, 10) }
                        .buttonStyle(.borderedProminent).disabled(op == nil || session.busy)
                } else {
                    Button("Done") { close() }.buttonStyle(.borderedProminent)
                }
            }
        }
        .onChange(of: opKey) { _, _ in session.preview(op) }
        .onChange(of: editor.strokes) { _, _ in
            renderStrokes()
            session.preview(op)
        }
        .onChange(of: editor.tap) { _, p in if p != nil { makeCutout(at: p) } }
        .fileImporter(isPresented: $showImporter, allowedContentTypes: tool == .lut ? [UTType(filenameExtension: "cube") ?? .data] : [.image]) { r in
            guard case .success(let url) = r else { return }
            if tool == .lut {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let dest = Core.newFile("cube")
                try? FileManager.default.copyItem(at: url, to: dest)
                lutURL = dest
            } else {
                mapURL = try? PhotoIO.importURL(url)
            }
        }
    }

    private var needsApply: Bool { tool != .clone }

    // MARK: op

    private var opKey: String { op ?? "" }

    private var op: String? {
        switch tool {
        case .adjust:
            return opJSON("adjust", ["brightness": brightness, "contrast": contrast, "saturation": saturation, "exposure": exposure,
                                     "warmth": warmth, "highlights": highlights, "shadows": shadows, "curve": curve, "vignette": vignette])
        case .looks:
            return opJSON("look", ["name": look, "amount": amount])
        case .enhance:
            return opJSON("auto_enhance")
        case .crop:
            var ops: [[String: Any]] = []
            if abs(straighten) > 0.05 { ops.append(opDict("straighten", ["degrees": straighten])) }
            let c = editor.crop
            if c.width < 0.999 || c.height < 0.999 {
                ops.append(opDict("crop", ["x": c.minX, "y": c.minY, "w": c.width, "h": c.height]))
            }
            return ops.isEmpty ? nil : opJSON("chain", ["ops": ops])
        case .restore:
            var ops: [[String: Any]] = []
            if denoise > 0.01 { ops.append(opDict("denoise", ["amount": denoise])) }
            if sharpen > 0.01 { ops.append(opDict("sharpen", ["amount": sharpen, "radius": 1.0])) }
            return ops.isEmpty ? nil : opJSON("chain", ["ops": ops])
        case .selective:
            guard let m = maskFile() else { return nil }
            let adj = opDict("adjust", ["brightness": brightness, "contrast": contrast, "saturation": saturation, "exposure": exposure,
                                        "warmth": warmth, "highlights": highlights, "shadows": shadows, "curve": [0, 0.25, 0.5, 0.75, 1], "vignette": 0])
            return opJSON("masked", ["mask": m.path, "ops": [adj]])
        case .heal:
            guard let m = maskFile() else { return nil }
            return opJSON("inpaint", ["mask": m.path])
        case .clone:
            return nil
        case .cutout:
            guard let m = maskURL else { return nil }
            var ops: [[String: Any]] = [opDict("mask_alpha", ["mask": m.path])]
            if let t = touchUpMask(), let orig = app.original {
                ops.append(opDict("touch_up", ["original": orig.path, "mask": t.path, "restore": !editor.erase]))
            }
            switch background {
            case "white": ops.append(opDict("fill", ["top": [1, 1, 1], "bottom": [1, 1, 1]]))
            case "black": ops.append(opDict("fill", ["top": [0, 0, 0], "bottom": [0, 0, 0]]))
            case "sunset": ops.append(opDict("fill", ["top": [1.0, 0.55, 0.3], "bottom": [0.35, 0.15, 0.45]]))
            case "studio": ops.append(opDict("fill", ["top": [0.32, 0.34, 0.38], "bottom": [0.12, 0.12, 0.14]]))
            case "sky": ops.append(opDict("fill", ["top": [0.45, 0.7, 1.0], "bottom": [0.88, 0.94, 1.0]]))
            case "blur":
                if let orig = app.original { ops.append(opDict("blurred_background", ["path": orig.path, "radius": 40])) }
            default: break
            }
            return opJSON("chain", ["ops": ops])
        case .pixelArt:
            return opJSON("pixel_art", ["cell": Int(cell), "colors": Int(colors), "dither": dither, "outline": outline])
        case .lut:
            guard let l = lutURL else { return nil }
            return opJSON("lut", ["path": l.path, "amount": amount])
        case .texture:
            if textureMode == "normal" { return opJSON("normal_map", ["strength": strength]) }
            return opJSON("seamless", ["blend": blend])
        case .maps:
            guard let m = mapURL else { return nil }
            if mapMode == "ao" { return opJSON("ao_blend", ["path": m.path, "amount": amount]) }
            let c = UIColor(fogColor)
            var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
            c.getRed(&r, green: &g, blue: &b, alpha: &a)
            return opJSON("depth_fog", ["path": m.path, "color": [r, g, b], "density": fogDensity, "start": fogStart])
        }
    }

    /// Mask rendered from the current strokes (re-rendered when they change).
    @State private var strokeMask: URL?

    private func maskFile() -> URL? { strokeMask }

    private func touchUpMask() -> URL? { editor.brushActive ? strokeMask : nil }

    private func renderStrokes() {
        let draw = editor.strokes
        guard !draw.isEmpty, let b = session.base else { strokeMask = nil; return }
        strokeMask = MaskRenderer.render(draw, size: CGSize(width: Int(b.width), height: Int(b.height)))
    }

    private func applyNow() {
        guard let op else { return }
        session.apply(op, to: app, name: tool.rawValue)
        close()
    }

    private func makeCutout(at p: CGPoint?) {
        guard let src = app.current else { return }
        cutBusy = true
        Task.detached {
            let r = Result { try SubjectMask.make(for: src, at: p) }
            await MainActor.run {
                cutBusy = false
                switch r {
                case .success(let u): maskURL = u
                case .failure(let e): app.message = describe(e)
                }
            }
        }
    }

    // MARK: controls

    @ViewBuilder
    private var controls: some View {
        switch tool {
        case .adjust:
            VStack(spacing: 10) {
                SliderRow(label: "Exposure", value: $exposure, range: -2...2)
                SliderRow(label: "Brightness", value: $brightness, range: -0.5...0.5)
                SliderRow(label: "Contrast", value: $contrast, range: 0.5...1.5)
                SliderRow(label: "Saturation", value: $saturation, range: 0...2)
                SliderRow(label: "Warmth", value: $warmth, range: -1...1)
                SliderRow(label: "Highlights", value: $highlights, range: -1...1)
                SliderRow(label: "Shadows", value: $shadows, range: -1...1)
                SliderRow(label: "Vignette", value: $vignette)
                CurveEditor(points: $curve)
            }
        case .looks:
            VStack(spacing: 10) {
                ChipPicker(options: Core.engine.looks().map { ($0, $0.capitalized) }, selection: $look)
                SliderRow(label: "Amount", value: $amount)
            }
        case .enhance:
            Text("One-tap levels and colour balance.").foregroundStyle(Theme.dim)
        case .crop:
            VStack(spacing: 10) {
                HStack {
                    Button { session.apply(opJSON("rotate", ["clockwise": false]), to: app, name: "rotate") } label: { Image(systemName: "rotate.left") }
                    Button { session.apply(opJSON("rotate", ["clockwise": true]), to: app, name: "rotate") } label: { Image(systemName: "rotate.right") }
                    Button { session.apply(opJSON("flip", ["horizontal": true]), to: app, name: "flip") } label: { Image(systemName: "arrow.left.and.right.righttriangle.left.righttriangle.right") }
                    Button { session.apply(opJSON("flip", ["horizontal": false]), to: app, name: "flip") } label: { Image(systemName: "arrow.up.and.down.righttriangle.up.righttriangle.down") }
                }
                .buttonStyle(.bordered)
                ChipPicker(options: Self.aspects, selection: aspectBinding)
                SliderRow(label: "Straighten", value: $straighten, range: -45...45, format: { String(format: "%.1f°", $0) })
            }
        case .restore:
            VStack(spacing: 10) {
                SliderRow(label: "Denoise", value: $denoise)
                SliderRow(label: "Sharpen (halo-free)", value: $sharpen)
            }
        case .selective, .heal:
            VStack(spacing: 10) {
                brushControls
                if tool == .selective {
                    SliderRow(label: "Exposure", value: $exposure, range: -2...2)
                    SliderRow(label: "Contrast", value: $contrast, range: 0.5...1.5)
                    SliderRow(label: "Saturation", value: $saturation, range: 0...2)
                    SliderRow(label: "Warmth", value: $warmth, range: -1...1)
                } else {
                    Text("Paint over what should disappear.").font(.caption).foregroundStyle(Theme.dim)
                }
            }
        case .clone:
            VStack(spacing: 10) {
                Text(editor.cloneSetting ? "Tap the spot to copy from." : "Tap where to paint it. Each tap is one stroke.")
                    .foregroundStyle(Theme.dim)
                SliderRow(label: "Brush", value: $editor.brush, range: 0.005...0.15)
                Button("Pick a new source") { editor.cloneSetting = true }.buttonStyle(.bordered)
            }
        case .cutout:
            VStack(spacing: 10) {
                HStack {
                    Button { makeCutout(at: nil) } label: { Label("All subjects", systemImage: "person.2") }
                    Text("or tap one").foregroundStyle(Theme.dim)
                    if cutBusy { ProgressView() }
                }
                .buttonStyle(.bordered)
                if maskURL != nil {
                    Toggle("Touch up with brush", isOn: $editor.brushActive)
                    if editor.brushActive { brushControls }
                    ChipPicker(options: [("none", "Transparent"), ("white", "White"), ("black", "Black"), ("studio", "Studio"),
                                         ("sky", "Sky"), ("sunset", "Sunset"), ("blur", "Blurred")], selection: $background)
                }
            }
        case .pixelArt:
            VStack(spacing: 10) {
                SliderRow(label: "Pixel size", value: $cell, range: 2...32, format: { "\(Int($0))px" })
                SliderRow(label: "Colours", value: $colors, range: 2...64, format: { "\(Int($0))" })
                Toggle("Dither", isOn: $dither)
                Toggle("Outline", isOn: $outline)
            }
        case .lut:
            VStack(spacing: 10) {
                Button { showImporter = true } label: { Label(lutURL == nil ? "Import .cube LUT" : "Replace LUT", systemImage: "square.and.arrow.down") }
                    .buttonStyle(.bordered)
                SliderRow(label: "Amount", value: $amount)
            }
        case .texture:
            VStack(spacing: 10) {
                ChipPicker(options: [("seamless", "Seamless tile"), ("normal", "Normal map")], selection: $textureMode)
                if textureMode == "normal" {
                    SliderRow(label: "Strength", value: $strength, range: 0.5...8)
                } else {
                    SliderRow(label: "Blend width", value: $blend, range: 0.05...0.5)
                }
            }
        case .maps:
            VStack(spacing: 10) {
                ChipPicker(options: [("ao", "Ambient occlusion"), ("fog", "Depth fog")], selection: $mapMode)
                Button { showImporter = true } label: { Label(mapURL == nil ? "Choose \(mapMode == "ao" ? "AO" : "depth") map" : "Replace map", systemImage: "photo") }
                    .buttonStyle(.bordered)
                if mapMode == "ao" {
                    SliderRow(label: "Amount", value: $amount)
                } else {
                    SliderRow(label: "Density", value: $fogDensity)
                    SliderRow(label: "Start", value: $fogStart, range: 0...0.95)
                    ColorPicker("Fog colour", selection: $fogColor, supportsOpacity: false)
                }
            }
        }
    }

    private var brushControls: some View {
        VStack(spacing: 8) {
            SliderRow(label: "Brush", value: $editor.brush, range: 0.005...0.15)
            HStack {
                Picker("", selection: $editor.erase) {
                    Text(tool == .cutout ? "Restore" : "Paint").tag(false)
                    Text("Erase").tag(true)
                }
                .pickerStyle(.segmented)
                Button { _ = editor.strokes.popLast() } label: { Image(systemName: "arrow.uturn.backward") }.disabled(editor.strokes.isEmpty)
                Button { editor.strokes.removeAll() } label: { Image(systemName: "trash") }.disabled(editor.strokes.isEmpty)
            }
        }
    }

    static let aspects: [(CGFloat, String)] = [
        (0, "Free"), (1, "1:1"), (0.8, "4:5"), (1.25, "5:4"), (16.0 / 9.0, "16:9"), (9.0 / 16.0, "9:16"),
    ]

    private var aspectBinding: Binding<CGFloat> {
        Binding(get: { editor.cropAspect ?? 0 }, set: { a in setAspect(a) })
    }

    private func setAspect(_ a: CGFloat) {
        guard a > 0, let size = app.preview?.size, size.width > 0 else {
            editor.cropAspect = nil
            editor.crop = CGRect(x: 0, y: 0, width: 1, height: 1)
            return
        }
        editor.cropAspect = a
        let px = size.width / size.height
        var w: CGFloat = 1, h = px / a
        if h > 1 { h = 1; w = a / px }
        editor.crop = CGRect(x: (1 - w) / 2, y: (1 - h) / 2, width: w, height: h)
    }
}

/// Five-point tone curve: vertical sliders at fixed inputs.
struct CurveEditor: View {
    @Binding var points: [Double]
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Curve").font(.subheadline)
            GeometryReader { geo in
                let w = geo.size.width, h = geo.size.height
                ZStack {
                    RoundedRectangle(cornerRadius: 8).fill(Theme.panel2)
                    Path { p in
                        for i in 0..<5 {
                            let pt = CGPoint(x: w * CGFloat(i) / 4, y: h * (1 - points[i]))
                            if i == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
                        }
                    }
                    .stroke(Theme.accent, lineWidth: 2)
                    ForEach(0..<5, id: \.self) { i in
                        Circle().fill(.white).frame(width: 16, height: 16)
                            .position(x: w * CGFloat(i) / 4, y: h * (1 - points[i]))
                            .gesture(DragGesture().onChanged { v in
                                points[i] = min(1, max(0, 1 - v.location.y / h))
                            })
                    }
                }
            }
            .frame(height: 120)
            Button("Reset curve") { points = [0, 0.25, 0.5, 0.75, 1] }.font(.caption)
        }
    }
}
