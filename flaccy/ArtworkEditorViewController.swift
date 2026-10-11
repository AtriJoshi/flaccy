import PhotosUI
import UIKit
import UniformTypeIdentifiers

/// The sheet where a person chooses a cover for an album: frame the current
/// one, bring their own from Photos, Files, the clipboard or a drop, or pick
/// a higher-resolution match found online. Nothing is written until Save.
final class ArtworkEditorViewController: UIViewController {

    var onSaved: (() -> Void)?

    private let album: Album
    private let hasCustomCover: Bool
    private let cropView = ArtworkCropView()
    private let backdropImageView = UIImageView()
    private let infoLabel = UILabel()
    private let scrollView = UIScrollView()
    private let content = UIStackView()
    private let sourceRow = UIStackView()
    private let onlineHeader = UILabel()
    private let onlineSpinner = UIActivityIndicatorView(style: .medium)
    private let onlineStatus = UILabel()
    private var candidatesView: UICollectionView!
    private var candidatesDataSource: UICollectionViewDiffableDataSource<Int, ArtworkCandidate>!
    private let resetButton = UIButton(type: .system)
    private var saveButton: UIBarButtonItem!
    private var selectedCandidate: ArtworkCandidate?
    private var loadTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var hasReplacedImage = false
    private var didApplyCurrentArtwork = false
    private var thumbnails = [URL: UIImage]()

    init(album: Album) {
        self.album = album
        self.hasCustomCover = AlbumArtworkEditor.hasCustomCover(album)
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    deinit {
        loadTask?.cancel()
        searchTask?.cancel()
    }

    static func present(for album: Album, from presenter: UIViewController, onSaved: (() -> Void)? = nil) {
        let editor = ArtworkEditorViewController(album: album)
        editor.onSaved = onSaved
        let navigation = UINavigationController(rootViewController: editor)
        navigation.modalPresentationStyle = .pageSheet
        if let sheet = navigation.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
        presenter.present(navigation, animated: true)
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        overrideUserInterfaceStyle = .dark
        view.backgroundColor = .black
        title = String(localized: "Artwork")
        navigationItem.leftBarButtonItem = UIBarButtonItem(
            systemItem: .cancel, primaryAction: UIAction { [weak self] _ in self?.cancel() }
        )
        saveButton = UIBarButtonItem(
            title: String(localized: "Save"), style: .done, target: self, action: #selector(save)
        )
        saveButton.isEnabled = false
        navigationItem.rightBarButtonItem = saveButton
        isModalInPresentation = false
        navigationController?.presentationController?.delegate = self

        setupBackdrop()
        setupContent()
        setupDrop()
        loadCurrentArtwork()
        startOnlineSearch()

        NotificationCenter.default.addObserver(
            self, selector: #selector(pasteboardChanged), name: UIPasteboard.changedNotification, object: nil
        )
    }

    private var hasUnsavedChanges: Bool { hasReplacedImage || cropView.hasUserFraming }

    private func cancel() {
        guard let presentation = navigationController?.presentationController else {
            dismiss(animated: true)
            return
        }
        if hasUnsavedChanges {
            presentationControllerDidAttemptToDismiss(presentation)
        } else {
            dismiss(animated: true)
        }
    }

    private func setupBackdrop() {
        backdropImageView.contentMode = .scaleAspectFill
        backdropImageView.clipsToBounds = true
        backdropImageView.alpha = 0
        backdropImageView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(backdropImageView)

        let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemUltraThinMaterialDark))
        blur.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(blur)

