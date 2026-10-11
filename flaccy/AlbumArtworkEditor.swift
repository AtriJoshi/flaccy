import UIKit

/// Saving and clearing a person-chosen cover. The cover is stored under every
/// key a surface can ask for the release by, because tracks look their art up
/// by their own album title and performing artist while album cards use the
/// grouped title and artist.
nonisolated enum AlbumArtworkEditor {

    static let outputPixelSize: CGFloat = 1600
    private static let jpegQuality: CGFloat = 0.9

    static func keys(for album: Album) -> [(title: String, artist: String)] {
        var seen = Set<String>()
        var keys: [(title: String, artist: String)] = []
        let candidates = [(album.title, album.artist)] + album.tracks.map { ($0.albumTitle, $0.artist) }
        for (title, artist) in candidates where !title.isEmpty {
            guard seen.insert("\(title)\0\(artist)").inserted else { continue }
            keys.append((title, artist))
        }
        return keys
    }

    static func hasCustomCover(_ album: Album) -> Bool {
        DatabaseManager.shared.hasCustomCover(forKeys: keys(for: album))
    }

    static func encode(_ image: UIImage) -> Data? {
        image.jpegData(compressionQuality: jpegQuality)
    }

    static func save(_ data: Data, for album: Album) throws {
        let keys = keys(for: album)
        try DatabaseManager.shared.saveCustomCover(data, forKeys: keys)
        refreshSurfaces(for: keys)
        AppLogger.info("Custom cover saved for \(album.title) (\(data.count / 1024) KB)", category: .content)
    }

    static func reset(_ album: Album) throws {
        let keys = keys(for: album)
        try DatabaseManager.shared.clearCustomCover(forKeys: keys)
        refreshSurfaces(for: keys)
        AppLogger.info("Custom cover cleared for \(album.title)", category: .content)
    }

    private static func refreshSurfaces(for keys: [(title: String, artist: String)]) {
        for key in keys {
            ArtworkPaletteExtractor.invalidate(title: key.title, artist: key.artist)
        }
        AlbumArtworkCache.shared.invalidate(keys: keys)
    }
}

nonisolated struct ArtworkCandidate: Sendable, Hashable, Identifiable {
    let id: String
    let source: String
    let title: String
    let artist: String
    let thumbnailURL: URL
    let fullURL: URL
    let score: Double
}

