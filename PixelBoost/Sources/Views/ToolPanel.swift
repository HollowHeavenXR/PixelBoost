import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Controls for the selected tool. Each tool describes its edit as one JSON
/// op; the panel previews it live and applies it at full size. Newer tools
/// live in their own panel views and hand their op over via
/// `editor.pendingOp`.
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
    @State private var adjustTab = "light"
    @State private var brightness = 0.0
    @State private var contrast = 1.0
    @State private var saturation = 1.0
    @State private var exposure = 0.0
    @State private var warmth = 0.0
    @State private var tint = 0.0
    @State private var vibrance = 0.0
    @State private var highlights = 0.0
    @State private var shadows = 0.0
    @State private var whites = 0.0
    @State private var blacks = 0.0
    @State private var clarity = 0.0
    @State private var texture = 0.0
    @State private var dehaze = 0.0
    @State private var fade = 0.0
    @State private var grain = 0.0
    @State private var grainSize = 0.3
    @State private var vignette = 0.0
    @State private var curve: [Double] = [0, 0.25, 0.5, 0.75, 1]
    // Local adjustments
    @State private var local = LocalAdjust()
    @State private var invert = false
    // Looks
    @State private var look = "vivid"
    @State private var amount = 1.0
    // Crop / perspective
    @State private var straighten = 0.0
    @State private var vertical = 0.0
    @State private var horizontal = 0.0
    // Restore / skin
    @State private var denoise = 0.0
    @State private var sharpen = 0.0
    @State private var smoothAmount = 0.5
    @State private var smoothRadius = 0.5
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
                .frame(maxHeight: 260)
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
        .onChange(of: editor.pickedPoint) { _, p in if let p { picked(p) } }
        .onAppear { setupHandles() }
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

    private func setupHandles() {
        switch tool {
        case .gradient:
            editor.p1 = CGPoint(x: 0.5, y: 0.05)
            editor.p2 = CGPoint(x: 0.5, y: 0.55)
        case .radial:
            editor.p1 = CGPoint(x: 0.5, y: 0.45)
            editor.radius = CGSize(width: 0.28, height: 0.3)
        default:
            break
        }
    }

    // MARK: op

    private var opKey: String { op ?? "" }

    private var adjustFields: [String: Any] {
        ["brightness": brightness, "contrast": contrast, "saturation": saturation, "exposure": exposure,
         "warmth": warmth, "tint": tint, "vibrance": vibrance, "highlights": highlights, "shadows": shadows,
         "whites": whites, "blacks": blacks, "clarity": clarity, "texture": texture, "dehaze": dehaze,
         "fade": fade, "grain": grain, "grain_size": grainSize, "curve": curve, "vignette": vignette]
    }

    private func localOp(_ mask: [String: Any]) -> String? {
        if local.isNeutral { return nil }
        let m: [String: Any] = invert ? ["kind": "invert", "mask": mask] : mask
        return opJSON("local", ["mask": m, "ops": local.ops])
    }

    private var op: String? {
        switch tool {
        case .adjust:
            return opJSON("adjust", adjustFields)
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
        case .perspective:
            if abs(vertical) < 0.005 && abs(horizontal) < 0.005 { return nil }
            return opJSON("perspective", ["vertical": vertical, "horizontal": horizontal])
        case .restore:
            var ops: [[String: Any]] = []
            if denoise > 0.01 { ops.append(opDict("denoise", ["amount": denoise])) }
            if sharpen > 0.01 { ops.append(opDict("sharpen", ["amount": sharpen, "radius": 1.0])) }
            return ops.isEmpty ? nil : opJSON("chain", ["ops": ops])
        case .selective:
            guard let m = maskFile() else { return nil }
            return localOp(["kind": "file", "path": m.path])
        case .gradient:
            return localOp(["kind": "linear", "x0": editor.p1.x, "y0": editor.p1.y, "x1": editor.p2.x, "y1": editor.p2.y])
        case .radial:
            return localOp(["kind": "radial", "cx": editor.p1.x, "cy": editor.p1.y, "rx": editor.radius.width, "ry": editor.radius.height, "feather": 0.6])
        case .smooth:
            let s = opDict("smooth", ["amount": smoothAmount, "radius": 0.003 + smoothRadius * 0.012])
            if let m = maskFile() {
                return opJSON("local", ["mask": ["kind": "feather", "mask": ["kind": "file", "path": m.path], "radius": 0.004], "ops": [s]])
            }
            return opJSON("chain", ["ops": [s]])
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
            return opJSON("depth_fog", ["path": m.path, "color": rgb(fogColor), "density": fogDensity, "start": fogStart])
        case .presets, .curves, .hsl, .grade, .blur, .fx, .text, .draw, .layer, .frame:
            return editor.pendingOp
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
        session.apply(op, to: app, name: tool.title)
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

    /// Eyedropper result.
    private func picked(_ p: CGPoint) {
        defer { editor.picking = nil; editor.pickedPoint = nil }
        guard let base = session.base else { return }
        if editor.picking == .balance {
            let wb = Core.engine.pickBalance(image: base, x: Float(p.x), y: Float(p.y))
            warmth = Double(wb.warmth)
            tint = Double(wb.tint)
        }
    }

    // MARK: controls

    @ViewBuilder
    private var controls: some View {
        switch tool {
        case .adjust:
            adjustControls
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
                    Button { session.apply(opJSON("rotate", ["clockwise": false]), to: app, name: "Rotate") } label: { Image(systemName: "rotate.left") }
                    Button { session.apply(opJSON("rotate", ["clockwise": true]), to: app, name: "Rotate") } label: { Image(systemName: "rotate.right") }
                    Button { session.apply(opJSON("flip", ["horizontal": true]), to: app, name: "Flip") } label: { Image(systemName: "arrow.left.and.right.righttriangle.left.righttriangle.right") }
                    Button { session.apply(opJSON("flip", ["horizontal": false]), to: app, name: "Flip") } label: { Image(systemName: "arrow.up.and.down.righttriangle.up.righttriangle.down") }
                }
                .buttonStyle(.bordered)
                ChipPicker(options: Self.aspects, selection: aspectBinding)
                SliderRow(label: "Straighten", value: $straighten, range: -45...45, format: { String(format: "%.1f°", $0) })
            }
        case .perspective:
            VStack(spacing: 10) {
                Text("Line up verticals with the grid.").font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Vertical", value: $vertical, range: -1...1)
                SliderRow(label: "Horizontal", value: $horizontal, range: -1...1)
                Button("Reset") { vertical = 0; horizontal = 0 }.font(.caption)
            }
        case .restore:
            VStack(spacing: 10) {
                SliderRow(label: "Denoise", value: $denoise)
                SliderRow(label: "Sharpen (halo-free)", value: $sharpen)
            }
        case .selective:
            VStack(spacing: 10) {
                brushControls
                Toggle("Invert mask", isOn: $invert)
                LocalAdjustControls(value: $local)
            }
        case .gradient, .radial:
            VStack(spacing: 10) {
                Text(tool == .gradient ? "Drag the two handles: full effect at the big one, fading out to the small one."
                                       : "Drag inside to move, use the handles to size it.")
                    .font(.caption).foregroundStyle(Theme.dim)
                Toggle(tool == .radial ? "Affect outside" : "Invert", isOn: $invert)
                LocalAdjustControls(value: $local)
            }
        case .smooth:
            VStack(spacing: 10) {
                Text(editor.strokes.isEmpty ? "Smooths the whole photo — or paint over skin to smooth only there." : "Smoothing only where you painted.")
                    .font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Smoothness", value: $smoothAmount)
                SliderRow(label: "Size", value: $smoothRadius)
                brushControls
            }
        case .heal:
            VStack(spacing: 10) {
                brushControls
                Text("Paint over what should disappear.").font(.caption).foregroundStyle(Theme.dim)
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
        case .presets:
            PresetsPanel(session: session, editor: editor)
        case .curves:
            CurvesPanel(editor: editor)
        case .hsl:
            HslPanel(editor: editor)
        case .grade:
            GradePanel(editor: editor)
        case .blur:
            BlurPanel(editor: editor)
        case .fx:
            FxPanel(editor: editor)
        case .text:
            TextPanel(editor: editor)
        case .draw:
            DrawPanel(editor: editor)
        case .layer:
            LayerPanel(editor: editor)
        case .frame:
            FramePanel(editor: editor)
        }
    }

    private var adjustControls: some View {
        VStack(spacing: 10) {
            ChipPicker(options: [("light", "Light"), ("colour", "Colour"), ("effects", "Effects"), ("curve", "Curve")], selection: $adjustTab)
            switch adjustTab {
            case "colour":
                HStack {
                    Button {
                        editor.picking = editor.picking == .balance ? nil : .balance
                    } label: {
                        Label(editor.picking == .balance ? "Tap something white or grey…" : "Pick white balance", systemImage: "eyedropper")
                    }
                    .buttonStyle(.bordered)
                    .tint(editor.picking == .balance ? Theme.amber : .white)
                    Spacer()
                }
                SliderRow(label: "Temperature", value: $warmth, range: -1...1)
                SliderRow(label: "Tint", value: $tint, range: -1...1)
                SliderRow(label: "Vibrance", value: $vibrance, range: -1...1)
                SliderRow(label: "Saturation", value: $saturation, range: 0...2)
            case "effects":
                SliderRow(label: "Clarity", value: $clarity, range: -1...1)
                SliderRow(label: "Texture", value: $texture, range: -1...1)
                SliderRow(label: "Dehaze", value: $dehaze, range: -1...1)
                SliderRow(label: "Vignette", value: $vignette, range: -1...1)
                SliderRow(label: "Fade", value: $fade)
                SliderRow(label: "Grain", value: $grain)
                SliderRow(label: "Grain size", value: $grainSize)
            case "curve":
                CurveEditor(points: $curve)
            default:
                SliderRow(label: "Exposure", value: $exposure, range: -2...2)
                SliderRow(label: "Brightness", value: $brightness, range: -0.5...0.5)
                SliderRow(label: "Contrast", value: $contrast, range: 0.5...1.5)
                SliderRow(label: "Highlights", value: $highlights, range: -1...1)
                SliderRow(label: "Shadows", value: $shadows, range: -1...1)
                SliderRow(label: "Whites", value: $whites, range: -1...1)
                SliderRow(label: "Blacks", value: $blacks, range: -1...1)
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
        (0, "Free"), (1, "1:1"), (0.8, "4:5"), (1.25, "5:4"), (1.5, "3:2"), (2.0 / 3.0, "2:3"),
        (4.0 / 3.0, "4:3"), (0.75, "3:4"), (16.0 / 9.0, "16:9"), (9.0 / 16.0, "9:16"), (2.39, "2.39:1"),
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

/// Five-point tone curve: drag the points.
struct CurveEditor: View {
    @Binding var points: [Double]
    var tint: Color = Theme.accent
    var title = "Curve"
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline)
            GeometryReader { geo in
                let w = geo.size.width, h = geo.size.height
                ZStack {
                    RoundedRectangle(cornerRadius: 8).fill(Theme.panel2)
                    Path { p in
                        p.move(to: CGPoint(x: 0, y: h))
                        p.addLine(to: CGPoint(x: w, y: 0))
                    }
                    .stroke(.white.opacity(0.15), lineWidth: 1)
                    Path { p in
                        for i in 0..<points.count {
                            let pt = CGPoint(x: w * CGFloat(i) / CGFloat(points.count - 1), y: h * (1 - points[i]))
                            if i == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
                        }
                    }
                    .stroke(tint, lineWidth: 2)
                    ForEach(0..<points.count, id: \.self) { i in
                        Circle().fill(.white).frame(width: 16, height: 16)
                            .position(x: w * CGFloat(i) / CGFloat(points.count - 1), y: h * (1 - points[i]))
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
