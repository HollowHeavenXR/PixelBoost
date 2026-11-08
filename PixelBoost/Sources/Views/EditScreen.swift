import SwiftUI

enum EditTool: String, CaseIterable, Identifiable {
    case adjust, looks, enhance, crop, restore, selective, heal, clone, cutout, pixelArt, lut, texture, maps
    var id: String { rawValue }

    var title: String {
        switch self {
        case .adjust: return "Adjust"
        case .looks: return "Looks"
        case .enhance: return "Auto"
        case .crop: return "Crop"
        case .restore: return "Restore"
        case .selective: return "Selective"
        case .heal: return "Heal"
        case .clone: return "Clone"
        case .cutout: return "Cutout"
        case .pixelArt: return "Pixel Art"
        case .lut: return "LUT"
        case .texture: return "Texture"
        case .maps: return "AO & Fog"
        }
    }

    var icon: String {
        switch self {
        case .adjust: return "slider.horizontal.3"
        case .looks: return "camera.filters"
        case .enhance: return "wand.and.rays"
        case .crop: return "crop.rotate"
        case .restore: return "bandage"
        case .selective: return "paintbrush.pointed"
        case .heal: return "eraser"
        case .clone: return "square.on.square.dashed"
        case .cutout: return "person.crop.rectangle"
        case .pixelArt: return "square.grid.3x3.square"
        case .lut: return "cube"
        case .texture: return "square.grid.4x3.fill"
        case .maps: return "cloud.fog"
        }
    }

    static let groups: [(String, [EditTool])] = [
        ("Tone & Colour", [.enhance, .adjust, .looks, .lut, .selective]),
        ("Fix", [.restore, .heal, .clone, .crop]),
        ("Create", [.cutout, .pixelArt, .texture, .maps]),
    ]
}

/// State shared between a tool's panel and its overlay on the image.
@MainActor
final class EditorState: ObservableObject {
    @Published var strokes: [Stroke] = []
    @Published var brush: Double = 0.03
    @Published var erase = false
    /// Normalised crop rectangle.
    @Published var crop = CGRect(x: 0, y: 0, width: 1, height: 1)
    @Published var cropAspect: CGFloat? = nil
    /// Clone source and last destination (normalised).
    @Published var cloneSource: CGPoint?
    @Published var cloneSetting = true
    /// Cutout tap point (normalised).
    @Published var tap: CGPoint?
    @Published var brushActive = false

    func reset() {
        strokes = []
        erase = false
        crop = CGRect(x: 0, y: 0, width: 1, height: 1)
        cropAspect = nil
        cloneSource = nil
        cloneSetting = true
        tap = nil
        brushActive = false
    }
}

struct EditScreen: View {
    @EnvironmentObject var app: AppModel
    @StateObject private var session = ToolSession()
    @StateObject private var editor = EditorState()
    @State private var tool: EditTool?

    var body: some View {
        NavigationStack {
            Group {
                if app.current == nil {
                    EmptyPhotoState(title: "Edit a photo")
                } else {
                    VStack(spacing: 0) {
                        stage.frame(maxHeight: .infinity)
                        panel
                    }
                }
            }
            .background(Theme.canvas)
            .navigationTitle(tool?.title ?? "Edit")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if tool == nil, app.current != nil {
                    ToolbarItemGroup(placement: .topBarLeading) {
                        Button { app.undo() } label: { Image(systemName: "arrow.uturn.backward") }.disabled(!app.canUndo)
                        Button { app.redo() } label: { Image(systemName: "arrow.uturn.forward") }.disabled(!app.canRedo)
                    }
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        if let c = app.current { ShareLink(item: c) { Image(systemName: "square.and.arrow.up") } }
                        Button { save() } label: { Image(systemName: "square.and.arrow.down") }
                        PhotosMenu()
                    }
                }
            }
        }
        .onChange(of: app.current) { _, new in session.load(new) }
        .onAppear { session.load(app.current) }
    }

    private var stage: some View {
        ImageStage(image: session.live ?? app.preview) { rect in
            toolOverlay(rect)
        }
        .padding(10)
        .overlay {
            if session.busy { ProgressView().controlSize(.large) }
        }
    }

    @ViewBuilder
    private func toolOverlay(_ rect: CGRect) -> some View {
        switch tool {
        case .selective, .heal:
            BrushOverlay(rect: rect, strokes: $editor.strokes, radius: editor.brush, erase: editor.erase) { editor.objectWillChange.send() }
        case .cutout where editor.brushActive:
            BrushOverlay(rect: rect, strokes: $editor.strokes, radius: editor.brush, erase: editor.erase) { editor.objectWillChange.send() }
        case .crop:
            CropOverlay(rect: rect, crop: $editor.crop, aspect: editor.cropAspect, imageSize: app.preview?.size ?? .zero)
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

    @ViewBuilder
    private var panel: some View {
        VStack(spacing: 0) {
            if let tool {
                ToolPanel(tool: tool, session: session, editor: editor) { close() }
                    .padding(14)
            } else {
                toolGrid
            }
        }
        .background(Theme.panel)
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
                                    Text(t.title).font(.caption2)
                                }
                                .frame(maxWidth: .infinity, minHeight: 64)
                                .background(Theme.panel2, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
            .padding(14)
        }
        .frame(maxHeight: 330)
    }

    private func open(_ t: EditTool) {
        editor.reset()
        session.clear()
        if t == .enhance {
            session.apply(opJSON("auto_enhance"), to: app, name: "auto_enhance")
            return
        }
        tool = t
    }

    private func close() {
        tool = nil
        editor.reset()
        session.clear()
    }

    private func cloneStamp(at p: CGPoint) {
        guard let s = editor.cloneSource else { return }
        let op = opJSON("clone_stamp", ["sx": s.x, "sy": s.y, "dx": p.x, "dy": p.y, "radius": editor.brush, "hardness": 0.5])
        session.apply(op, to: app, name: "clone")
    }

    private func save() {
        guard let c = app.current else { return }
        Task {
            do { try await PhotoIO.saveToPhotos(c); app.message = "Saved to Photos." } catch { app.message = describe(error) }
        }
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