        let scrim = UIView()
        scrim.backgroundColor = UIColor.black.withAlphaComponent(0.55)
        scrim.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrim)

        for overlay in [backdropImageView, blur, scrim] {
            NSLayoutConstraint.activate([
                overlay.topAnchor.constraint(equalTo: view.topAnchor),
                overlay.leadingAnchor.constraint(equalTo: view.leadingAnchor),
                overlay.trailingAnchor.constraint(equalTo: view.trailingAnchor),
                overlay.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            ])
        }
    }

    private func setupContent() {
        scrollView.alwaysBounceVertical = true
        scrollView.keyboardDismissMode = .onDrag
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrollView)

        content.axis = .vertical
        content.alignment = .fill
        content.spacing = 18
        content.translatesAutoresizingMaskIntoConstraints = false
        scrollView.addSubview(content)

        let cropHolder = UIView()
        cropHolder.translatesAutoresizingMaskIntoConstraints = false
        cropHolder.layer.shadowColor = UIColor.black.cgColor
        cropHolder.layer.shadowOpacity = 0.5
        cropHolder.layer.shadowOffset = CGSize(width: 0, height: 14)
        cropHolder.layer.shadowRadius = 28
        cropView.translatesAutoresizingMaskIntoConstraints = false
        cropView.onFramingChanged = { [weak self] in self?.refreshSaveState() }
        cropHolder.addSubview(cropView)
        let preferredWidth = cropView.widthAnchor.constraint(equalToConstant: 340)
        preferredWidth.priority = .defaultHigh
        NSLayoutConstraint.activate([
            cropView.topAnchor.constraint(equalTo: cropHolder.topAnchor),
            cropView.bottomAnchor.constraint(equalTo: cropHolder.bottomAnchor),
            cropView.centerXAnchor.constraint(equalTo: cropHolder.centerXAnchor),
            cropView.heightAnchor.constraint(equalTo: cropView.widthAnchor),
            preferredWidth,
            cropView.widthAnchor.constraint(lessThanOrEqualTo: cropHolder.widthAnchor),
            cropView.widthAnchor.constraint(lessThanOrEqualToConstant: 420),
        ])
        content.addArrangedSubview(cropHolder)

        infoLabel.font = .scaled(.footnote, size: 13, weight: .medium)
        infoLabel.textColor = UIColor.white.withAlphaComponent(0.6)
        infoLabel.textAlignment = .center
        infoLabel.numberOfLines = 2
        infoLabel.text = String(localized: "Pinch and drag to frame the cover")
        content.addArrangedSubview(infoLabel)

        sourceRow.axis = .horizontal
        sourceRow.distribution = .fillEqually
        sourceRow.spacing = 10
        sourceRow.addArrangedSubview(makeSourceButton(
            title: String(localized: "Photos"), image: "photo.on.rectangle"
        ) { [weak self] in self?.presentPhotoPicker() })
        sourceRow.addArrangedSubview(makeSourceButton(
            title: String(localized: "Files"), image: "folder"
        ) { [weak self] in self?.presentFilePicker() })
        sourceRow.addArrangedSubview(makeSourceButton(
            title: String(localized: "Paste"), image: "doc.on.clipboard"
        ) { [weak self] in self?.pasteFromClipboard() })
        content.addArrangedSubview(sourceRow)
        updatePasteAvailability()

        let headerRow = UIStackView()
        headerRow.axis = .horizontal
        headerRow.spacing = 8
        headerRow.alignment = .center
        onlineHeader.text = String(localized: "Find Online")
        onlineHeader.font = .scaled(.headline, size: 17, weight: .semibold)
        onlineHeader.textColor = .white
        onlineSpinner.color = .white
        onlineSpinner.hidesWhenStopped = true
        headerRow.addArrangedSubview(onlineHeader)
        headerRow.addArrangedSubview(onlineSpinner)
        headerRow.addArrangedSubview(UIView())
        content.addArrangedSubview(headerRow)

        setupCandidates()
        content.addArrangedSubview(candidatesView)

        onlineStatus.font = .scaled(.footnote, size: 13, weight: .regular)
        onlineStatus.textColor = UIColor.white.withAlphaComponent(0.55)
        onlineStatus.numberOfLines = 0
        onlineStatus.isHidden = true
        content.addArrangedSubview(onlineStatus)

        var resetConfig = UIButton.Configuration.plain()
        resetConfig.title = String(localized: "Use Original Artwork")
        resetConfig.image = UIImage(systemName: "arrow.uturn.backward")
        resetConfig.imagePadding = 6
        resetConfig.baseForegroundColor = .systemRed
        resetButton.configuration = resetConfig
        resetButton.addAction(UIAction { [weak self] _ in self?.confirmReset() }, for: .touchUpInside)
        resetButton.isHidden = !hasCustomCover
        content.addArrangedSubview(resetButton)

        NSLayoutConstraint.activate([
            scrollView.topAnchor.constraint(equalTo: view.topAnchor),
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            content.topAnchor.constraint(equalTo: scrollView.contentLayoutGuide.topAnchor, constant: 16),
            content.bottomAnchor.constraint(equalTo: scrollView.contentLayoutGuide.bottomAnchor, constant: -24),
            content.leadingAnchor.constraint(equalTo: scrollView.frameLayoutGuide.leadingAnchor, constant: 20),
            content.trailingAnchor.constraint(equalTo: scrollView.frameLayoutGuide.trailingAnchor, constant: -20),
            candidatesView.heightAnchor.constraint(equalToConstant: 148),
        ])
    }

    private func makeSourceButton(title: String, image: String, action: @escaping () -> Void) -> UIView {
        var config = UIButton.Configuration.plain()
        config.title = title
        config.image = UIImage(
            systemName: image, withConfiguration: UIImage.SymbolConfiguration(pointSize: 18, weight: .semibold)
        )
        config.imagePlacement = .top
        config.imagePadding = 6
        config.baseForegroundColor = .white
        config.contentInsets = NSDirectionalEdgeInsets(top: 12, leading: 8, bottom: 12, trailing: 8)
        config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { incoming in
            var outgoing = incoming
            outgoing.font = .scaled(.footnote, size: 13, weight: .semibold)
            return outgoing
        }
        let button = UIButton(configuration: config)
        button.addAction(UIAction { _ in
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        }, for: .touchUpInside)
        let glass = LiquidGlass.view(cornerRadius: 16, interactive: true)
        button.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(button)
        NSLayoutConstraint.activate([
            button.topAnchor.constraint(equalTo: glass.contentView.topAnchor),
            button.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor),
            button.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor),
            button.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor),
        ])
        return glass
    }

    private func setupCandidates() {
        let layout = UICollectionViewFlowLayout()
        layout.scrollDirection = .horizontal
        layout.itemSize = CGSize(width: 120, height: 148)
        layout.minimumLineSpacing = 12
        candidatesView = UICollectionView(frame: .zero, collectionViewLayout: layout)
        candidatesView.backgroundColor = .clear
        candidatesView.showsHorizontalScrollIndicator = false
        candidatesView.clipsToBounds = false
        candidatesView.delegate = self
        candidatesView.translatesAutoresizingMaskIntoConstraints = false
        candidatesView.isHidden = true

        let registration = UICollectionView.CellRegistration<ArtworkCandidateCell, ArtworkCandidate> { [weak self] cell, _, candidate in
            guard let self else { return }
            cell.configure(
                candidate: candidate,
                image: self.thumbnails[candidate.thumbnailURL],
                isSelected: candidate == self.selectedCandidate
            )
            if self.thumbnails[candidate.thumbnailURL] == nil {
                self.fetchThumbnail(for: candidate)
            }
        }
        candidatesDataSource = UICollectionViewDiffableDataSource(collectionView: candidatesView) { view, indexPath, candidate in
            view.dequeueConfiguredReusableCell(using: registration, for: indexPath, item: candidate)
        }
    }

    private func fetchThumbnail(for candidate: ArtworkCandidate) {
        Task { [weak self] in
            guard let image = await OnlineArtworkSearch.downloadThumbnail(candidate.thumbnailURL) else { return }
            await MainActor.run {
                guard let self else { return }
                self.thumbnails[candidate.thumbnailURL] = image
                var snapshot = self.candidatesDataSource.snapshot()
                guard snapshot.itemIdentifiers.contains(candidate) else { return }
                snapshot.reconfigureItems([candidate])
                self.candidatesDataSource.apply(snapshot, animatingDifferences: false)
            }
        }
    }

    private func setupDrop() {
        cropView.addInteraction(UIDropInteraction(delegate: self))
    }

    private func loadCurrentArtwork() {
        let apply: (UIImage?) -> Void = { [weak self] image in
            guard let self, !self.hasReplacedImage, !self.didApplyCurrentArtwork, let image else { return }
            self.didApplyCurrentArtwork = true
            self.cropView.setImage(image, userInitiated: false)
            self.setBackdrop(image)
            self.infoLabel.text = self.describe(image, source: String(localized: "Current artwork"))
        }
        if let cached = AlbumArtworkCache.shared.artwork(forAlbum: album.title, artist: album.artist) {
            apply(cached)
        } else {
            AlbumArtworkCache.shared.loadArtwork(forAlbum: album.title, artist: album.artist, completion: apply)
        }
    }

    private func startOnlineSearch() {
        onlineSpinner.startAnimating()
        searchTask = Task { [weak self, album] in
            let results = await OnlineArtworkSearch.search(title: album.title, artist: album.artist)
            await MainActor.run {
                guard let self, !Task.isCancelled else { return }
                self.onlineSpinner.stopAnimating()
                if results.isEmpty {
                    self.candidatesView.isHidden = true
                    self.onlineStatus.text = String(localized: "No matching covers found online. Try Photos or Files instead.")
                    self.onlineStatus.isHidden = false
                    return
                }
                self.onlineStatus.isHidden = true
                self.candidatesView.isHidden = false
                var snapshot = NSDiffableDataSourceSnapshot<Int, ArtworkCandidate>()
                snapshot.appendSections([0])
                snapshot.appendItems(results)
                self.candidatesDataSource.apply(snapshot, animatingDifferences: true)
            }
        }
    }

    private func describe(_ image: UIImage, source: String) -> String {
        let width = Int(image.size.width * image.scale)
        let height = Int(image.size.height * image.scale)
        return "\(source) · \(width) × \(height)"
    }

    private func setBackdrop(_ image: UIImage) {
        UIView.transition(with: backdropImageView, duration: 0.35, options: .transitionCrossDissolve) {
            self.backdropImageView.image = image
            self.backdropImageView.alpha = 1
        }
    }

    private func adopt(_ image: UIImage, source: String) {
        hasReplacedImage = true
        cropView.setImage(image, userInitiated: true)
        setBackdrop(image)
        infoLabel.text = describe(image, source: source)
        refreshSaveState()
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        scrollView.setContentOffset(CGPoint(x: 0, y: -scrollView.adjustedContentInset.top), animated: true)
    }

    private func refreshSaveState() {
        saveButton.isEnabled = hasUnsavedChanges && cropView.hasImage
    }

    @objc private func pasteboardChanged() {
        updatePasteAvailability()
    }

    private func updatePasteAvailability() {
        let available = UIPasteboard.general.hasImages
        sourceRow.arrangedSubviews.last?.alpha = available ? 1 : 0.45
        sourceRow.arrangedSubviews.last?.isUserInteractionEnabled = available
    }

    private func presentPhotoPicker() {
        var configuration = PHPickerConfiguration()
        configuration.filter = .images
        configuration.selectionLimit = 1
        configuration.preferredAssetRepresentationMode = .current
        let picker = PHPickerViewController(configuration: configuration)
        picker.delegate = self
        present(picker, animated: true)
    }

    private func presentFilePicker() {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.image], asCopy: true)
        picker.delegate = self
        present(picker, animated: true)
    }

    private func pasteFromClipboard() {
        guard let image = UIPasteboard.general.image else {
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
            return
        }
        adopt(Self.downsampled(image), source: String(localized: "Clipboard"))
    }

    private func load(data: Data, source: String) {
        guard let image = AlbumArtworkCache.decode(data, maxPixelSize: 3200) else {
            UINotificationFeedbackGenerator().notificationOccurred(.error)
            infoLabel.text = String(localized: "That file couldn't be read as an image.")
            return
        }
        adopt(image, source: source)
    }

    private static func downsampled(_ image: UIImage) -> UIImage {
        guard let data = image.pngData(), let reduced = AlbumArtworkCache.decode(data, maxPixelSize: 3200) else {
            return image
        }
        return reduced
    }

    private func select(_ candidate: ArtworkCandidate) {
        selectedCandidate = candidate
        var snapshot = candidatesDataSource.snapshot()
        snapshot.reconfigureItems(snapshot.itemIdentifiers)
        candidatesDataSource.apply(snapshot, animatingDifferences: false)
        UISelectionFeedbackGenerator().selectionChanged()

        loadTask?.cancel()
        cropView.setLoading(true)
        infoLabel.text = String(localized: "Loading \(candidate.source) cover…")
        loadTask = Task { [weak self] in
            do {
                let image = try await OnlineArtworkSearch.download(candidate.fullURL)
                await MainActor.run {
                    guard let self, !Task.isCancelled else { return }
                    self.cropView.setLoading(false)
                    self.adopt(image, source: candidate.source)
                }
            } catch {
                await MainActor.run {
                    guard let self, !Task.isCancelled else { return }
                    self.cropView.setLoading(false)
                    self.selectedCandidate = nil
                    self.infoLabel.text = String(localized: "Couldn't download that cover. Check your connection and try another.")
                    UINotificationFeedbackGenerator().notificationOccurred(.error)
                    var snapshot = self.candidatesDataSource.snapshot()
                    snapshot.reconfigureItems(snapshot.itemIdentifiers)
                    self.candidatesDataSource.apply(snapshot, animatingDifferences: false)
                }
            }
        }
    }

    @objc private func save() {
        guard let image = cropView.croppedImage(side: AlbumArtworkEditor.outputPixelSize),
              let data = AlbumArtworkEditor.encode(image)
        else {
            UINotificationFeedbackGenerator().notificationOccurred(.error)
            return
        }
        saveButton.isEnabled = false
        let album = album
        Task { [weak self] in
            do {
                try await Task.detached { try AlbumArtworkEditor.save(data, for: album) }.value
                await MainActor.run {
                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                    self?.finish()
                }
            } catch {
                AppLogger.error("Saving cover failed: \(error.localizedDescription)", category: .database)
                await MainActor.run {
                    UINotificationFeedbackGenerator().notificationOccurred(.error)
                    self?.saveButton.isEnabled = true
                    self?.infoLabel.text = String(localized: "Couldn't save the artwork. Try again.")
                }
            }
        }
    }

    private func finish() {
        let callback = onSaved
        dismiss(animated: true) { callback?() }
    }

    private func confirmReset() {
        let sheet = UIAlertController(
            title: String(localized: "Use the original artwork?"),
            message: String(localized: "Your chosen cover will be removed and the artwork from the music files is used again."),
            preferredStyle: .actionSheet
        )
        sheet.addAction(UIAlertAction(title: String(localized: "Use Original"), style: .destructive) { [weak self] _ in
            self?.performReset()
        })
        sheet.addAction(UIAlertAction(title: String(localized: "Cancel"), style: .cancel))
        sheet.popoverPresentationController?.sourceView = resetButton
        sheet.popoverPresentationController?.sourceRect = resetButton.bounds
        present(sheet, animated: true)
    }

    private func performReset() {
        let album = album
        Task { [weak self] in
            do {
                try await Task.detached { try AlbumArtworkEditor.reset(album) }.value
                await MainActor.run {
                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                    self?.finish()
                }
            } catch {
                AppLogger.error("Resetting cover failed: \(error.localizedDescription)", category: .database)
                await MainActor.run { UINotificationFeedbackGenerator().notificationOccurred(.error) }
            }
        }
    }
}

