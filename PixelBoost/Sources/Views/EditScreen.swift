import SwiftUI

enum EditTool: String, CaseIterable, Identifiable {
    case presets, looks, lut
    case enhance, adjust, curves, hsl, grade
    case selective, gradient, radial
    case heal, clone, smooth, restore
    case crop, perspective
    case blur, fx
    case text, draw, layer, frame, cutout, pixelArt, texture, maps
    var id: String { rawValue }

    var title: String {
        switch self {
        case .presets: return "Presets"
        case .looks: return "Looks"
        case .lut: return "LUT"
        case .enhance: return "Auto"
        case .adjust: return "Adjust"
        case .curves: return "Curves"
        case .hsl: return "Colour Mix"
        case .grade: return "Grading"
        case .selective: return "Brush"
        case .gradient: return "Linear"
        case .radial: return "Radial"
        case .heal: return "Heal"
        case .clone: return "Clone"
        case .smooth: return "Skin"
        case .restore: return "Detail"
        case .crop: return "Crop"
        case .perspective: return "Perspective"
        case .blur: return "Blur"
        case .fx: return "Effects"
        case .text: return "Text"
        case .draw: return "Draw"
        case .layer: return "Layer"
        case .frame: return "Frame"
        case .cutout: return "Cutout"
        case .pixelArt: return "Pixel Art"
        case .texture: return "Texture"
        case .maps: return "AO & Fog"
        }
    }

    var icon: String {
        switch self {
        case .presets: return "sparkles.rectangle.stack"
        case .looks: return "camera.filters"
        case .lut: return "cube"
        case .enhance: return "wand.and.rays"
        case .adjust: return "slider.horizontal.3"
        case .curves: return "chart.xyaxis.line"
        case .hsl: return "paintpalette"
        case .grade: return "circle.lefthalf.filled"
        case .selective: return "paintbrush.pointed"
        case .gradient: return "rectangle.split.1x2"
        case .radial: return "circle.dashed"
        case .heal: return "eraser"
        case .clone: return "square.on.square.dashed"
        case .smooth: return "face.smiling"
        case .restore: return "bandage"
        case .crop: return "crop.rotate"
        case .perspective: return "perspective"
        case .blur: return "drop"
        case .fx: return "sparkles"
        case .text: return "textformat"
        case .draw: return "pencil.tip"
        case .layer: return "square.on.square"
        case .frame: return "photo.artframe"
        case .cutout: return "person.crop.rectangle"
        case .pixelArt: return "square.grid.3x3.square"
        case .texture: return "square.grid.4x3.fill"
        case .maps: return "cloud.fog"
        }
    }

    static let groups: [(String, [EditTool])] = [
        ("Presets & Looks", [.presets, .looks, .lut]),
        ("Light & Colour", [.enhance, .adjust, .curves, .hsl, .grade]),
        ("Local", [.selective, .gradient, .radial]),
        ("Retouch", [.heal, .clone, .smooth, .restore]),
        ("Geometry", [.crop, .perspective]),
        ("Effects", [.blur, .fx]),
        ("Create", [.text, .draw, .layer, .frame, .cutout, .pixelArt, .texture, .maps]),
    ]
}

/// State shared between a tool's panel and its overlay on the image.
@MainActor
final class EditorState: ObservableObject {
    enum Pick: Equatable { case balance, color }

    @Published var strokes: [Stroke] = []
    @Published var brush: Double = 0.03
    @Published var erase = false
    /// Normalised crop rectangle.
    @Published var crop = CGRect(x: 0, y: 0, width: 1, height: 1)
    @Published var cropAspect: CGFloat? = nil
    /// Shape crop ("rect" = plain crop) and its rings.
    @Published var cropShape = "rect"
    @Published var shapeRotation = 0.0
    @Published var shapeParam = 0.0
    @Published var rings: [RingSpec] = []
    @Published var shapeFilled = false
    @Published var shapeFill = Color.white
    /// Clone source and last destination (normalised).
    @Published var cloneSource: CGPoint?
    @Published var cloneSetting = true
    /// Cutout tap point (normalised).
    @Published var tap: CGPoint?
    @Published var brushActive = false
    /// Op of panels that build their own (newer tools).
    @Published var pendingOp: String?
    /// Handles: gradient start/end, text / layer / blur centre.
    @Published var p1 = CGPoint(x: 0.5, y: 0.5)
    @Published var p2 = CGPoint(x: 0.5, y: 0.85)
    /// Radial mask radii (fractions of width / height).
    @Published var radius = CGSize(width: 0.25, height: 0.25)
    /// Blur tool mode and tilt-shift band.
    @Published var blurMode = "gaussian"
    @Published var band = 0.2
    @Published var angle = 0.0
    /// Drawing.
    @Published var marks: [DrawMark] = []
    @Published var drawStyle = "pen"
    @Published var drawColor = Color(red: 1, green: 0.3, blue: 0.4)
    @Published var drawWidth = 0.01
    @Published var drawFill = false
    /// Eyedropper on the canvas.
    @Published var picking: Pick?
    @Published var pickedPoint: CGPoint?

