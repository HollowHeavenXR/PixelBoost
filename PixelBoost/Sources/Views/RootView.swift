import SwiftUI

struct RootView: View {
    @EnvironmentObject var app: AppModel

    var body: some View {
        TabView(selection: $app.tab) {
            UpscaleScreen().tag(AppModel.Tab.upscale)
                .tabItem { Label("Upscale", systemImage: "arrow.up.left.and.arrow.down.right") }
            EditScreen().tag(AppModel.Tab.edit)
                .tabItem { Label("Edit", systemImage: "slider.horizontal.3") }
            BatchScreen().tag(AppModel.Tab.batch)
                .tabItem { Label("Batch", systemImage: "square.stack.3d.up") }
            HistoryScreen().tag(AppModel.Tab.history)
                .tabItem { Label("History", systemImage: "clock") }
            SettingsScreen().tag(AppModel.Tab.settings)
                .tabItem { Label("Settings", systemImage: "gearshape") }
        }
        .background(Theme.canvas)
        .alert("PixelBoost", isPresented: Binding(get: { app.message != nil }, set: { if !$0 { app.message = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(app.message ?? "")
        }
    }
}
