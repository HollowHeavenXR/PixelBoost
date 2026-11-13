import SwiftUI
import UniformTypeIdentifiers
import UIKit
import WidgetKit

/// Upscale mode: the photo fills the screen; one summary bar opens the
/// upscale sheet. On-device runs paint tiles live onto the canvas.
struct StudioView: View {
    @EnvironmentObject var app: AppModel
    @ObservedObject var job: UpscaleJob
    @State private var showSheet = false
    @State private var comparing = false
    @State private var resultPreview: UIImage?
    @State private var smartSummary: String?
    @State private var lastScale: Float = 4
    let choice = UpscaleChoice()

    var body: some View {
        VStack(spacing: 0) {
            StudioHeader(title: "Studio")
            if app.current == nil {
                EmptyPhotoState()
            } else {
                canvas.padding(.horizontal, 12)
                bottom.padding(.horizontal, 14).padding(.top, 12).padding(.bottom, 92)
            }
        }
        .sheet(isPresented: $showSheet) {
            UpscaleSheet(onDevice: startOnDevice, onCloud: sendToCloud)
                .presentationDetents([.large])
                .presentationBackground(Theme.ink)
        }
        .onChange(of: app.current) { _, _ in
            job.reset()
            resultPreview = nil
            comparing = false
        }
        .task(id: "\(app.current?.path ?? "")|\(choice.smart)|\(choice.whereTo)|\(choice.maxWait)") {
            smartSummary = nil
            guard choice.smart, let url = app.current else { return }
            smartSummary = await choice.analysis(for: url, cloud: choice.whereTo == "cloud")?.summary
        }
    }

