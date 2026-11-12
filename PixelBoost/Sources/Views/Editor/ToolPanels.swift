import PhotosUI
import SwiftUI
import UIKit

/// Parse an op JSON string back into a dictionary (to nest it in another op).
func jsonObject(_ s: String) -> Any? {
    try? JSONSerialization.jsonObject(with: Data(s.utf8))
}

private extension View {
    /// Hand the panel's op to the editor whenever it changes.
    @MainActor
    func publishes(_ op: String?, to editor: EditorState) -> some View {
        onChange(of: op, initial: true) { _, v in editor.pendingOp = v }
    }
}

// MARK: - Presets

struct PresetsPanel: View {
    @ObservedObject var session: ToolSession
    @ObservedObject var editor: EditorState
    @EnvironmentObject var app: AppModel
    @State private var presets: [PresetEntry] = Core.presets.list()
    @State private var thumbs: [String: UIImage] = [:]
    @State private var selected: String?
    @State private var amount = 1.0
    @State private var naming = false
    @State private var name = ""

    private var op: String? {
        guard let id = selected, let p = presets.first(where: { $0.id == id }) else { return nil }
        if amount >= 0.999 { return p.opJson }
        guard let inner = jsonObject(p.opJson) else { return nil }
        return opJSON("local", ["mask": ["kind": "all"], "ops": [inner], "amount": amount])
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 10) {
                    ForEach(presets, id: \.id) { p in
                        Button { selected = selected == p.id ? nil : p.id } label: {
                            VStack(spacing: 4) {
                                ZStack {
                                    RoundedRectangle(cornerRadius: 12).fill(Theme.card2)
                                    if let t = thumbs[p.id] {
                                        Image(uiImage: t).resizable().scaledToFill()
                                    } else {
                                        ProgressView()
                                    }
                                }
                                .frame(width: 76, height: 76)
                                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                                .overlay(RoundedRectangle(cornerRadius: 12).stroke(selected == p.id ? Theme.amber : .clear, lineWidth: 3))
                                Text(p.name).font(.caption2).lineLimit(1).frame(width: 76)
                                    .foregroundStyle(p.builtin ? Color.white : Theme.amber)
                            }
                        }
                        .buttonStyle(.plain)
                        .contextMenu {
                            if !p.builtin {
                                Button(role: .destructive) {
                                    try? Core.presets.delete(id: p.id)
                                    presets = Core.presets.list()
                                    if selected == p.id { selected = nil }
                                } label: { Label("Delete Preset", systemImage: "trash") }
                            }
                        }
                    }
                }
            }
            if selected != nil { SliderRow(label: "Amount", value: $amount) }
            Button {
                if app.recipe == nil {
                    app.message = "Make some edits first — then save them here to reuse on other photos."
                } else {
                    name = ""
                    naming = true
                }
            } label: { Label("Save my edits as a preset", systemImage: "plus.square.on.square") }
                .buttonStyle(.bordered)
        }
        .publishes(op, to: editor)
        .task(id: session.base?.width ?? 0) { await renderThumbs() }
        .alert("New preset", isPresented: $naming) {
            TextField("Name", text: $name)
            Button("Save") { savePreset() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Saves the edits made to this photo (painted masks and layers stay behind).")
        }
    }

    private func savePreset() {
        guard let r = app.recipe else { return }
        do {
            let e = try Core.presets.save(name: name.isEmpty ? "My preset" : name, opJson: r)
            presets = Core.presets.list()
            thumbs[e.id] = nil
            Task { await renderThumbs() }
            app.flash("Preset saved")
        } catch { app.message = describe(error) }
    }

    private func renderThumbs() async {
        guard let base = session.base else { return }
        let list = presets.filter { thumbs[$0.id] == nil }
        for p in list {
            let img = await Task.detached(priority: .utility) {
                (try? Core.engine.editThumbnail(image: base, opJson: p.opJson, maxSide: 180))?.uiImage
            }.value
            if let img { thumbs[p.id] = img }
        }
    }
}

