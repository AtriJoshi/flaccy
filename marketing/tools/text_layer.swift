import AppKit
import Foundation

struct Spec: Decodable {
    let out: String
    let text: String
    let width: CGFloat
    let size: CGFloat
    let minSize: CGFloat?
    let maxLines: Int?
    let weight: String?
    let family: String?
    let color: String?
    let accent: [String]?
    let accentColor: String?
    let tracking: CGFloat?
    let lineHeight: CGFloat?
    let align: String?
    let uppercase: Bool?
    let shadow: Bool?
}

struct Metrics: Encodable {
    let out: String
    let width: Int
    let height: Int
    let size: CGFloat
    let lines: Int
}

func color(_ hex: String) -> NSColor {
    let value = UInt32(hex.dropFirst(), radix: 16) ?? 0xFFFFFF
    return NSColor(
        srgbRed: CGFloat((value >> 16) & 0xFF) / 255,
        green: CGFloat((value >> 8) & 0xFF) / 255,
        blue: CGFloat(value & 0xFF) / 255,
        alpha: 1
    )
}

func weight(_ name: String?) -> NSFont.Weight {
    switch name {
    case "regular": return .regular
    case "medium": return .medium
    case "semibold": return .semibold
    case "bold": return .bold
    case "black": return .black
    default: return .heavy
    }
}

func font(_ spec: Spec, size: CGFloat) -> NSFont {
    let w = weight(spec.weight)
    if let family = spec.family, family != "system" {
        let descriptor = NSFontDescriptor(fontAttributes: [
            .family: family,
            .traits: [NSFontDescriptor.TraitKey.weight: w.rawValue],
        ])
        if let f = NSFont(descriptor: descriptor, size: size) { return f }
    }
    return NSFont.systemFont(ofSize: size, weight: w)
}

func attributed(_ spec: Spec, size: CGFloat) -> NSAttributedString {
    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = spec.align == "left" ? .left : (spec.align == "right" ? .right : .center)
    paragraph.lineHeightMultiple = spec.lineHeight ?? 1.04
    paragraph.lineBreakMode = .byWordWrapping
    let base = font(spec, size: size)
    let text = (spec.uppercase ?? false) ? spec.text.uppercased() : spec.text
    let result = NSMutableAttributedString(string: text, attributes: [
        .font: base,
        .foregroundColor: color(spec.color ?? "#FFFFFF"),
        .kern: (spec.tracking ?? 0),
        .paragraphStyle: paragraph,
    ])
    let accentColor = color(spec.accentColor ?? "#FF4D6D")
    for fragment in spec.accent ?? [] {
        let needle = (spec.uppercase ?? false) ? fragment.uppercased() : fragment
        var searchRange = NSRange(location: 0, length: result.length)
        while let range = Range(searchRange, in: text), let found = text.range(of: needle, range: range) {
            let nsRange = NSRange(found, in: text)
            result.addAttribute(.foregroundColor, value: accentColor, range: nsRange)
            searchRange = NSRange(location: nsRange.upperBound, length: result.length - nsRange.upperBound)
        }
    }
    return result
}

func lineCount(_ string: NSAttributedString, width: CGFloat) -> Int {
    let storage = NSTextStorage(attributedString: string)
    let container = NSTextContainer(size: NSSize(width: width, height: .greatestFiniteMagnitude))
    container.lineFragmentPadding = 0
    let layout = NSLayoutManager()
    layout.addTextContainer(container)
    storage.addLayoutManager(layout)
    _ = layout.glyphRange(for: container)
    var count = 0
    var index = 0
    let glyphs = layout.numberOfGlyphs
    while index < glyphs {
        var range = NSRange()
        layout.lineFragmentRect(forGlyphAt: index, effectiveRange: &range)
        count += 1
        index = NSMaxRange(range)
    }
    return count
}

func render(_ spec: Spec) -> Metrics {
    var size = spec.size
    let minSize = spec.minSize ?? spec.size * 0.6
    var string = attributed(spec, size: size)
    if let maxLines = spec.maxLines {
        while lineCount(string, width: spec.width) > maxLines && size > minSize {
            size -= 2
            string = attributed(spec, size: size)
        }
    }
    let bounds = string.boundingRect(
        with: NSSize(width: spec.width, height: .greatestFiniteMagnitude),
        options: [.usesLineFragmentOrigin, .usesFontLeading]
    )
    let pad: CGFloat = (spec.shadow ?? false) ? 40 : 4
    let width = Int(ceil(spec.width + pad * 2))
    let height = Int(ceil(bounds.height + pad * 2))
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0
    )!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    NSColor.clear.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()
    if spec.shadow ?? false {
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(0.45)
        shadow.shadowBlurRadius = 24
        shadow.shadowOffset = NSSize(width: 0, height: -6)
        shadow.set()
    }
    string.draw(with: NSRect(x: pad, y: pad, width: spec.width, height: bounds.height + 2), options: [.usesLineFragmentOrigin, .usesFontLeading])
    NSGraphicsContext.restoreGraphicsState()
    try? rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: spec.out))
    return Metrics(out: spec.out, width: width, height: height, size: size, lines: lineCount(string, width: spec.width))
}

let input = FileHandle.standardInput.readDataToEndOfFile()
let specs = try JSONDecoder().decode([Spec].self, from: input)
let metrics = specs.map(render)
print(String(data: try JSONEncoder().encode(metrics), encoding: .utf8)!)