    private var canvas: some View {
        ZStack {
            if comparing, let before = app.preview, let after = resultPreview {
                CompareSlider(before: before, after: after)
            } else if let img = job.canvas ?? app.preview {
                Image(uiImage: img).resizable().interpolation(.high).scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay(alignment: .topLeading) {
            if let i = app.info {
                Text("\(i.width) × \(i.height)").font(.caption.monospacedDigit().weight(.medium))
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    .background(.black.opacity(0.55), in: Capsule()).padding(10)
            }
        }
    }

    @ViewBuilder
    private var bottom: some View {
        switch job.state {
        case .running:
            running
        case .done:
            if let o = job.outcome { result(o) }
        default:
            VStack(spacing: 10) {
                if case .failed(let m) = job.state {
                    Text(m).font(.footnote).foregroundStyle(.red).frame(maxWidth: .infinity, alignment: .leading)
                }
                summaryBar
            }
        }
    }

    /// "Auto · 4× · Natural ›" + the main button.
    private var summaryBar: some View {
        VStack(spacing: 10) {
            Button { showSheet = true } label: {
                HStack(spacing: 10) {
                    Image(systemName: choice.whereTo == "cloud" ? "cloud.fill" : "iphone")
                        .foregroundStyle(Theme.amber)
                    Text(summaryText).font(.subheadline.weight(.medium)).lineLimit(1)
                    Spacer()
                    Text("Change").font(.subheadline).foregroundStyle(Theme.dim)
                    Image(systemName: "chevron.up").font(.caption.bold()).foregroundStyle(Theme.dim)
                }
                .padding(.horizontal, 16).padding(.vertical, 14)
                .background(Theme.card, in: Capsule())
                .overlay(Capsule().stroke(Theme.line))
            }
            .buttonStyle(.plain)
            PrimaryButton(title: choice.whereTo == "cloud" ? "Send to Cloud" : "Upscale", icon: choice.whereTo == "cloud" ? "icloud.and.arrow.up" : "wand.and.stars") {
                choice.whereTo == "cloud" ? sendToCloud() : startOnDevice()
            }
        }
    }

    private var summaryText: String {
        if choice.smart {
            return smartSummary.map { "Smart · \($0)" } ?? "Smart settings"
        }
        return "\(UpscaleChoice.name(choice.model)) · \(Int(choice.scale))× · \(choice.fidelity.capitalized)\(choice.quality > 1 ? " · \(choice.quality == 2 ? "High" : "Max")" : "")"
    }

    private var running: some View {
        Card {
            HStack {
                ZStack {
                    Circle().stroke(Theme.card2, lineWidth: 5)
                    Circle().trim(from: 0, to: job.progress).stroke(Theme.energy, style: StrokeStyle(lineWidth: 5, lineCap: .round)).rotationEffect(.degrees(-90))
                    Text("\(Int(job.progress * 100))").font(.caption.monospacedDigit().bold())
                }
                .frame(width: 46, height: 46)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Upscaling on this iPhone").font(.subheadline.weight(.semibold))
                    Text(job.eta.map { "about \(formatDuration($0)) left · tile \(job.done)/\(job.total)" } ?? "starting…")
                        .font(.caption.monospacedDigit()).foregroundStyle(Theme.dim)
                }
                Spacer()
                CircleButton(icon: "xmark") { job.cancel() }
            }
        }
    }

    private func result(_ o: UpscaleOutcome) -> some View {
        Card {
            HStack {
                VStack(alignment: .leading, spacing: 3) {
                    Text("\(o.width) × \(o.height)").font(.headline.monospacedDigit())
                    Text("\(UpscaleChoice.name(o.model)) · \(formatDuration(o.seconds)) · \(String(format: "%.1f", o.fidelityDb)) dB\(o.capped ? " · size capped" : "")")
                        .font(.caption).foregroundStyle(Theme.dim)
                }
                Spacer()
                Button { comparing.toggle() } label: {
                    Image(systemName: "rectangle.lefthalf.filled").font(.title3)
                        .foregroundStyle(comparing ? Theme.amber : .white)
                }
                .disabled(resultPreview == nil)
            }
            if app.sourceAsset != nil {
                Button { app.replaceOriginal(with: URL(fileURLWithPath: o.outputPath), model: o.model, scale: lastScale) } label: {
                    Label("Replace original in Photos", systemImage: "arrow.triangle.2.circlepath")
                        .font(.subheadline.weight(.bold)).frame(maxWidth: .infinity).padding(.vertical, 11)
                        .background(Theme.energy, in: Capsule()).foregroundStyle(.black)
                }
                .buttonStyle(.plain)
                .disabled(app.busy)
            }
            HStack(spacing: 8) {
                action(app.sourceAsset != nil ? "Copy" : "Save", "square.and.arrow.down") { save(o) }
                ShareLink(item: URL(fileURLWithPath: o.outputPath)) {
                    Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity).padding(.vertical, 10)
                        .background(Theme.card2, in: Capsule())
                }
                .buttonStyle(.plain)
                action("Edit", "slider.horizontal.3") {
                    app.commit(URL(fileURLWithPath: o.outputPath), step: "Upscale \(o.width)×\(o.height)")
                    app.mode = .edit
                }
                action("Again", "arrow.counterclockwise") { job.reset() }
            }
            .font(.subheadline.weight(.medium))
        }
    }

    private func action(_ t: String, _ i: String, _ f: @escaping () -> Void) -> some View {
        Button(action: f) {
            Label(t, systemImage: i).labelStyle(.titleAndIcon).frame(maxWidth: .infinity).padding(.vertical, 10)
                .background(Theme.card2, in: Capsule())
        }
        .buttonStyle(.plain)
    }

    // MARK: actions

    private func sendToCloud() {
        showSheet = false
        guard let url = app.current else { return }
        let asset = app.sourceAsset
        Task {
            let s = await choice.resolved(for: url, cloud: true)
            let label = app.info.map { "\($0.width)×\($0.height) → \(Int(s.scale))×" } ?? ""
            CloudStore.shared.send(url, settings: UpscaleChoice.cloudSettings(s), label: label, app: app, asset: asset)
        }
    }