// MARK: - Curves

struct CurvesPanel: View {
    @ObservedObject var editor: EditorState
    @State private var channel = "master"
    @State private var master: [Double] = [0, 0.25, 0.5, 0.75, 1]
    @State private var red: [Double] = [0, 0.25, 0.5, 0.75, 1]
    @State private var green: [Double] = [0, 0.25, 0.5, 0.75, 1]
    @State private var blue: [Double] = [0, 0.25, 0.5, 0.75, 1]

    private static let identity: [Double] = [0, 0.25, 0.5, 0.75, 1]

    private var op: String? {
        if [master, red, green, blue].allSatisfy({ $0 == Self.identity }) { return nil }
        return opJSON("curves", ["master": master, "red": red, "green": green, "blue": blue])
    }

    private var binding: Binding<[Double]> {
        switch channel {
        case "red": return $red
        case "green": return $green
        case "blue": return $blue
        default: return $master
        }
    }

    private var tint: Color {
        switch channel {
        case "red": return .red
        case "green": return .green
        case "blue": return Color(red: 0.3, green: 0.5, blue: 1)
        default: return Theme.accent
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            ChipPicker(options: [("master", "RGB"), ("red", "Red"), ("green", "Green"), ("blue", "Blue")], selection: $channel)
            CurveEditor(points: binding, tint: tint, title: channel == "master" ? "Tone curve" : "\(channel.capitalized) curve")
        }
        .publishes(op, to: editor)
    }
}

// MARK: - HSL colour mixer

struct HslPanel: View {
    @ObservedObject var editor: EditorState
    @State private var band = 0
    @State private var hue = [Double](repeating: 0, count: 8)
    @State private var sat = [Double](repeating: 0, count: 8)
    @State private var lum = [Double](repeating: 0, count: 8)

    static let bands: [(String, Color)] = [
        ("Red", .red), ("Orange", .orange), ("Yellow", .yellow), ("Green", .green),
        ("Aqua", .cyan), ("Blue", .blue), ("Purple", .purple), ("Magenta", Color(red: 1, green: 0.2, blue: 0.7)),
    ]

    private var op: String? {
        if (hue + sat + lum).allSatisfy({ abs($0) < 0.001 }) { return nil }
        return opJSON("hsl", ["hue": hue, "sat": sat, "lum": lum])
    }

    var body: some View {
        VStack(spacing: 12) {
            HStack(spacing: 8) {
                ForEach(0..<8, id: \.self) { i in
                    Button { band = i } label: {
                        Circle().fill(Self.bands[i].1).frame(width: 28, height: 28)
                            .overlay(Circle().stroke(.white, lineWidth: band == i ? 3 : 0))
                            .overlay(alignment: .topTrailing) {
                                if abs(hue[i]) + abs(sat[i]) + abs(lum[i]) > 0.001 {
                                    Circle().fill(Theme.amber).frame(width: 7, height: 7)
                                }
                            }
                    }
                    .buttonStyle(.plain)
                }
            }
            Text(Self.bands[band].0).font(.subheadline.weight(.semibold)).frame(maxWidth: .infinity, alignment: .leading)
            SliderRow(label: "Hue", value: $hue[band], range: -1...1)
            SliderRow(label: "Saturation", value: $sat[band], range: -1...1)
            SliderRow(label: "Luminance", value: $lum[band], range: -1...1)
            Button("Reset all colours") {
                hue = [Double](repeating: 0, count: 8)
                sat = hue
                lum = hue
            }
            .font(.caption)
        }
        .publishes(op, to: editor)
    }
}

// MARK: - Colour grading

struct GradeWheel: Equatable {
    var hue = 0.0
    var sat = 0.0
    var lum = 0.0
    var dict: [String: Any] { ["hue": hue, "sat": sat, "lum": lum] }
}

