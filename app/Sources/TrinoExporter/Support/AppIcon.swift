import AppKit
import SwiftUI

/// The macOS app icon.
///
/// Drawn from the same lattice as the in-app mark (`hiveCells`), so the Dock and the sidebar
/// cannot drift apart. Where the sidebar mark is two colours on the app's dark canvas, the icon
/// inverts it: a CleanMyMac gradient tile carrying the comb in white, which is what survives
/// being shrunk to 16pt in the Dock.
///
/// Regenerate with `app/make-icon.sh` — never by hand.
struct QueryHiveIcon: View {
    var size: CGFloat = 1024
    /// macOS asks for 16 and 32 point artwork as well as the big sizes, and the seven-cell comb
    /// turns to a smudge at those — measured, not assumed: at 32pt only the lit centre survived
    /// and the icon read as a dot on a blue square. The compact variant is the same identity
    /// distilled to one bold cell, which is also what the sidebar mark uses.
    var compact = false

    /// Apple's icon grid: the squircle is about 824/1024 of the canvas, with a corner ratio of
    /// roughly 0.225. Getting these two numbers right is most of what makes an icon sit
    /// correctly next to Apple's own in the Dock.
    private var tile: CGFloat { size * 0.805 }
    private var radius: CGFloat { tile * 0.225 }

    var body: some View {
        ZStack {
            // No baked-in drop shadow: the Dock draws its own, and two of them read as a smudge.
            // The mark's own pair, pinned: this render becomes assets/icon.icns, a fixed file no
            // user setting can reach, so it must not follow the accent the user happened to pick.
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(LinearGradient(colors: [Tone.brandGlow, Tone.brandDeep],
                                     startPoint: .topLeading, endPoint: .bottomTrailing))
                .overlay {
                    // Top sheen, the glossy highlight every CleanMyMac tile carries.
                    RoundedRectangle(cornerRadius: radius, style: .continuous)
                        .fill(LinearGradient(colors: [.white.opacity(0.22), .clear],
                                             startPoint: .top, endPoint: .center))
                }
                .overlay {
                    // A touch of weight at the bottom so the tile reads as an object, not a swatch.
                    RoundedRectangle(cornerRadius: radius, style: .continuous)
                        .fill(LinearGradient(colors: [.clear, Color(hex: 0x1B0B4A).opacity(0.28)],
                                             startPoint: .center, endPoint: .bottom))
                }
                .overlay {
                    RoundedRectangle(cornerRadius: radius, style: .continuous)
                        .strokeBorder(.white.opacity(0.25), lineWidth: size * 0.007)
                }
                .frame(width: tile, height: tile)

            // The compact glyph sits smaller than the comb does: a solid white hexagon has far
            // more ink than seven outlined cells, and at 0.64 it stopped reading as a hexagon at
            // 32pt and started reading as a white square.
            comb
                .frame(width: tile * (compact ? 0.62 : 0.58),
                       height: tile * (compact ? 0.62 : 0.58))
                .shadow(color: Color(hex: 0x1B0B4A).opacity(0.35), radius: size * 0.012, y: size * 0.006)
        }
        .frame(width: size, height: size)
    }

    /// The lit centre cell is solid; the six around it are outlined so the comb still has
    /// structure when the whole thing is 16pt wide, instead of turning into one white blob.
    private var comb: some View {
        Canvas { context, canvasSize in
            if compact {
                // A hexagonal *ring*, not a disc. A solid fill at this size has no silhouette to
                // read -- the vertices anti-alias away and it looks like a rounded square -- while
                // the negative space in the middle defines the shape even at 16pt. Drawn as one
                // very thick stroke so the corners stay mitered and hexagonal.
                let radius = min(canvasSize.width, canvasSize.height) / 2 * 0.98
                let centre = CGPoint(x: canvasSize.width / 2, y: canvasSize.height / 2)
                context.stroke(hexagonPath(centre: centre, radius: radius * 0.72),
                               with: .color(.white),
                               style: StrokeStyle(lineWidth: radius * 0.52, lineJoin: .miter))
                return
            }
            let unit = min(canvasSize.width, canvasSize.height) / 5.5
            for cell in hiveCells(in: canvasSize) {
                if cell.lit {
                    context.fill(cell.path, with: .color(.white))
                } else {
                    context.fill(cell.path, with: .color(.white.opacity(0.10)))
                    context.stroke(cell.path, with: .color(.white.opacity(0.92)),
                                   lineWidth: max(1, unit * 0.12))
                }
            }
        }
    }
}

/// `--icon <path>` renders the icon to a PNG and exits, so `app/make-icon.sh` can build the
/// `.icns` from the same code the UI draws from.
enum AppIconRenderer {
    static func requestedPath() -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--icon"), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    /// `--icon-compact` renders the small-size artwork instead of the full comb.
    static func requestedCompact() -> Bool {
        CommandLine.arguments.contains("--icon-compact")
    }

    @MainActor
    static func run(path: String, compact: Bool) -> Never {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let renderer = ImageRenderer(content: QueryHiveIcon(size: 1024, compact: compact))
        renderer.scale = 1
        guard let image = renderer.nsImage,
              let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("icon: ImageRenderer produced nothing\n".utf8))
            exit(1)
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            print("icon: wrote \(path) (\(bitmap.pixelsWide)x\(bitmap.pixelsHigh), \(png.count) bytes)")
            exit(0)
        } catch {
            FileHandle.standardError.write(Data("icon: \(error.localizedDescription)\n".utf8))
            exit(1)
        }
    }
}

/// `--icon-sheet <path>` draws the icon at every size macOS asks for, on one canvas.
///
/// Judging the small artwork on its own is how a mark that reads at 1024 but turns into a smear
/// at 32 gets shipped. Each entry is rendered at its real size with the variant `make-icon.sh`
/// actually puts in that slot.
struct QueryHiveIconSheet: View {
    private let sizes: [CGFloat] = [256, 128, 64, 48, 32, 16]

    var body: some View {
        HStack(alignment: .bottom, spacing: 26) {
            ForEach(sizes, id: \.self) { size in
                VStack(spacing: 12) {
                    QueryHiveIcon(size: size, compact: size <= 32)
                        .frame(width: size, height: size)
                    Text("\(Int(size))")
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.45))
                }
            }
        }
        .padding(44)
        .background(Tone.canvas)
    }
}

extension AppIconRenderer {
    static func requestedSheetPath() -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--icon-sheet"), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    @MainActor
    static func runSheet(path: String) -> Never {
        // The sheet's own background is `Tone.canvas`, which follows the theme; pin it so an icon
        // build renders the same sheet regardless of who ran it. The mark itself is drawn in
        // `Tone.brandGlow`/`brandDeep`, which no setting can reach.
        ThemeStore.shared.pin(theme: .midnight, accent: .ice)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let renderer = ImageRenderer(content: QueryHiveIconSheet())
        renderer.scale = 2
        guard let image = renderer.nsImage,
              let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("icon: sheet render failed\n".utf8))
            exit(1)
        }
        try? png.write(to: URL(fileURLWithPath: path))
        print("icon: wrote \(path)")
        exit(0)
    }
}
