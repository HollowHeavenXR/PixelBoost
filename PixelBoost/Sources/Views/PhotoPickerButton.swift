import PhotosUI
import SwiftUI

struct PhotoPickerButton: View {
    @EnvironmentObject var app: AppModel
    var title = "Choose Photo"
    @State private var item: PhotosPickerItem?

    var body: some View {
        PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) {
            Label(title, systemImage: "photo.on.rectangle")
                .fontWeight(.semibold)
                .frame(maxWidth: 280)
                .padding(.vertical, 13)
                .background(Theme.accent, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .foregroundStyle(.black)
        }
        .onChange(of: item) { _, new in
            guard let new else { return }
            Task {
                do { app.setPhoto(try await PhotoIO.importItem(new)) } catch { app.message = describe(error) }
                item = nil
            }
        }
    }
}
