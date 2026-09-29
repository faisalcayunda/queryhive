import AppKit
import SwiftUI

/// The macOS app icon.
///
/// Drawn from the same lattice as the in-app mark (`hiveCells`), so the Dock and the sidebar
/// cannot drift apart. Where the sidebar mark is two colours on the app's dark canvas, the icon
/// inverts it: a deep emissive tile carrying the comb in white.
///
/// ## Why the tile went dark
///
/// It used to be a bright cyan-to-violet gradient with the mark in white, and the mark's contrast
/// against that surface was the problem: **1.67:1 at the cyan corner**, well under the 3:1 that
/// non-text elements are expected to clear. The comb was legible only because of the drop shadow
/// under it. Measured with WCAG relative luminance against the two gradient ends: white on
/// `#4FD8FF` is 1.67:1, on `#7B61FF` is 4.20:1. A dark base puts the same white at 10.1:1 and
/// 18.1:1, so the mark stops depending on an effect to be seen.
///
/// That is also what makes it read as a tool for data engineers rather than a consumer utility:
/// the instrument-panel vocabulary is a dark field with an emissive subject. Cyan does not leave —
/// it is demoted from the surface to the *light*, as a core glow behind the comb and a rim light on
/// the tile edge. The rim is load-bearing rather than decoration: on a dark Dock (`#1C1C1E`) the
/// dark tile is 1.06:1 against its background, and the rim is what gives it an edge (13.4:1).
///
/// Regenerate with `app/make-icon.sh` — never by hand.
struct QueryHiveIcon: View {
    var size: CGFloat = 1024
    /// macOS asks for 16 and 32 point artwork as well as the big sizes, and the seven-cell comb
    /// turns to a smudge at those — measured, not assumed: at 32pt only the lit centre survived
    /// and the icon read as a dot on a blue square. The compact variant is the same identity
    /// distilled to one bold ring.
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
                .fill(LinearGradient(colors: [Tone.brandTileTop, Tone.brandTileBase],
                                     startPoint: .topLeading, endPoint: .bottomTrailing))
                .overlay {
                    // The emissive core: cyan light rising from behind the comb rather than a
                    // sheen lying on top of it. Off-centre and above middle, because a light
                    // source exactly at the centre reads as a printed gradient instead of light.
                    //
                    // Deliberately tight and dim. A tile-wide wash at 0.34 was measured to reach
                    // luminance 97-130 across the middle of the tile, which is the same as the
                    // quiet cells at 0.26 white — so the wash erased the six cells it was meant to
                    // sit behind, and filled the gaps between them until the comb resolved as two
                    // blobs instead of seven shapes. Light that outshines its subject is not light.
                    RadialGradient(colors: [Tone.brandGlow.opacity(0.24), .clear],
                                   center: UnitPoint(x: 0.5, y: 0.44),
                                   startRadius: 0,
                                   endRadius: tile * 0.45)
                }
                .overlay {
                    // Rim light. The one effect on this tile that is not decoration: it is what
                    // separates a near-black tile from a near-black Dock. Two colours so the edge
                    // agrees with the core above and the violet base below.
                    RoundedRectangle(cornerRadius: radius, style: .continuous)
                        .strokeBorder(LinearGradient(colors: [Tone.brandRim, Tone.brandDeep.opacity(0.14)],
                                                     startPoint: .top, endPoint: .bottom),
                                      lineWidth: size * 0.011)
                }
                .frame(width: tile, height: tile)

            // The compact glyph sits smaller than the comb does: a solid white hexagon has far
            // more ink than seven outlined cells, and at 0.64 it stopped reading as a hexagon at
            // 32pt and started reading as a white square.
            comb
                .frame(width: tile * (compact ? 0.62 : 0.58),
                       height: tile * (compact ? 0.62 : 0.58))
        }
        .frame(width: size, height: size)
    }

    /// One lit cell that emits, and six that are present but quiet.
    ///
    /// The six used to be *outlined* rather than filled, and the outline was the wrong instrument:
    /// a hairline stroke is the first thing to vanish when an icon is scaled down, so the mark's
    /// structure depended on exactly the detail that could not survive. Flat fills carry structure
    /// as *tone* instead, which averages cleanly at every size. The lit cell keeps a bloom, because
    /// a large soft shape is also something that survives downscaling — it is the hairlines that do
    /// not.
    private var comb: some View {
        Canvas { context, canvasSize in
            if compact {
                // A hexagonal *ring*, not a disc. A solid fill at this size has no silhouette to
                // read -- the vertices anti-alias away and it looks like a rounded square -- while
                // the negative space in the middle defines the shape even at 16pt. Drawn as one
                // very thick stroke so the corners stay mitered and hexagonal.
                //
                // Honest limit: at 16pt a regular hexagon cannot really read *as* a hexagon -- six
                // vertices across five pixels is under a pixel each. What carries it is the flat top
                // and bottom edges, which is why the ring is drawn with a flat-top orientation.
                let radius = min(canvasSize.width, canvasSize.height) / 2 * 0.98
                let centre = CGPoint(x: canvasSize.width / 2, y: canvasSize.height / 2)
                context.stroke(hexagonPath(centre: centre, radius: radius * 0.72),
                               with: .color(.white),
                               style: StrokeStyle(lineWidth: radius * 0.52, lineJoin: .miter))
                return
            }
            let unit = min(canvasSize.width, canvasSize.height) / 5.5
            // 0.80, not 0.94: at 0.94 the gap between neighbouring cells is 0.104r, which is 0.57px
            // at 64pt and fuses all seven hexagons into a single blob. At 0.80 the gap is 0.346r —
            // 2.0px at 64, 4.0px at 128 — and the comb reads as a comb.
            for cell in hiveCells(in: canvasSize, inset: 0.80) {
                if cell.lit {
                    // The bloom goes down *first*, under the cell, so the light spills around a
                    // white shape rather than tinting it cyan. Drawn as a second blurred copy of
                    // the same path: a shadow would be clipped to the outside of the shape, and a
                    // shadow of a white cell is not what "emissive" means.
                    context.drawLayer { glow in
                        glow.addFilter(.blur(radius: unit * 0.22))
                        glow.fill(cell.path, with: .color(Tone.brandGlow.opacity(0.55)))
                    }
                    context.fill(cell.path, with: .linearGradient(
                        Gradient(colors: [.white, Color(hex: 0xE6F7FF)]),
                        startPoint: CGPoint(x: 0, y: 0),
                        endPoint: CGPoint(x: canvasSize.width, y: canvasSize.height)))
                } else {
                    // 0.38, not 0.20: the quiet cells have to out-read the core glow behind them,
                    // and 0.20 white measured below the glow's own luminance across the middle of
                    // the tile — the cells disappeared into the light. At 0.38 they sit clearly
                    // above it while staying well below the lit cell, which is what keeps the comb
                    // legible as seven shapes rather than one bright smear.
                    context.fill(cell.path, with: .color(.white.opacity(0.38)))
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
                        .font(.code(11))
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
