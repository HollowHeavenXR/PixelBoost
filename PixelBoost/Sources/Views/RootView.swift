import SwiftUI

/// One canvas, three modes, a floating command bar.
struct RootView: View {
    @EnvironmentObject var app: AppModel
    @ObservedObject var cloud = CloudStore.shared
    @StateObject private var job = UpscaleJob()
    @Environment(\.scenePhase) private var phase

    var body: some View {
        ZStack(alignment: .bottom) {
            Theme.ink.ignoresSafeArea()
            Group {
                switch app.mode {
                case .upscale: StudioView(job: job)
                case .edit: EditScreen()
                case .cloud: CloudScreen()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            CommandBar(readyBadge: cloud.readyCount, active: cloud.activeCount)
                .padding(.bottom, 6)
            if let t = app.toast {
                Text(t).font(.subheadline.weight(.semibold))
                    .padding(.horizontal, 18).padding(.vertical, 12)
                    .background(Theme.card2, in: Capsule())
                    .overlay(Capsule().stroke(Theme.amber.opacity(0.5)))
                    .padding(.bottom, 96)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        .animation(.spring(duration: 0.3), value: app.toast)
        .sheet(isPresented: $app.showSettings) { SettingsScreen().presentationBackground(Theme.ink) }
        .alert("PixelBoost", isPresented: Binding(get: { app.message != nil }, set: { if !$0 { app.message = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(app.message ?? "")
        }
        .task { await cloud.refresh() }
        .onChange(of: phase) { _, p in
            if p == .active { Task { await cloud.refresh() } }
        }
    }
}

struct CommandBar: View {
    @EnvironmentObject var app: AppModel
    let readyBadge: Int
    let active: Int

    var body: some View {
        HStack(spacing: 4) {
            item(.upscale, "Upscale", "arrow.up.left.and.arrow.down.right", 0)
            item(.edit, "Edit", "slider.horizontal.3", 0)
            item(.cloud, "Cloud", active > 0 ? "icloud.and.arrow.up" : "icloud", readyBadge)
        }
        .padding(5)
        .background(Theme.card.opacity(0.96), in: Capsule())
        .overlay(Capsule().stroke(Theme.line))
        .shadow(color: .black.opacity(0.5), radius: 16, y: 6)
        .padding(.horizontal, 40)
    }

    private func item(_ m: AppModel.Mode, _ title: String, _ icon: String, _ badge: Int) -> some View {
        let on = app.mode == m
        return Button {
            app.mode = m
        } label: {
            HStack(spacing: 6) {
                Image(systemName: icon).font(.system(size: 15, weight: .semibold))
                if on { Text(title).font(.subheadline.weight(.bold)) }
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 13)
            .background(on ? AnyShapeStyle(Theme.energy) : AnyShapeStyle(Color.clear), in: Capsule())
            .foregroundStyle(on ? Color.black : Color.white)
            .overlay(alignment: .topTrailing) {
                if badge > 0 {
                    Text("\(badge)").font(.caption2.bold()).foregroundStyle(.white)
                        .padding(5).background(Theme.magenta, in: Circle()).offset(x: -6, y: -2)
                }
            }
        }
        .buttonStyle(.plain)
        .animation(.spring(duration: 0.25), value: on)
    }
}