extension ArtworkEditorViewController: UICollectionViewDelegate {

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        guard let candidate = candidatesDataSource.itemIdentifier(for: indexPath) else { return }
        select(candidate)
    }
}

extension ArtworkEditorViewController: UIAdaptivePresentationControllerDelegate {

    func presentationControllerShouldDismiss(_ presentationController: UIPresentationController) -> Bool {
        !hasUnsavedChanges
    }

    func presentationControllerDidAttemptToDismiss(_ presentationController: UIPresentationController) {
        let sheet = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
        sheet.addAction(UIAlertAction(title: String(localized: "Discard Changes"), style: .destructive) { [weak self] _ in
            self?.dismiss(animated: true)
        })
        sheet.addAction(UIAlertAction(title: String(localized: "Keep Editing"), style: .cancel))
        sheet.popoverPresentationController?.barButtonItem = navigationItem.leftBarButtonItem
        present(sheet, animated: true)
    }
}

extension ArtworkEditorViewController: PHPickerViewControllerDelegate {

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        guard let provider = results.first?.itemProvider else { return }
        cropView.setLoading(true)
        provider.loadDataRepresentation(forTypeIdentifier: UTType.image.identifier) { [weak self] data, _ in
            DispatchQueue.main.async {
                guard let self else { return }
                self.cropView.setLoading(false)
                guard let data else {
                    UINotificationFeedbackGenerator().notificationOccurred(.error)
                    return
                }
                self.load(data: data, source: String(localized: "Photo"))
            }
        }
    }
}