/// Finds higher-resolution covers for a release from the two catalogs that
/// serve square artwork without a key. Results are scored on how closely the
/// catalog's title and artist match the release so the right cover leads.
nonisolated enum OnlineArtworkSearch {

    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 10
        configuration.waitsForConnectivity = false
        return URLSession(configuration: configuration)
    }()

    static func search(title: String, artist: String) async -> [ArtworkCandidate] {
        let credit = artist.caseInsensitiveCompare("Various Artists") == .orderedSame ? "" : artist
        async let apple = appleCandidates(title: title, artist: credit)
        async let deezer = deezerCandidates(title: title, artist: credit)
        let merged = await apple + deezer
        var seen = Set<URL>()
        let unique = merged.filter { seen.insert($0.fullURL).inserted }
        let ranked = unique.sorted { $0.score > $1.score }
        let confident = ranked.filter { $0.score >= 0.5 }
        return Array((confident.count >= 3 ? confident : ranked).prefix(18))
    }

    static func download(_ url: URL) async throws -> UIImage {
        let (data, response) = try await session.data(from: url)
        guard (response as? HTTPURLResponse)?.statusCode == 200,
              let image = AlbumArtworkCache.decode(data, maxPixelSize: 3200)
        else { throw URLError(.cannotDecodeContentData) }
        return image
    }

    static func downloadThumbnail(_ url: URL) async -> UIImage? {
        guard let (data, _) = try? await session.data(from: url) else { return nil }
        return AlbumArtworkCache.decode(data, maxPixelSize: 360)
    }

    private static func appleCandidates(title: String, artist: String) async -> [ArtworkCandidate] {
        var components = URLComponents(string: "https://itunes.apple.com/search")
        components?.queryItems = [
            URLQueryItem(name: "term", value: "\(artist) \(title)".trimmingCharacters(in: .whitespaces)),
            URLQueryItem(name: "media", value: "music"),
            URLQueryItem(name: "entity", value: "album"),
            URLQueryItem(name: "limit", value: "20"),
        ]
        guard let url = components?.url,
              let (data, _) = try? await session.data(from: url),
              let response = try? JSONDecoder().decode(AppleResponse.self, from: data)
        else { return [] }
        return response.results.compactMap { item in
            guard let small = item.artworkUrl100,
                  let thumbnail = URL(string: small.replacingOccurrences(of: "100x100bb", with: "360x360bb")),
                  let full = URL(string: small.replacingOccurrences(of: "100x100bb", with: "1400x1400bb"))
            else { return nil }
            return ArtworkCandidate(
                id: "apple-\(item.collectionId ?? 0)",
                source: "Apple Music",
                title: item.collectionName ?? "",
                artist: item.artistName ?? "",
                thumbnailURL: thumbnail,
                fullURL: full,
                score: score(candidateTitle: item.collectionName ?? "", candidateArtist: item.artistName ?? "", title: title, artist: artist)
            )
        }
    }

    private static func deezerCandidates(title: String, artist: String) async -> [ArtworkCandidate] {
        var components = URLComponents(string: "https://api.deezer.com/search/album")
        let query = artist.isEmpty ? title : "artist:\"\(artist)\" album:\"\(title)\""
        components?.queryItems = [
            URLQueryItem(name: "q", value: query),
            URLQueryItem(name: "limit", value: "12"),
        ]
        guard let url = components?.url,
              let (data, _) = try? await session.data(from: url),
              let response = try? JSONDecoder().decode(DeezerResponse.self, from: data)
        else { return [] }
        return response.data.compactMap { item in
            guard let thumbnail = item.cover_medium.flatMap(URL.init(string:)),
                  let full = (item.cover_xl ?? item.cover_big).flatMap(URL.init(string:))
            else { return nil }
            return ArtworkCandidate(
                id: "deezer-\(item.id)",
                source: "Deezer",
                title: item.title,
                artist: item.artist?.name ?? "",
                thumbnailURL: thumbnail,
                fullURL: full,
                score: score(candidateTitle: item.title, candidateArtist: item.artist?.name ?? "", title: title, artist: artist)
            )
        }
    }

    /// Title carries most of the weight; an exact edition-folded title match is
    /// worth far more than a substring, and the artist breaks ties between
    /// different records that share a name.
    static func score(candidateTitle: String, candidateArtist: String, title: String, artist: String) -> Double {
        let wanted = LibraryHygiene.baseTitle(title)
        let found = LibraryHygiene.baseTitle(candidateTitle)
        var titleScore = 0.0
        if wanted == found {
            titleScore = 0.7
        } else if !wanted.isEmpty, !found.isEmpty, found.contains(wanted) || wanted.contains(found) {
            titleScore = 0.4
        }
        let artistScore: Double
        if artist.isEmpty {
            artistScore = 0.15
        } else {
            artistScore = LibraryHygiene.artistKey(candidateArtist) == LibraryHygiene.artistKey(artist) ? 0.3 : 0
        }
        return titleScore + artistScore
    }

    private struct AppleResponse: Decodable {
        struct Item: Decodable {
            let collectionId: Int?
            let collectionName: String?
            let artistName: String?
            let artworkUrl100: String?
        }
        let results: [Item]
    }

    private struct DeezerResponse: Decodable {
        struct Artist: Decodable { let name: String }
        struct Item: Decodable {
            let id: Int
            let title: String
            let artist: Artist?
            let cover_medium: String?
            let cover_big: String?
            let cover_xl: String?
        }
        let data: [Item]
    }
}