struct GradePanel: View {
    @ObservedObject var editor: EditorState
    @State private var which = "shadows"
    @State private var shadows = GradeWheel()
    @State private var midtones = GradeWheel()
    @State private var highlights = GradeWheel()
    @State private var global = GradeWheel()
    @State private var balance = 0.0

    private var op: String? {
        if [shadows, midtones, highlights, global].allSatisfy({ $0 == GradeWheel() }) && abs(balance) < 0.001 { return nil }
        return opJSON("grade", ["shadows": shadows.dict, "midtones": midtones.dict, "highlights": highlights.dict,
                                "global": global.dict, "balance": balance])
    }

    private var wheel: Binding<GradeWheel> {
        switch which {
        case "midtones": return $midtones
        case "highlights": return $highlights
        case "global": return $global
        default: return $shadows
        }
    }

    var body: some View {
        VStack(spacing: 12) {
            ChipPicker(options: [("shadows", "Shadows"), ("midtones", "Midtones"), ("highlights", "Highlights"), ("global", "Global")], selection: $which)
            HStack(spacing: 16) {
                ColorWheel(hue: wheel.hue, sat: wheel.sat).frame(width: 130, height: 130)
                VStack(spacing: 8) {
                    SliderRow(label: "Hue", value: wheel.hue, range: 0...360, format: { "\(Int($0))°" })
                    SliderRow(label: "Strength", value: wheel.sat)
                    SliderRow(label: "Luminance", value: wheel.lum, range: -1...1)
                }
            }
            SliderRow(label: "Balance", value: $balance, range: -1...1)
            Button("Reset") {
                shadows = GradeWheel(); midtones = GradeWheel(); highlights = GradeWheel(); global = GradeWheel(); balance = 0
            }
            .font(.caption)
        }
        .publishes(op, to: editor)
    }
}

// MARK: - Blur

struct BlurPanel: View {
    @ObservedObject var editor: EditorState
    @EnvironmentObject var app: AppModel
    @State private var strength = 0.4
    @State private var feather = 0.2
    @State private var zoomAmount = 0.35
    @State private var spin = 15.0
    @State private var motionAngle = 0.0
    @State private var motionDistance = 0.3
    @State private var bokeh = 0.5
    @State private var subject: URL?
    @State private var finding = false

    private var op: String? {
        switch editor.blurMode {
        case "tilt":
            return opJSON("tilt_shift", ["y": editor.p1.y, "angle": editor.angle, "band": editor.band, "feather": feather, "radius": strength * 0.03])
        case "zoom":
            return opJSON("zoom_blur", ["cx": editor.p1.x, "cy": editor.p1.y, "amount": zoomAmount])
        case "spin":
            return opJSON("spin_blur", ["cx": editor.p1.x, "cy": editor.p1.y, "degrees": spin])
        case "motion":
            return opJSON("motion_blur", ["angle": motionAngle, "distance": motionDistance * 0.15])
        case "portrait":
            guard let s = subject else { return nil }
            return opJSON("lens_blur", ["radius": strength * 0.03, "bokeh": bokeh,
                                        "keep": ["kind": "feather", "mask": ["kind": "file", "path": s.path], "radius": 0.002]])
        case "lens":
            return opJSON("lens_blur", ["radius": strength * 0.03, "bokeh": bokeh])
        default:
            return strength < 0.005 ? nil : opJSON("blur", ["radius": strength * 0.03])
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            ChipPicker(options: [("gaussian", "Soft"), ("portrait", "Portrait"), ("lens", "Lens"), ("tilt", "Tilt-shift"),
                                 ("zoom", "Zoom"), ("spin", "Spin"), ("motion", "Motion")], selection: $editor.blurMode)
            switch editor.blurMode {
            case "tilt":
                Text("Drag on the photo to move the sharp band.").font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Blur", value: $strength)
                SliderRow(label: "Band", value: $editor.band, range: 0.02...0.8)
                SliderRow(label: "Feather", value: $feather, range: 0.02...0.6)
                SliderRow(label: "Angle", value: $editor.angle, range: -90...90, format: { "\(Int($0))°" })
            case "zoom":
                Text("Drag on the photo to move the centre.").font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Amount", value: $zoomAmount)
            case "spin":
                Text("Drag on the photo to move the centre.").font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Angle", value: $spin, range: 1...90, format: { "\(Int($0))°" })
            case "motion":
                SliderRow(label: "Direction", value: $motionAngle, range: 0...180, format: { "\(Int($0))°" })
                SliderRow(label: "Distance", value: $motionDistance)
            case "portrait":
                HStack {
                    if finding { ProgressView() }
                    Text(subject == nil ? (finding ? "Finding the subject…" : "No subject found.") : "Subject stays sharp; the background gets lens blur.")
                        .font(.caption).foregroundStyle(Theme.dim)
                    Spacer()
                }
                SliderRow(label: "Blur", value: $strength)
                SliderRow(label: "Bokeh highlights", value: $bokeh)
            case "lens":
                SliderRow(label: "Blur", value: $strength)
                SliderRow(label: "Bokeh highlights", value: $bokeh)
            default:
                SliderRow(label: "Blur", value: $strength)
            }
        }
        .publishes(op, to: editor)
        .onChange(of: editor.blurMode, initial: true) { _, m in
            if m == "tilt" { editor.p1 = CGPoint(x: 0.5, y: 0.55) }
            if m == "zoom" || m == "spin" { editor.p1 = CGPoint(x: 0.5, y: 0.5) }
            if m == "portrait", subject == nil { findSubject() }
        }
    }

