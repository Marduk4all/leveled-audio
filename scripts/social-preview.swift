// Erzeugt das Vorschaubild für geteilte Links (GitHub „Social preview“, 1280×640).
//
// Aufruf (nur macOS):  swift scripts/social-preview.swift [Ziel.png]
//   Default: docs/images/social-preview.png
// Nutzt assets/icon.png und docs/images/gui-settings-en.png.

import AppKit

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let out = CommandLine.arguments.count > 1
    ? URL(fileURLWithPath: CommandLine.arguments[1])
    : root.appendingPathComponent("docs/images/social-preview.png")

let W = 1280, H = 640
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: W, pixelsHigh: H, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext

func rgb(_ r: Int, _ g: Int, _ b: Int, _ a: CGFloat = 1) -> NSColor {
    NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: a)
}

// Hintergrund: Verlauf wie das Icon (Indigo → Türkis)
let bg = NSGradient(colors: [rgb(0x3E, 0x2F, 0xB8), rgb(0x2B, 0x5F, 0xC0), rgb(0x0F, 0xA3, 0x96)])!
bg.draw(in: NSRect(x: 0, y: 0, width: W, height: H), angle: -25)

// Dezente Pegelbalken im Hintergrund
ctx.saveGState()
rgb(255, 255, 255, 0.06).setFill()
var x: CGFloat = 40
for h in [120, 260, 90, 200, 160, 220, 220, 220, 220, 140, 300, 110] {
    let bh = CGFloat(h)
    NSBezierPath(roundedRect: NSRect(x: x, y: 40, width: 26, height: bh), xRadius: 13, yRadius: 13).fill()
    x += 52
}
ctx.restoreGState()

// Icon
if let icon = NSImage(contentsOf: root.appendingPathComponent("assets/icon.png")) {
    icon.draw(in: NSRect(x: 52, y: 408, width: 176, height: 176))
}

func text(_ s: String, _ font: NSFont, _ color: NSColor, _ rect: NSRect, spacing: CGFloat = 0) {
    let p = NSMutableParagraphStyle()
    p.lineSpacing = spacing
    NSAttributedString(string: s, attributes: [.font: font, .foregroundColor: color, .paragraphStyle: p])
        .draw(with: rect, options: [.usesLineFragmentOrigin])
}

let left: CGFloat = 70
text("Leveled Audio", .systemFont(ofSize: 62, weight: .bold), .white,
     NSRect(x: 236, y: 458, width: 460, height: 90))
text("Automatic leveling, noise reduction\nand loudness normalization\nfor spoken-word audio",
     .systemFont(ofSize: 34, weight: .medium), rgb(255, 255, 255, 0.95),
     NSRect(x: left, y: 228, width: 640, height: 160), spacing: 4)

// Use-Case-Chips
let gold = rgb(0xFF, 0xC8, 0x57)
var cx = left
for label in ["Podcasts", "Video", "Interviews", "Voice-over"] {
    let font = NSFont.systemFont(ofSize: 24, weight: .semibold)
    let w = (label as NSString).size(withAttributes: [.font: font]).width + 36
    let r = NSRect(x: cx, y: 160, width: w, height: 44)
    rgb(255, 255, 255, 0.16).setFill()
    NSBezierPath(roundedRect: r, xRadius: 22, yRadius: 22).fill()
    gold.setStroke()
    let border = NSBezierPath(roundedRect: r.insetBy(dx: 1, dy: 1), xRadius: 21, yRadius: 21)
    border.lineWidth = 2
    border.stroke()
    text(label, font, .white, NSRect(x: cx + 18, y: 167, width: w, height: 34))
    cx += w + 14
}

text("LUFS · EBU R128 · batch · offline\nFree & open source · macOS · Windows · Linux",
     .systemFont(ofSize: 22, weight: .regular), rgb(255, 255, 255, 0.85),
     NSRect(x: left, y: 52, width: 660, height: 70), spacing: 4)

// Screenshot der App rechts, mit Schatten und runden Ecken
if let shot = NSImage(contentsOf: root.appendingPathComponent("docs/images/gui-settings-en.png")) {
    let h: CGFloat = 530
    let w = h * shot.size.width / shot.size.height
    let r = NSRect(x: CGFloat(W) - w - 50, y: (CGFloat(H) - h) / 2, width: w, height: h)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 30, color: NSColor.black.withAlphaComponent(0.4).cgColor)
    NSColor.white.setFill()
    NSBezierPath(roundedRect: r, xRadius: 16, yRadius: 16).fill()
    ctx.restoreGState()
    ctx.saveGState()
    NSBezierPath(roundedRect: r, xRadius: 16, yRadius: 16).addClip()
    shot.draw(in: r)
    ctx.restoreGState()
}

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: out)
print(out.path)
