import PhotosUI
import SwiftUI

/// Server jobs (live status, full-res download) and on-device history.
struct CloudScreen: View {
    @EnvironmentObject var app: AppModel
    @ObservedObject var cloud = CloudStore.shared
    @State private var section = 0
    @State private var picks: [PhotosPickerItem] = []
    @State private var showBatch = false
    @State private var history: [HistoryEntry] = []
    let choice = UpscaleChoice()

    var body: some View {
        VStack(spacing: 0) {
            StudioHeader(title: "Cloud", showPhotoMenu: false)
            Picker("", selection: $section) {
                Text("PixelBoost Cloud").tag(0)
                Text("On this iPhone").tag(1)
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16).padding(.bottom, 10)
            ScrollView {
                VStack(spacing: 12) {
                    if section == 0 { cloudList } else { historyGrid }
                }
                .padding(.horizontal, 14)
                .padding(.bottom, 110)
            }
            .refreshable { await cloud.refresh(); history = Core.history.list() }
        }
        .onAppear {
            cloud.startPolling()
            history = Core.history.list()
        }
        .onDisappear { cloud.stopPolling() }
        .sheet(isPresented: $showBatch) { BatchScreen().presentationBackground(Theme.ink) }
        .onChange(of: picks) { _, new in sendPicked(new) }
    }

    // MARK: cloud

    @ViewBuilder
    private var cloudList: some View {
        HStack(spacing: 10) {
            Circle().fill(cloud.reachable == false ? Color.red : Theme.good).frame(width: 8, height: 8)
            Text(cloud.reachable == false ? "Cloud unreachable" : cloud.activeCount > 0 ? "\(cloud.activeCount) working" : "Cloud ready")
                .font(.footnote.weight(.medium)).foregroundStyle(Theme.dim)
            Spacer()
            PhotosPicker(selection: $picks, maxSelectionCount: 30, matching: .images) {
                Label("Send photos", systemImage: "plus").font(.footnote.weight(.semibold))
                    .padding(.horizontal, 12).padding(.vertical, 8).background(Theme.card2, in: Capsule())
            }
            Button { showBatch = true } label: {
                Image(systemName: "iphone.gen3").font(.footnote.weight(.semibold))
                    .padding(8).background(Theme.card2, in: Circle())
            }
            .buttonStyle(.plain)
        }
        if !cloud.sending.isEmpty {
            Card {
                HStack {
                    ProgressView()
                    Text("Sending \(cloud.sending.count) photo\(cloud.sending.count == 1 ? "" : "s")… keep the app open until it's sent.")
                        .font(.subheadline)
                }
            }
        }
        if cloud.jobs.isEmpty {
            VStack(spacing: 10) {
                Image(systemName: "cloud").font(.system(size: 44, weight: .light)).foregroundStyle(Theme.energy)
                Text("Nothing in the cloud yet").font(.headline)
                Text("Pick PixelBoost Cloud in the upscale sheet, or send photos from here. You can close the app while they upscale; results stay for 7 days.")
                    .font(.footnote).foregroundStyle(Theme.dim).multilineTextAlignment(.center)
            }
            .padding(.vertical, 50).padding(.horizontal, 20)
        }
        ForEach(cloud.jobs, id: \.id) { j in jobCard(j) }
    }

    private func jobCard(_ j: CloudJob) -> some View {
        Card {
            HStack(alignment: .top, spacing: 12) {
                ZStack {
                    Theme.card2
                    if let t = cloud.thumbs[j.id] { Image(uiImage: t).resizable().scaledToFill() }
                }
                .frame(width: 72, height: 72).clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                VStack(alignment: .leading, spacing: 4) {
                    Text(j.label.isEmpty ? j.filename : j.label).font(.subheadline.weight(.semibold)).lineLimit(1)
                    status(j)
                }
                Spacer(minLength: 0)
                Menu {
                    if let u = cloud.localResult(j) {
                        ShareLink(item: u) { Label("Share", systemImage: "square.and.arrow.up") }
                    }
                    Button(role: .destructive) { cloud.delete(j) } label: {
                        Label(j.status == "running" || j.status == "queued" ? "Cancel" : "Delete", systemImage: "trash")
                    }
                } label: {
                    Image(systemName: "ellipsis").frame(width: 30, height: 30)
                }
            }
            if j.status == "running" {
                ProgressView(value: Double(j.progress)).tint(Theme.amber)
            }
            if j.status == "done" { doneActions(j) }
        }
    }