    private func findSubject() {
        guard let src = app.current, !finding else { return }
        finding = true
        Task.detached {
            let r = try? SubjectMask.make(for: src)
            await MainActor.run { subject = r; finding = false }
        }
    }
}

// MARK: - Effects

struct FxPanel: View {
    @ObservedObject var editor: EditorState
    @State private var effect = "glow"
    @State private var amount = 1.0
    @State private var size = 0.5
    @State private var color1 = Color(red: 0.10, green: 0.08, blue: 0.25)
    @State private var color2 = Color(red: 1.0, green: 0.82, blue: 0.55)
    @State private var glowRadius = 0.4
    @State private var threshold = 0.7

    private static let names: [String: String] = [
        "glow": "Glow", "chromatic": "Chromatic", "halftone": "Halftone", "duotone": "Duotone", "posterize": "Posterize",
        "sketch": "Sketch", "glitch": "Glitch", "oil": "Oil Paint", "mosaic": "Mosaic", "invert": "Invert",
        "solarize": "Solarize", "threshold": "Threshold", "emboss": "Emboss", "light_leak": "Light Leak",
    ]

    private var options: [(String, String)] {
        (["glow"] + Core.engine.effects()).map { ($0, Self.names[$0] ?? $0.capitalized) }
    }

    private var usesColors: Bool { ["duotone", "halftone", "light_leak"].contains(effect) }

    private var op: String? {
        if effect == "glow" {
            return opJSON("bloom", ["amount": amount, "radius": 0.005 + glowRadius * 0.05, "threshold": threshold])
        }
        return opJSON("effect", ["name": effect, "amount": amount, "size": size, "colors": [rgb(color1), rgb(color2)]])
    }

    var body: some View {
        VStack(spacing: 10) {
            ChipPicker(options: options, selection: $effect)
            SliderRow(label: "Amount", value: $amount)
            if effect == "glow" {
                SliderRow(label: "Radius", value: $glowRadius)
                SliderRow(label: "Threshold", value: $threshold, range: 0.2...0.95)
            } else if !["invert"].contains(effect) {
                SliderRow(label: "Size", value: $size)
            }
            if usesColors {
                ColorPicker(effect == "light_leak" ? "Leak colour 1" : (effect == "halftone" ? "Ink" : "Shadows"), selection: $color1, supportsOpacity: false)
                ColorPicker(effect == "light_leak" ? "Leak colour 2" : (effect == "halftone" ? "Paper" : "Highlights"), selection: $color2, supportsOpacity: false)
            }
        }
        .publishes(op, to: editor)
    }
}