    private func startOnDevice() {
        showSheet = false
        guard let url = app.current, let info = app.info else { return }
        comparing = false
        resultPreview = nil
        UIApplication.shared.isIdleTimerDisabled = true
        Task {
            let s = await choice.resolved(for: url, cloud: false)
            let quality = Int(s.ensemble), scale = Double(s.scale)
            lastScale = s.scale
            let outW = Int((Double(info.width) * scale).rounded()), outH = Int((Double(info.height) * scale).rounded())
            let began = Date()
            let o = await job.run(input: url, settings: s, base: app.preview, outW: outW, outH: outH)
            UIApplication.shared.isIdleTimerDisabled = false
            log(info: info, settings: s, outcome: o, ms: Int(Date().timeIntervalSince(began) * 1000))
            guard let o else { return }
            resultPreview = job.canvas
            let plan = Core.engine.plan(width: info.width, height: info.height, model: o.model, scale: Float(scale), tile: 128)
            if o.seconds > 2 {
                choice.gflops = 0.7 * choice.gflops + 0.3 * (plan.gflop * Double(o.passes) * Double(max(quality, 1)) / o.seconds)
            }
            let entry = HistoryEntry(id: "", createdAt: 0, model: o.model, scale: Float(scale), inWidth: info.width, inHeight: info.height,
                                     outWidth: o.width, outHeight: o.height, seconds: o.seconds, fidelityDb: o.fidelityDb, resultPath: "", thumbPath: "")
            if let added = try? Core.history.add(entry: entry, resultFile: o.outputPath), let t = UIImage(contentsOfFile: added.thumbPath) {
                UpscaleSnapshot.record(resultThumbnail: t)
                WidgetCenter.shared.reloadAllTimelines()
            }
        }
    }

    private func save(_ o: UpscaleOutcome) {
        Task {
            do {
                try await PhotoIO.saveToPhotos(URL(fileURLWithPath: o.outputPath))
                app.flash("Saved to Photos")
            } catch { app.message = describe(error) }
        }
    }

    private func log(info: ImageInfo, settings s: UpscaleSettings, outcome o: UpscaleOutcome?, ms: Int) {
        guard Prefs.telemetry else { return }
        var e: [String: Any] = [
            "source_width": info.width, "source_height": info.height, "technique": "pixelboost_rust",
            "model_name": o?.model ?? s.model, "tile_size": 128, "overlap": 24, "scale_factor": Int(s.scale.rounded()),
            "processing_ms": ms, "success": o != nil, "app_version": Core.appVersion,
            "os_version": UIDevice.current.systemVersion, "device_model": UIDevice.current.model,
            "detail_level": s.fidelity, "sharpen": Double(s.detail), "run_kind": "single",
        ]
        if let o {
            e["output_width"] = o.width
            e["output_height"] = o.height
            e["tile_count"] = o.tiles
            e["fidelity_psnr"] = o.fidelityDb
            e["output_was_capped"] = o.capped
        } else if case .failed(let m) = job.state {
            e["error_message"] = m
        } else {
            e["cancelled"] = true
        }
        if let d = try? JSONSerialization.data(withJSONObject: e) {
            Core.server.logUpscale(entryJson: String(decoding: d, as: UTF8.self))
        }
    }
}

/// Top bar shared by the three modes.
struct StudioHeader: View {
    @EnvironmentObject var app: AppModel
    let title: String
    var showPhotoMenu = true
    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 0) {
                Text("PIXELBOOST").font(.caption2.weight(.heavy)).tracking(2.5).foregroundStyle(Theme.energy)
                Text(title).font(.title2.weight(.bold))
            }
            Spacer()
            if showPhotoMenu, app.current != nil { PhotosMenu() }
            CircleButton(icon: "gearshape") { app.showSettings = true }
        }
        .padding(.horizontal, 16).padding(.top, 6).padding(.bottom, 10)
    }
}

/// Replace / paste / files.
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
            Image(systemName: "photo.badge.plus").font(.system(size: 16, weight: .semibold))
                .frame(width: 42, height: 42).background(Theme.card2, in: Circle())
        }
        .fileImporter(isPresented: $showFiles, allowedContentTypes: [.image]) { r in
            if case .success(let url) = r { app.importFile(url) }
        }
        .sheet(isPresented: $picking) {
            VStack { PhotoPickerButton().padding() }
                .presentationDetents([.height(140)])
                .presentationBackground(Theme.ink)
                .onChange(of: app.current) { _, _ in picking = false }
        }
    }
}
