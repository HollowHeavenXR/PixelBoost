import SwiftUI

struct HistoryScreen: View {
    @EnvironmentObject var app: AppModel
    @State private var entries: [HistoryEntry] = []
    @State private var bytes: UInt64 = 0

    var body: some View {
        NavigationStack {
            ScrollView {
                if entries.isEmpty {
                    Text("Finished upscales show up here.").foregroundStyle(Theme.dim).padding(.top, 80)
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                    ForEach(entries, id: \.id) { e in
                        VStack(alignment: .leading, spacing: 4) {
                            AsyncThumb(path: e.thumbPath)
                                .frame(height: 130).frame(maxWidth: .infinity)
                                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            Text("\(e.outWidth)×\(e.outHeight)").font(.caption.monospacedDigit())
                            Text("\(e.model) · \(UpscaleScreen.duration(e.seconds))").font(.caption2).foregroundStyle(Theme.dim)
                        }
                        .contextMenu {
                            ShareLink(item: URL(fileURLWithPath: e.resultPath))
                            Button { Task { try? await PhotoIO.saveToPhotos(URL(fileURLWithPath: e.resultPath)) } } label: { Label("Save to Photos", systemImage: "square.and.arrow.down") }
                            Button { app.setPhoto(copy(e.resultPath)); app.tab = .edit } label: { Label("Edit", systemImage: "slider.horizontal.3") }
                            Button { upload(e) } label: { Label("Upload to cloud (24h)", systemImage: "icloud.and.arrow.up") }
                            Button(role: .destructive) { try? Core.history.delete(id: e.id); reload() } label: { Label("Delete", systemImage: "trash") }
                        }
                    }
                }
                .padding(14)
            }
            .background(Theme.canvas)
            .navigationTitle("History")
            .toolbar {
                if !entries.isEmpty {
                    Menu {
                        Text(ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file))
                        Button("Clear all", role: .destructive) { try? Core.history.clear(); reload() }
                    } label: { Image(systemName: "ellipsis.circle") }
                }
            }
            .onAppear { reload() }
        }
    }

    private func reload() {
        entries = Core.history.list()
        bytes = Core.history.diskBytes()
    }

    private func copy(_ path: String) -> URL {
        let dest = Core.newFile(URL(fileURLWithPath: path).pathExtension)
        try? FileManager.default.copyItem(at: URL(fileURLWithPath: path), to: dest)
        return dest
    }

    private func upload(_ e: HistoryEntry) {
        Task.detached {
            let r = Result { try Core.server.uploadExport(path: e.resultPath, label: "\(e.model) \(e.outWidth)x\(e.outHeight)", ttlHours: 24) }
            await MainActor.run {
                switch r {
                case .success: app.message = "Uploaded — kept for 24 hours."
                case .failure(let err): app.message = describe(err)
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
            Theme.panel2
            if let img { Image(uiImage: img).resizable().scaledToFill() }
        }
        .task(id: path) { img = UIImage(contentsOfFile: path) }
    }
}