extension ArtworkEditorViewController: UIDocumentPickerDelegate {

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first, let data = try? Data(contentsOf: url) else {
            UINotificationFeedbackGenerator().notificationOccurred(.error)
            return
        }
        load(data: data, source: url.deletingPathExtension().lastPathComponent)
    }
}

extension ArtworkEditorViewController: UIDropInteractionDelegate {

    func dropInteraction(_ interaction: UIDropInteraction, canHandle session: UIDropSession) -> Bool {
        session.canLoadObjects(ofClass: UIImage.self)
    }

    func dropInteraction(_ interaction: UIDropInteraction, sessionDidUpdate session: UIDropSession) -> UIDropProposal {
        UIDropProposal(operation: .copy)
    }

    func dropInteraction(_ interaction: UIDropInteraction, performDrop session: UIDropSession) {
        session.loadObjects(ofClass: UIImage.self) { [weak self] items in
            guard let image = items.first as? UIImage else { return }
            DispatchQueue.main.async {
                self?.adopt(Self.downsampled(image), source: String(localized: "Dropped image"))
            }
        }
    }
}

private final class ArtworkCandidateCell: UICollectionViewCell {

    private let imageView = UIImageView()
    private let sourceLabel = UILabel()
    private let ring = UIView()

    override init(frame: CGRect) {
        super.init(frame: frame)

        imageView.contentMode = .scaleAspectFill
        imageView.clipsToBounds = true
        imageView.layer.cornerRadius = 12
        imageView.layer.cornerCurve = .continuous
        imageView.backgroundColor = UIColor.white.withAlphaComponent(0.1)
        imageView.translatesAutoresizingMaskIntoConstraints = false

        ring.isUserInteractionEnabled = false
        ring.layer.cornerRadius = 12
        ring.layer.cornerCurve = .continuous
        ring.layer.borderWidth = 3
        ring.layer.borderColor = UIColor.white.cgColor
        ring.alpha = 0
        ring.translatesAutoresizingMaskIntoConstraints = false

        sourceLabel.font = .scaled(.caption1, size: 11, weight: .medium)
        sourceLabel.textColor = UIColor.white.withAlphaComponent(0.6)
        sourceLabel.numberOfLines = 1
        sourceLabel.textAlignment = .center
        sourceLabel.translatesAutoresizingMaskIntoConstraints = false

        contentView.addSubview(imageView)
        contentView.addSubview(ring)
        contentView.addSubview(sourceLabel)
        NSLayoutConstraint.activate([
            imageView.topAnchor.constraint(equalTo: contentView.topAnchor),
            imageView.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            imageView.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            imageView.heightAnchor.constraint(equalTo: imageView.widthAnchor),
            ring.topAnchor.constraint(equalTo: imageView.topAnchor),
            ring.leadingAnchor.constraint(equalTo: imageView.leadingAnchor),
            ring.trailingAnchor.constraint(equalTo: imageView.trailingAnchor),
            ring.bottomAnchor.constraint(equalTo: imageView.bottomAnchor),
            sourceLabel.topAnchor.constraint(equalTo: imageView.bottomAnchor, constant: 6),
            sourceLabel.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            sourceLabel.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
        ])
        isAccessibilityElement = true
        accessibilityTraits = .button
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(candidate: ArtworkCandidate, image: UIImage?, isSelected: Bool) {
        imageView.image = image
        sourceLabel.text = candidate.source
        UIView.animate(withDuration: 0.18) { self.ring.alpha = isSelected ? 1 : 0 }
        accessibilityLabel = "\(candidate.title), \(candidate.artist), \(candidate.source)"
        accessibilityTraits = isSelected ? [.button, .selected] : .button
    }
}