// MARK: - Text

struct TextPanel: View {
    @ObservedObject var editor: EditorState
    @State private var text = "Your text"
    @State private var font = "heavy"
    @State private var size = 0.1
    @State private var color = Color.white
    @State private var align = "center"
    @State private var outline = 0.0
    @State private var outlineColor = Color.black
    @State private var shadow = 0.4
    @State private var boxOn = false
    @State private var boxColor = Color.black.opacity(0.55)
    @State private var rotation = 0.0
    @State private var spacing = 0.0

    private var op: String? {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return nil }
        var bg = rgba(boxColor)
        if !boxOn { bg[3] = 0 }
        return opJSON("text", ["text": text, "x": editor.p1.x, "y": editor.p1.y, "size": size, "font": font,
                               "color": rgba(color), "align": align, "rotation": rotation, "outline": outline,
                               "outline_color": rgba(outlineColor), "shadow": shadow, "background": bg,
                               "letter_spacing": spacing, "line_height": 1.15])
    }

    var body: some View {
        VStack(spacing: 10) {
            TextField("Text", text: $text, axis: .vertical)
                .lineLimit(1...4)
                .padding(10)
                .background(Theme.card2, in: RoundedRectangle(cornerRadius: 10))
            Text("Drag on the photo to place it.").font(.caption).foregroundStyle(Theme.dim).frame(maxWidth: .infinity, alignment: .leading)
            ChipPicker(options: Core.engine.fonts().map { ($0.id, $0.name) }, selection: $font)
            SliderRow(label: "Size", value: $size, range: 0.02...0.35)
            HStack {
                ColorPicker("Colour", selection: $color)
                Picker("", selection: $align) {
                    Image(systemName: "text.alignleft").tag("left")
                    Image(systemName: "text.aligncenter").tag("center")
                    Image(systemName: "text.alignright").tag("right")
                }
                .pickerStyle(.segmented).frame(width: 140)
            }
            SliderRow(label: "Outline", value: $outline, range: 0...0.2)
            if outline > 0.001 { ColorPicker("Outline colour", selection: $outlineColor) }
            SliderRow(label: "Shadow", value: $shadow)
            Toggle("Background box", isOn: $boxOn)
            if boxOn { ColorPicker("Box colour", selection: $boxColor) }
            SliderRow(label: "Rotation", value: $rotation, range: -180...180, format: { "\(Int($0))°" })
            SliderRow(label: "Letter spacing", value: $spacing, range: -0.1...0.5)
        }
        .publishes(op, to: editor)
    }
}

// MARK: - Draw

struct DrawPanel: View {
    @ObservedObject var editor: EditorState

    private var op: String? {
        editor.marks.isEmpty ? nil : opJSON("draw", ["marks": editor.marks.map(\.dict)])
    }

    var body: some View {
        VStack(spacing: 10) {
            ChipPicker(options: [("pen", "Pen"), ("marker", "Marker"), ("highlighter", "Highlighter"), ("neon", "Neon"),
                                 ("line", "Line"), ("arrow", "Arrow"), ("rect", "Box"), ("ellipse", "Ellipse")],
                       selection: $editor.drawStyle)
            HStack {
                ColorPicker("Colour", selection: $editor.drawColor)
                if editor.drawStyle == "rect" || editor.drawStyle == "ellipse" {
                    Toggle("Fill", isOn: $editor.drawFill).frame(width: 110)
                }
            }
            SliderRow(label: "Width", value: $editor.drawWidth, range: 0.002...0.06)
            HStack {
                Button { _ = editor.marks.popLast() } label: { Label("Undo", systemImage: "arrow.uturn.backward") }.disabled(editor.marks.isEmpty)
                Spacer()
                Button(role: .destructive) { editor.marks.removeAll() } label: { Label("Clear", systemImage: "trash") }.disabled(editor.marks.isEmpty)
            }
            .buttonStyle(.bordered)
        }
        .publishes(op, to: editor)
    }
}

