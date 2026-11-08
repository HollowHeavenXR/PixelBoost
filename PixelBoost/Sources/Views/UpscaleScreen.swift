import SwiftUI
import UIKit
import UniformTypeIdentifiers
import WidgetKit

struct UpscaleScreen: View {
    @EnvironmentObject var app: AppModel
    @StateObject private var job = UpscaleJob()

    @AppStorage("up.model") private var model = "auto"
    @AppStorage("up.scale") private var scale = 4.0
    @AppStorage("up.fidelity") private var fidelity = "natural"
    @AppStorage("up.detail") private var detail = 0.35
    @AppStorage("up.quality") private var quality = 1
    @AppStorage("up.denoise") private var denoise = 0.0
    /// Learned device speed (GFLOP/s) for the time estimate.
    @AppStorage("up.gflops") private var gflops = 40.0

    @State private var autoPick: String?
    @State private var comparing = false
    @State private var resultPreview: UIImage?
    @State private var shareURL: URL?

    var body: some View {
        NavigationStack {
            Group {
                if app.current == nil {
                    EmptyPhotoState(title: "Upscale a photo")
                } else {
                    content
                }
            }
            .background(Theme.canvas)
            .navigationTitle("PixelBoost")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if app.current != nil {
                    ToolbarItem(placement: .topBarLeading) {
                        PhotosMenu()
                    }
                }
            }
        }
        .onChange(of: app.current) { _, _ in
            job.reset()
            resultPreview = nil
            comparing = false
            refreshAutoPick()
        }
        .onAppear { refreshAutoPick() }
    }

    private var effectiveModel: String { model == "auto" ? (autoPick ?? "photo") : model }

    private var content: some View {
        ScrollView {
            VStack(spacing: 14) {
                stage
                if job.state == .running { progressPanel }
                if let o = job.outcome, job.state == .done { resultPanel(o) }
                if job.state != .running { settingsPanel }
                if case .failed(let m) = job.state {
                    Text(m).font(.footnote).foregroundStyle(.red)
                }
            }
            .padding(14)
        }
    }

    // MARK: stage

    private var stage: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Theme.panel)
            if comparing, let before = app.preview, let after = resultPreview {
                CompareSlider(before: before, after: after)
            } else if let img = job.canvas ?? app.preview {
                Image(uiImage: img).resizable().interpolation(.high).scaledToFit()
            }
        }
        .frame(height: 360)
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(alignment: .bottomLeading) {
            if let i = app.info {
                Text("\(i.width)×\(i.height)\(i.hasAlpha ? " · alpha" : "")")
                    .font(.caption.monospacedDigit()).padding(6)
                    .background(.black.opacity(0.6), in: Capsule()).padding(10)
            }
        }
    }

    private var progressPanel: some View {
        Panel {
            ProgressView(value: job.progress)
            HStack {
                Text("Tile \(job.done) of \(max(job.total, 1))").font(.subheadline.monospacedDigit())
                Spacer()
                if let eta = job.eta { Text("~\(Self.duration(eta)) left").font(.subheadline).foregroundStyle(Theme.dim) }
            }
            Button(role: .destructive) { job.cancel() } label: { Label("Cancel", systemImage: "xmark") }
                .buttonStyle(.bordered)
        }
    }

    private func resultPanel(_ o: UpscaleOutcome) -> some View {
        Panel(title: "Result") {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(o.width)×\(o.height)").font(.headline.monospacedDigit())
                    Text("\(modelName(o.model)) · \(Self.duration(o.seconds)) · fidelity \(String(format: "%.1f", o.fidelityDb)) dB")
                        .font(.caption).foregroundStyle(Theme.dim)
                }
                Spacer()
                Toggle("Compare", isOn: $comparing).labelsHidden().disabled(resultPreview == nil)
                Image(systemName: "rectangle.split.2x1").foregroundStyle(Theme.dim)
            }
            HStack(spacing: 10) {
                Button { save(o) } label: { Label("Save", systemImage: "square.and.arrow.down") }
                ShareLink(item: URL(fileURLWithPath: o.outputPath)) { Label("Share", systemImage: "square.and.arrow.up") }
                Button {
                    app.commit(URL(fileURLWithPath: o.outputPath))
                    app.tab = .edit
                } label: { Label("Edit", systemImage: "slider.horizontal.3") }
            }
            .buttonStyle(.bordered)
        }
    }

    // MARK: settings

    private var settingsPanel: some View {
        VStack(spacing: 14) {
            Panel(title: "Model") {
                ChipPicker(options: [("auto", "Auto")] + app.models.map { ($0.id, $0.name) }, selection: $model)
                Text(modelBlurb).font(.caption).foregroundStyle(Theme.dim)
            }
            Panel(title: "Output") {
                ChipPicker(options: [(2.0, "2×"), (3.0, "3×"), (4.0, "4×"), (8.0, "8×")], selection: $scale)
                ChipPicker(options: [(1, "Standard"), (2, "High"), (8, "Max")], selection: $quality)
                Text(qualityBlurb).font(.caption).foregroundStyle(Theme.dim)
            }
            Panel(title: "Clarity") {
                ChipPicker(options: [("off", "Raw AI"), ("natural", "Natural"), ("faithful", "Faithful")], selection: $fidelity)
                Text(fidelityBlurb).font(.caption).foregroundStyle(Theme.dim)
                SliderRow(label: "Detail", value: $detail)
                if effectiveModel == "fast" {
                    SliderRow(label: "Denoise", value: $denoise)
                }
            }
            planLine
            PrimaryButton(title: "Upscale", icon: "wand.and.stars") { start() }
        }
    }

    private var planLine: some View {
        let i = app.info
        let plan = i.map { Core.engine.plan(width: $0.width, height: $0.height, model: effectiveModel, scale: Float(scale), tile: 128) }
        return HStack {
            if let p = plan {
                Text("→ \(p.outWidth)×\(p.outHeight) · \(p.tiles) tiles")
                Spacer()
                Text("≈ \(Self.duration(p.gflop * Double(max(quality, 1)) * (scale > 4 ? 17 : 1) / max(gflops, 1)))")
            }
        }
        .font(.footnote.monospacedDigit())
        .foregroundStyle(Theme.dim)
        .padding(.horizontal, 4)
    }

    private var modelBlurb: String {
        if model == "auto" {
            return "Auto picks per photo — this one: \(modelName(effectiveModel))."
        }
        return app.models.first { $0.id == model }?.bestFor ?? ""
    }

    private var qualityBlurb: String {
        switch quality {
        case 2: return "Runs the model twice (mirrored) and averages — steadier fine detail, 2× the time."
        case 8: return "Eight rotated/mirrored passes averaged — the cleanest result, 8× the time."
        default: return "One pass of the model over every pixel."
        }
    }

    private var fidelityBlurb: String {
        switch fidelity {
        case "off": return "The network's output as-is."
        case "faithful": return "Every output pixel is pulled to agree with the source as closely as possible."
        default: return "Keeps the AI's sharp edges, restores colour and shading it flattened, removes halos."
        }
    }

    private func modelName(_ id: String) -> String { app.models.first { $0.id == id }?.name ?? id }

    // MARK: actions

    private func refreshAutoPick() {
        guard let url = app.current else { return }
        Task.detached {
            let m = try? Core.engine.autoModel(inputPath: url.path)
            await MainActor.run { autoPick = m }
        }
    }

    private func start() {
        guard let url = app.current, let info = app.info else { return }
        let s = UpscaleSettings(model: model, scale: Float(scale), fidelity: fidelity, detail: Float(detail),
                                denoise: Float(denoise), ensemble: UInt32(quality), tile: 128)
        let outW = Int((Double(info.width) * scale).rounded()), outH = Int((Double(info.height) * scale).rounded())
        comparing = false
        resultPreview = nil
        UIApplication.shared.isIdleTimerDisabled = true
        Task {
            let began = Date()
            let o = await job.run(input: url, settings: s, base: app.preview, outW: outW, outH: outH)
            UIApplication.shared.isIdleTimerDisabled = false
            log(info: info, settings: s, outcome: o, ms: Int(Date().timeIntervalSince(began) * 1000))
            guard let o else { return }
            resultPreview = job.canvas
            let plan = Core.engine.plan(width: info.width, height: info.height, model: o.model, scale: Float(scale), tile: 128)
            if o.seconds > 2 { gflops = 0.7 * gflops + 0.3 * (plan.gflop * Double(o.passes) * Double(max(quality, 1)) / o.seconds) }
            let entry = HistoryEntry(id: "", createdAt: 0, model: o.model, scale: Float(scale), inWidth: info.width, inHeight: info.height,
                                     outWidth: o.width, outHeight: o.height, seconds: o.seconds, fidelityDb: o.fidelityDb, resultPath: "", thumbPath: "")
            let added = try? Core.history.add(entry: entry, resultFile: o.outputPath)
            if let t = added.flatMap({ UIImage(contentsOfFile: $0.thumbPath) }) {
                UpscaleSnapshot.record(resultThumbnail: t)
                WidgetCenter.shared.reloadAllTimelines()
            }
        }
    }

    private func save(_ o: UpscaleOutcome) {
        Task {
            do {
                try await PhotoIO.saveToPhotos(URL(fileURLWithPath: o.outputPath))
                app.message = "Saved to Photos."
                app.log("save", ["w": o.width, "h": o.height])
            } catch { app.message = describe(error) }
        }
    }

    private func log(info: ImageInfo, settings s: UpscaleSettings, outcome o: UpscaleOutcome?, ms: Int) {
        guard Prefs.telemetry else { return }
        var e: [String: Any] = [
            "source_width": info.width, "source_height": info.height, "technique": "pixelboost_rust",
            "model_name": o?.model ?? s.model, "tile_size": 128, "overlap": 16, "scale_factor": Int(s.scale.rounded()),
            "processing_ms": ms, "success": o != nil, "app_version": Core.appVersion,
            "os_version": UIDevice.current.systemVersion, "device_model": UIDevice.current.model,
            "detail_level": s.fidelity, "sharpen": Double(s.detail), "run_kind": "single",
        ]
        if let o {
            e["output_width"] = o.width
            e["output_height"] = o.height
            e["tile_count"] = o.tiles
            e["fidelity_psnr"] = o.fidelityDb
        } else if case .failed(let m) = job.state {
            e["error_message"] = m
        } else {
            e["cancelled"] = true
        }
        if let d = try? JSONSerialization.data(withJSONObject: e) {
            Core.server.logUpscale(entryJson: String(decoding: d, as: UTF8.self))
        }
    }

    static func duration(_ s: Double) -> String {
        if s < 1 { return "<1s" }
        if s < 90 { return "\(Int(s.rounded()))s" }
        return "\(Int(s / 60))m \(Int(s.truncatingRemainder(dividingBy: 60)))s"
    }
}

/// Replace / undo menu shown when a photo is loaded.
struct PhotosMenu: View {
    @EnvironmentObject var app: AppModel
    @State private var showFiles = false
    @State private var picking = false
    var body: some View {
        Menu {
            Button { picking = true } label: { Label("Choose Photo", systemImage: "photo") }
            Button { app.pasteFromClipboard() } label: { Label("Paste", systemImage: "doc.on.clipboard") }
            Button { showFiles = true } label: { Label("Files", systemImage: "folder") }
        } label: {
            Image(systemName: "photo.badge.plus")
        }
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.image]) { r in
            if case .success(let url) = r { app.importFile(url) }
        }
        .sheet(isPresented: $picking) {
            VStack { PhotoPickerButton().padding() }
                .presentationDetents([.height(140)])
                .onChange(of: app.current) { _, _ in picking = false }
        }
    }
}
