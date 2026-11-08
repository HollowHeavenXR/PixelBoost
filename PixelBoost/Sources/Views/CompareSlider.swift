import SwiftUI

/// Before/after with a draggable divider.
struct CompareSlider: View {
    let before: UIImage
    let after: UIImage
    @State private var split: CGFloat = 0.5

    var body: some View {
        GeometryReader { geo in
            let fitted = fit(after.size, in: geo.size)
            ZStack(alignment: .topLeading) {
                Image(uiImage: after).resizable().interpolation(.high).frame(width: fitted.width, height: fitted.height)
                Image(uiImage: before).resizable().interpolation(.none).frame(width: fitted.width, height: fitted.height)
                    .mask(alignment: .leading) { Rectangle().frame(width: fitted.width * split) }
                Rectangle().fill(.white).frame(width: 2, height: fitted.height).offset(x: fitted.width * split - 1)
                HStack {
                    Text("Before").font(.caption2.bold()).padding(5).background(.black.opacity(0.6), in: Capsule())
                    Spacer()
                    Text("After").font(.caption2.bold()).padding(5).background(.black.opacity(0.6), in: Capsule())
                }
                .padding(8).frame(width: fitted.width)
            }
            .frame(width: fitted.width, height: fitted.height)
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { v in
                split = min(1, max(0, v.location.x / max(fitted.width, 1)))
            })
            .position(x: geo.size.width / 2, y: geo.size.height / 2)
        }
    }

    private func fit(_ s: CGSize, in box: CGSize) -> CGSize {
        let k = min(box.width / max(s.width, 1), box.height / max(s.height, 1))
        return CGSize(width: s.width * k, height: s.height * k)
    }
}
