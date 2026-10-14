import AppKit

/// The equalizer's own window: opened from the transport bar's quick panel,
/// the Playback menu or ⌥⌘E, and left open beside the library while music
/// plays, the way Music's equalizer window is. It always draws over the dark
/// artwork wash, like the other artwork screens, and owns the undo manager its
/// gestures register with.
@MainActor
final class MacEqualizerWindowController: NSWindowController, NSWindowDelegate {

    static let shared = MacEqualizerWindowController()

    private let undo = UndoManager()

    private convenience init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1080, height: 700),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = EqualizerCopy.title
        window.titlebarAppearsTransparent = true
        window.appearance = NSAppearance(named: .darkAqua)
        window.backgroundColor = .black
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 940, height: 640)
        window.center()
        window.setFrameAutosaveName("FlaccyEqualizerWindow")
        self.init(window: window)
        window.delegate = self
        contentViewController = MacEqualizerViewController()
    }

    var isVisible: Bool {
        window?.isVisible == true
    }

    func show() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    func toggle() {
        if isVisible, window?.isKeyWindow == true {
            window?.orderOut(nil)
        } else {
            show()
        }
    }

    nonisolated func windowWillReturnUndoManager(_ window: NSWindow) -> UndoManager? {
        MainActor.assumeIsolated { undo }
    }

    override func synchronizeWindowTitleWithDocumentName() {}
}
