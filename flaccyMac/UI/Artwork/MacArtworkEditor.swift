import AppKit

/// Opens the artwork editor as a sheet on a window and reports how it ended
/// with the toast the rest of the app uses. One editor at a time, and never
/// over a sheet that is already up.
@MainActor
enum MacArtworkEditor {

    private static weak var activeWindow: NSWindow?

    private static let baseContentSize = NSSize(width: 960, height: 760)
    private static let minimumContentSize = NSSize(width: 760, height: 600)
    private static let parentMargin: CGFloat = 60

    @discardableResult
    static func present(
        for album: Album, initial: ArtworkInput? = nil, from window: NSWindow?
    ) -> ArtworkEditorViewController? {
        guard let parent = window ?? NSApp.keyWindow ?? NSApp.mainWindow,
              parent.attachedSheet == nil, activeWindow == nil
        else {
            NSSound.beep()
            return nil
        }
        let controller = ArtworkEditorViewController(album: album, initial: initial)
        let sheet = NSWindow(contentViewController: controller)
        sheet.styleMask = [.titled, .resizable]
        sheet.appearance = NSAppearance(named: .darkAqua)
        sheet.isReleasedWhenClosed = false
        let sizes = contentSizes(in: parent)
        sheet.contentMinSize = sizes.minimum
        sheet.setContentSize(sizes.preferred)
        activeWindow = sheet

        var outcome = ArtworkEditorViewController.Outcome.cancelled
        controller.onFinish = { [weak parent, weak sheet] result in
            outcome = result
            guard let parent, let sheet else { return }
            parent.endSheet(sheet)
        }
        parent.beginSheet(sheet) { [weak parent] _ in
            MainActor.assumeIsolated {
                activeWindow = nil
                guard let parent else { return }
                announce(outcome, in: parent)
            }
        }
        return controller
    }

    private static func announce(_ outcome: ArtworkEditorViewController.Outcome, in window: NSWindow) {
        switch outcome {
        case .saved: MacToast.show(String(localized: "Artwork updated"), style: .success, in: window)
        case .restored: MacToast.show(String(localized: "Original artwork restored"), style: .success, in: window)
        case .cancelled: break
        }
    }

    /// The sheet grows with the interface zoom like the window beneath it, and
    /// is held inside that window so it never hangs over an edge.
    private static func contentSizes(in parent: NSWindow) -> (minimum: NSSize, preferred: NSSize) {
        let scale = MacInterfaceScale.current
        let room = NSSize(
            width: max(parent.frame.width - parentMargin, 320),
            height: max(parent.frame.height - parentMargin, 320)
        )
        func fitted(_ size: NSSize) -> NSSize {
            NSSize(width: min(size.width * scale, room.width), height: min(size.height * scale, room.height))
        }
        return (fitted(minimumContentSize), fitted(baseContentSize))
    }
}