    func reset() {
        strokes = []
        erase = false
        crop = CGRect(x: 0, y: 0, width: 1, height: 1)
        cropAspect = nil
        cropShape = "rect"
        shapeRotation = 0
        shapeParam = 0
        rings = []
        shapeFilled = false
        cloneSource = nil
        cloneSetting = true
        tap = nil
        brushActive = false
        pendingOp = nil
        p1 = CGPoint(x: 0.5, y: 0.5)
        p2 = CGPoint(x: 0.5, y: 0.85)
        radius = CGSize(width: 0.25, height: 0.25)
        blurMode = "gaussian"
        band = 0.2
        angle = 0
        marks = []
        picking = nil
        pickedPoint = nil
    }
}

struct EditScreen: View {
    @EnvironmentObject var app: AppModel
    @StateObject private var session = ToolSession()
    @StateObject private var editor = EditorState()
    @State private var tool: EditTool?
    @State private var comparing = false
    @AppStorage("pb.histogram") private var showHistogram = false
    @State private var zoom: CGFloat = 1
    @State private var zoomBase: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var panBase: CGSize = .zero

    var body: some View {
        VStack(spacing: 0) {
            StudioHeader(title: tool?.title ?? "Edit")
            if app.current == nil {
                EmptyPhotoState(title: "Edit a photo")
            } else {
                if tool == nil { editBar }
                stage.frame(maxHeight: .infinity)
                panel
            }
        }
        .onChange(of: app.current) { _, new in session.load(new) }
        .onAppear { session.load(app.current) }
        .sheet(isPresented: $app.showExport) {
            ExportSheet().presentationDetents([.medium, .large]).presentationBackground(Theme.ink)
        }
        .sheet(isPresented: $app.showHistory) {
            HistorySheet().presentationDetents([.medium, .large]).presentationBackground(Theme.ink)
        }
    }