// MARK: - Image layer

struct LayerPanel: View {
    @ObservedObject var editor: EditorState
    @EnvironmentObject var app: AppModel
    @State private var item: PhotosPickerItem?
    @State private var layer: URL?
    @State private var fit = "free"
    @State private var size = 0.4
    @State private var rotation = 0.0
    @State private var opacity = 1.0
    @State private var blend = "normal"

    private var op: String? {
        guard let l = layer else { return nil }
        var d: [String: Any] = ["path": l.path, "x": editor.p1.x, "y": editor.p1.y, "w": size, "rotation": rotation,
                                "opacity": opacity, "blend": blend]
        if fit != "free" { d["fit"] = fit }
        return opJSON("overlay", d)
    }

    var body: some View {
        VStack(spacing: 10) {
            PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) {
                Label(layer == nil ? "Choose an image to layer" : "Replace layer image", systemImage: "photo.badge.plus")
            }
            .buttonStyle(.bordered)
            if layer != nil {
                ChipPicker(options: [("free", "Sticker"), ("cover", "Fill frame"), ("contain", "Fit frame")], selection: $fit)
                if fit == "free" {
                    Text("Drag on the photo to move it.").font(.caption).foregroundStyle(Theme.dim).frame(maxWidth: .infinity, alignment: .leading)
                    SliderRow(label: "Size", value: $size, range: 0.05...1.5)
                    SliderRow(label: "Rotation", value: $rotation, range: -180...180, format: { "\(Int($0))°" })
                }
                SliderRow(label: "Opacity", value: $opacity)
                ChipPicker(options: Core.engine.blendModes().map { ($0, $0.replacingOccurrences(of: "_", with: " ").capitalized) }, selection: $blend)
            } else {
                Text("Stickers, logos, textures or a second photo for a double exposure (try Fill frame + Screen).")
                    .font(.caption).foregroundStyle(Theme.dim)
            }
        }
        .publishes(op, to: editor)
        .onChange(of: item) { _, new in
            guard let new else { return }
            Task {
                do { layer = try await PhotoIO.importItem(new) } catch { app.message = describe(error) }
                item = nil
            }
        }
    }
}

// MARK: - Frame

struct FramePanel: View {
    @ObservedObject var editor: EditorState
    @State private var style = "border"
    @State private var width = 0.04
    @State private var color = Color.white
    @State private var radius = 0.08

    private var op: String? {
        switch style {
        case "pad11": return opJSON("frame", ["style": "pad", "ratio": 1.0, "width": width, "color": rgba(color)])
        case "pad45": return opJSON("frame", ["style": "pad", "ratio": 0.8, "width": width, "color": rgba(color)])
        case "pad916": return opJSON("frame", ["style": "pad", "ratio": 9.0 / 16.0, "width": width, "color": rgba(color)])
        case "rounded": return opJSON("frame", ["style": "rounded", "radius": radius])
        default: return opJSON("frame", ["style": style, "width": width, "color": rgba(color)])
        }
    }

    var body: some View {
        VStack(spacing: 10) {
            ChipPicker(options: [("border", "Border"), ("polaroid", "Polaroid"), ("film", "Film"), ("shadow", "Card"),
                                 ("pad11", "Fit 1:1"), ("pad45", "Fit 4:5"), ("pad916", "Fit 9:16"), ("rounded", "Rounded")],
                       selection: $style)
            if style == "rounded" {
                SliderRow(label: "Corner radius", value: $radius, range: 0.01...0.5)
                Text("Corners become transparent — export as PNG to keep them.").font(.caption).foregroundStyle(Theme.dim)
            } else {
                SliderRow(label: style.hasPrefix("pad") ? "Margin" : "Width", value: $width, range: 0...0.2)
                if style != "film" { ColorPicker("Colour", selection: $color, supportsOpacity: false) }
            }
        }
        .publishes(op, to: editor)
    }
}