    @ViewBuilder
    private func status(_ j: CloudJob) -> some View {
        switch j.status {
        case "queued":
            Text(j.queuePosition > 1 ? "Waiting · #\(j.queuePosition) in line" : "Up next").font(.caption).foregroundStyle(Theme.dim)
        case "running":
            Text("Upscaling · \(Int(j.progress * 100))% · \(UpscaleChoice.name(j.model))").font(.caption.monospacedDigit()).foregroundStyle(Theme.amber)
        case "done":
            Text("\(j.outWidth) × \(j.outHeight) · \(formatBytes(j.resultBytes)) · \(formatDuration(j.seconds))\(j.capped ? " · capped" : "")")
                .font(.caption.monospacedDigit()).foregroundStyle(Theme.good)
        case "failed":
            Text(j.error.isEmpty ? "Failed" : j.error).font(.caption).foregroundStyle(.red).lineLimit(2)
        default:
            Text(j.status.capitalized).font(.caption).foregroundStyle(Theme.dim)
        }
    }

    @ViewBuilder
    private func doneActions(_ j: CloudJob) -> some View {
        if let p = cloud.downloading[j.id] {
            HStack {
                ProgressView(value: p).tint(Theme.amber)
                Text("\(Int(p * 100))%").font(.caption.monospacedDigit()).foregroundStyle(Theme.dim)
            }
        } else if let u = cloud.localResult(j) {
            HStack(spacing: 8) {
                pill("Save to Photos", "square.and.arrow.down") {
                    Task {
                        do { try await PhotoIO.saveToPhotos(u); app.flash("Saved to Photos") } catch { app.message = describe(error) }
                    }
                }
                pill("Open", "photo") {
                    let copy = Core.newFile("png")
                    try? FileManager.default.copyItem(at: u, to: copy)
                    app.setPhoto(copy)
                    app.mode = .upscale
                }
            }
        } else {
            PrimaryButton(title: "Download full resolution", icon: "arrow.down.circle") { cloud.download(j, app: app) }
        }
    }

    private func pill(_ t: String, _ i: String, _ f: @escaping () -> Void) -> some View {
        Button(action: f) {
            Label(t, systemImage: i).font(.subheadline.weight(.semibold)).frame(maxWidth: .infinity).padding(.vertical, 11)
                .background(Theme.card2, in: Capsule())
        }
        .buttonStyle(.plain)
    }

    private func sendPicked(_ items: [PhotosPickerItem]) {
        guard !items.isEmpty else { return }
        picks = []
        let settings = choice.cloudSettings
        let scale = Int(choice.scale)
        Task {
            for it in items {
                if let url = try? await PhotoIO.importItem(it) {
                    let info = try? Core.engine.imageInfo(path: url.path)
                    let label = info.map { "\($0.width)×\($0.height) → \(scale)×" } ?? "Photo"
                    cloud.send(url, settings: settings, label: label, app: app)
                }
            }
        }
    }

    // MARK: on-device history

    @ViewBuilder
    private var historyGrid: some View {
        if history.isEmpty {
            Text("Upscales made on this iPhone show up here.").foregroundStyle(Theme.dim).padding(.top, 60)
        }
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
            ForEach(history, id: \.id) { e in
                VStack(alignment: .leading, spacing: 4) {
                    AsyncThumb(path: e.thumbPath)
                        .frame(height: 130).frame(maxWidth: .infinity)
                        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                    Text("\(e.outWidth) × \(e.outHeight)").font(.caption.monospacedDigit())
                    Text("\(UpscaleChoice.name(e.model)) · \(formatDuration(e.seconds))").font(.caption2).foregroundStyle(Theme.dim)
                }
                .contextMenu {
                    ShareLink(item: URL(fileURLWithPath: e.resultPath))
                    Button {
                        Task { try? await PhotoIO.saveToPhotos(URL(fileURLWithPath: e.resultPath)); app.flash("Saved to Photos") }
                    } label: { Label("Save to Photos", systemImage: "square.and.arrow.down") }
                    Button {
                        let copy = Core.newFile(URL(fileURLWithPath: e.resultPath).pathExtension)
                        try? FileManager.default.copyItem(at: URL(fileURLWithPath: e.resultPath), to: copy)
                        app.setPhoto(copy)
                        app.mode = .edit
                    } label: { Label("Edit", systemImage: "slider.horizontal.3") }
                    Button(role: .destructive) { try? Core.history.delete(id: e.id); history = Core.history.list() } label: { Label("Delete", systemImage: "trash") }
                }
            }
        }
    }
}

struct AsyncThumb: View {
    let path: String
    @State private var img: UIImage?
    var body: some View {
        ZStack {
            Theme.card2
            if let img { Image(uiImage: img).resizable().scaledToFill() }
        }
        .task(id: path) { img = UIImage(contentsOfFile: path) }
    }
}