    /// Undo / redo / history / histogram / more / export above the canvas.
    private var editBar: some View {
        HStack(spacing: 8) {
            CircleButton(icon: "arrow.uturn.backward") { app.undo() }.disabled(!app.canUndo).opacity(app.canUndo ? 1 : 0.4)
            CircleButton(icon: "arrow.uturn.forward") { app.redo() }.disabled(!app.canRedo).opacity(app.canRedo ? 1 : 0.4)
            CircleButton(icon: "clock.arrow.circlepath", badge: app.index) { app.showHistory = true }
            Spacer()
            Menu {
                Button { showHistogram.toggle() } label: { Label(showHistogram ? "Hide Histogram" : "Show Histogram", systemImage: "chart.bar") }
                Button { app.copyEdits() } label: { Label("Copy Edits", systemImage: "doc.on.doc") }
                Button { pasteEdits() } label: { Label("Paste Edits", systemImage: "doc.on.clipboard") }.disabled(app.copiedEdits == nil)
                Button { open(.presets) } label: { Label("Presets…", systemImage: "sparkles.rectangle.stack") }
                Divider()
                Button(role: .destructive) { app.jump(to: 0) } label: { Label("Revert to Original", systemImage: "arrow.counterclockwise") }.disabled(app.index == 0)
            } label: {
                Image(systemName: "ellipsis").font(.system(size: 16, weight: .semibold))
                    .frame(width: 42, height: 42).background(Theme.card2, in: Circle())
            }
            CircleButton(icon: "square.and.arrow.up") { app.showExport = true }
            Button { app.mode = .upscale } label: {
                Label("Upscale", systemImage: "wand.and.stars").font(.subheadline.weight(.bold))
                    .padding(.horizontal, 14).frame(height: 42).background(Theme.energy, in: Capsule()).foregroundStyle(.black)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 16).padding(.bottom, 6)
    }

    private var shown: UIImage? {
        if comparing { return tool == nil ? (app.originalPreview ?? app.preview) : app.preview }
        return session.live ?? app.preview
    }

    private var stage: some View {
        ImageStage(image: shown) { rect in
            if !comparing { toolOverlay(rect) }
        }
        .scaleEffect(tool == nil ? zoom : 1)
        .offset(tool == nil ? pan : .zero)
        .gesture(zoomGesture, including: tool == nil ? .all : .subviews)
        .gesture(TapGesture(count: 2).onEnded {
            withAnimation(.spring(duration: 0.3)) { zoom = 1; zoomBase = 1; pan = .zero; panBase = .zero }
        }, including: tool == nil ? .all : .subviews)
        .clipped()
        .padding(10)
        .overlay {
            if session.busy || app.busy { ProgressView().controlSize(.large) }
        }
        .overlay(alignment: .topTrailing) {
            if showHistogram, let h = session.histogram { HistogramView(histogram: h).padding(14) }
        }
        .overlay(alignment: .bottomLeading) { compareButton.padding(16) }
    }

    private var zoomGesture: some Gesture {
        SimultaneousGesture(
            MagnificationGesture()
                .onChanged { zoom = min(8, max(1, zoomBase * $0)) }
                .onEnded { _ in zoomBase = zoom; if zoom <= 1.01 { pan = .zero; panBase = .zero } },
            DragGesture()
                .onChanged { v in if zoom > 1.01 { pan = CGSize(width: panBase.width + v.translation.width, height: panBase.height + v.translation.height) } }
                .onEnded { _ in panBase = pan }
        )
    }

    /// Press and hold to see the photo before the current tool (or, with
    /// no tool open, the original).
    private var compareButton: some View {
        Image(systemName: comparing ? "eye.fill" : "eye")
            .font(.system(size: 15, weight: .semibold))
            .frame(width: 38, height: 38)
            .background(.black.opacity(0.55), in: Circle())
            .overlay(alignment: .trailing) {
                if comparing {
                    Text(tool == nil ? "Original" : "Before").font(.caption.bold())
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .background(.black.opacity(0.6), in: Capsule()).fixedSize().offset(x: 80)
                }
            }
            .onLongPressGesture(minimumDuration: 0.05, maximumDistance: 60, perform: {}, onPressingChanged: { comparing = $0 })
    }

    @ViewBuilder
    private func toolOverlay(_ rect: CGRect) -> some View {
        if editor.picking != nil {
            TapOverlay(rect: rect) { p in editor.pickedPoint = p }
        } else {
            switch tool {
            case .selective, .heal, .smooth:
                BrushOverlay(rect: rect, strokes: $editor.strokes, radius: editor.brush, erase: editor.erase) { editor.objectWillChange.send() }
            case .cutout where editor.brushActive:
                BrushOverlay(rect: rect, strokes: $editor.strokes, radius: editor.brush, erase: editor.erase) { editor.objectWillChange.send() }
            case .crop:
                ZStack {
                    if editor.cropShape != "rect" {
                        ShapeOverlay(rect: rect, editor: editor, imageSize: shown?.size ?? .zero)
                    }
                    CropOverlay(rect: rect, crop: $editor.crop, aspect: editor.cropAspect, imageSize: shown?.size ?? .zero)
                    GridOverlay(rect: rect, lines: 3)
                }
            case .perspective:
                GridOverlay(rect: rect)
            case .gradient:
                GradientOverlay(rect: rect, start: $editor.p1, end: $editor.p2)
            case .radial:
                RadialOverlay(rect: rect, center: $editor.p1, radius: $editor.radius)
            case .text, .layer:
                PointOverlay(rect: rect, point: $editor.p1)
            case .blur:
                if editor.blurMode == "tilt" {
                    BandOverlay(rect: rect, y: $editor.p1.y, band: editor.band, angle: editor.angle)
                } else if editor.blurMode == "zoom" || editor.blurMode == "spin" {
                    PointOverlay(rect: rect, point: $editor.p1)
                }
            case .draw:
                DrawOverlay(rect: rect, editor: editor)
            case .clone, .cutout:
                TapOverlay(rect: rect) { p in
                    if tool == .clone {
                        if editor.cloneSetting { editor.cloneSource = p; editor.cloneSetting = false } else { cloneStamp(at: p) }
                    } else {
                        editor.tap = p
                    }
                }
                .overlay {
                    if tool == .clone, let s = editor.cloneSource {
                        Circle().stroke(Theme.accent, lineWidth: 2).frame(width: 26, height: 26)
                            .position(x: rect.minX + s.x * rect.width, y: rect.minY + s.y * rect.height)
                    }
                }
            default:
                EmptyView()
            }
        }
    }

    @ViewBuilder
    private var panel: some View {
        VStack(spacing: 0) {
            if let tool {
                ToolPanel(tool: tool, session: session, editor: editor) { close() }
                    .padding(14)
                    .padding(.bottom, 4)
            } else {
                toolGrid
            }
        }
        .background(Theme.card, in: UnevenRoundedRectangle(topLeadingRadius: 26, topTrailingRadius: 26, style: .continuous))
        .padding(.bottom, 76)
    }

    private var toolGrid: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(EditTool.groups.indices, id: \.self) { gi in
                    let group = EditTool.groups[gi]
                    Text(group.0.uppercased()).font(.caption2.weight(.semibold)).tracking(1.2).foregroundStyle(Theme.dim)
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: 4), spacing: 10) {
                        ForEach(group.1) { t in
                            Button { open(t) } label: {
                                VStack(spacing: 6) {
                                    Image(systemName: t.icon).font(.title3)
                                    Text(t.title).font(.caption2).lineLimit(1).minimumScaleFactor(0.8)
                                }
                                .frame(maxWidth: .infinity, minHeight: 64)
                                .background(Theme.card2, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
            .padding(14)
        }
        .frame(maxHeight: 300)
    }

    private func open(_ t: EditTool) {
        editor.reset()
        session.clear()
        zoom = 1; zoomBase = 1; pan = .zero; panBase = .zero
        if t == .enhance {
            session.apply(opJSON("auto_enhance"), to: app, name: "Auto")
            return
        }
        tool = t
    }

    private func close() {
        tool = nil
        editor.reset()
        session.clear()
    }

    private func pasteEdits() {
        guard let r = app.copiedEdits else { return }
        app.run(r, step: "Pasted edits")
    }

    private func cloneStamp(at p: CGPoint) {
        guard let s = editor.cloneSource else { return }
        let op = opJSON("clone_stamp", ["sx": s.x, "sy": s.y, "dx": p.x, "dy": p.y, "radius": editor.brush, "hardness": 0.5])
        session.apply(op, to: app, name: "Clone")
    }
}

struct TapOverlay: View {
    let rect: CGRect
    let onTap: (CGPoint) -> Void
    var body: some View {
        Color.clear.contentShape(Rectangle())
            .gesture(SpatialTapGesture().onEnded { v in
                let p = CGPoint(x: (v.location.x - rect.minX) / rect.width, y: (v.location.y - rect.minY) / rect.height)
                if (0...1).contains(p.x), (0...1).contains(p.y) { onTap(p) }
            })
    }
}

/// Draggable crop frame (move by dragging inside, resize from the corner).
struct CropOverlay: View {
    let rect: CGRect
    @Binding var crop: CGRect
    let aspect: CGFloat?
    let imageSize: CGSize
    @State private var start: CGRect?

    var body: some View {
        let r = CGRect(x: rect.minX + crop.minX * rect.width, y: rect.minY + crop.minY * rect.height,
                       width: crop.width * rect.width, height: crop.height * rect.height)
        ZStack(alignment: .topLeading) {
            Path { p in
                p.addRect(rect)
                p.addRect(r)
            }
            .fill(Color.black.opacity(0.55), style: FillStyle(eoFill: true))
            Rectangle().stroke(.white, lineWidth: 1.5).frame(width: r.width, height: r.height).offset(x: r.minX, y: r.minY)
                .contentShape(Rectangle())
                .gesture(DragGesture().onChanged { v in
                    let s = start ?? crop
                    if start == nil { start = crop }
                    var n = s
                    n.origin.x = min(max(0, s.minX + v.translation.width / rect.width), 1 - s.width)
                    n.origin.y = min(max(0, s.minY + v.translation.height / rect.height), 1 - s.height)
                    crop = n
                }.onEnded { _ in start = nil })
            Circle().fill(.white).frame(width: 22, height: 22)
                .position(x: r.maxX, y: r.maxY)
                .gesture(DragGesture().onChanged { v in
                    let s = start ?? crop
                    if start == nil { start = crop }
                    var w = min(max(0.05, s.width + v.translation.width / rect.width), 1 - s.minX)
                    var h = min(max(0.05, s.height + v.translation.height / rect.height), 1 - s.minY)
                    if let a = aspect, imageSize.width > 0 {
                        // a = width / height in pixels.
                        let px = imageSize.width / imageSize.height
                        h = w * px / a
                        if s.minY + h > 1 { h = 1 - s.minY; w = h * a / px }
                    }
                    crop = CGRect(x: s.minX, y: s.minY, width: w, height: h)
                }.onEnded { _ in start = nil })
        }
    }
}
