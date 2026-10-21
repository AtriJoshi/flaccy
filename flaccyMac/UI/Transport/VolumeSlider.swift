import AppKit

final class VolumeSlider: NSSlider {

    convenience init() {
        self.init(frame: .zero)
    }

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        cell = VolumeSliderCell()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        cell = VolumeSliderCell()
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: super.intrinsicContentSize.width, height: 24)
    }

    override func scrollWheel(with event: NSEvent) {
        let delta = Double(event.scrollingDeltaY + event.scrollingDeltaX)
        guard delta != 0 else { return }
        let step = event.hasPreciseScrollingDeltas ? 0.004 : 0.03
        doubleValue = min(maxValue, max(minValue, doubleValue + delta * step))
        sendAction(action, to: target)
    }
}

private final class VolumeSliderCell: NSSliderCell {

    private let trackHeight: CGFloat = 4
    private let knobDiameter: CGFloat = 18

    override func drawBar(inside rect: NSRect, flipped: Bool) {
        let track = NSRect(
            x: rect.minX + knobDiameter / 2,
            y: rect.midY - trackHeight / 2,
            width: max(0, rect.width - knobDiameter),
            height: trackHeight
        )
        NSColor.labelColor.withAlphaComponent(0.16).setFill()
        NSBezierPath(roundedRect: track, xRadius: trackHeight / 2, yRadius: trackHeight / 2).fill()

        let span = maxValue - minValue
        let fraction = span > 0 ? CGFloat((doubleValue - minValue) / span) : 0
        let filled = NSRect(
            x: track.minX,
            y: track.minY,
            width: track.width * min(max(fraction, 0), 1),
            height: track.height
        )
        NSColor.labelColor.setFill()
        NSBezierPath(roundedRect: filled, xRadius: trackHeight / 2, yRadius: trackHeight / 2).fill()
    }

    override func drawKnob(_ knobRect: NSRect) {
        let rect = NSRect(
            x: knobRect.midX - knobDiameter / 2,
            y: knobRect.midY - knobDiameter / 2,
            width: knobDiameter,
            height: knobDiameter
        )
        NSColor.labelColor.setFill()
        NSBezierPath(ovalIn: rect).fill()
    }
}
