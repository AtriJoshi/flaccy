import AVFoundation
import Foundation

extension FixtureBuilder {

    static let afconvert = "/usr/bin/afconvert"

    static func syntheticTagMap() -> [String: String] {
        var map: [String: String] = [:]
        for (key, value) in syntheticTags where tagKeys.contains(key) { map[key] = value }
        return map
    }

    static func makeFLAC(
        label: String,
        in directory: URL,
        rate: Int,
        channels: Int,
        bits: Int,
        seconds: Double,
        tagged: Bool,
        cover: Data?,
        clickAt: Int? = nil,
        level: Float = 1
    ) throws -> Fixture {
        let wav = directory.appendingPathComponent("\(label).wav")
        let plain = directory.appendingPathComponent("\(label)-plain.flac")
        let final = directory.appendingPathComponent("\(label).flac")
        try writeWAV(to: wav, rate: rate, channels: channels, bits: bits, seconds: seconds, clickAt: clickAt, level: level)
        try run(afconvert, ["-f", "flac", "-d", "flac", wav.path, plain.path])
        if tagged {
            try FlacTagging.write(source: plain, to: final, comments: syntheticTags, picture: cover, mime: "image/png")
        } else {
            try FileManager.default.moveItem(at: plain, to: final)
        }
        try? FileManager.default.removeItem(at: wav)
        try? FileManager.default.removeItem(at: plain)
        let file = try AVAudioFile(forReading: final)
        return Fixture(
            label: label,
            url: final,
            kind: .lossless,
            sourceRate: Int(file.fileFormat.sampleRate),
            sourceChannels: Int(file.fileFormat.channelCount),
            sourceFrames: file.length,
            tags: tagged ? syntheticTagMap() : [:],
            hasCover: tagged && cover != nil,
            isReal: false
        )
    }

    static func makeWAV(label: String, in directory: URL, rate: Int, channels: Int, seconds: Double) throws -> Fixture {
        let source = directory.appendingPathComponent("\(label).wav")
        let temporary = directory.appendingPathComponent("\(label)-raw.wav")
        try writeWAV(to: temporary, rate: rate, channels: channels, bits: 16, seconds: seconds)
        try run(afconvert, ["-f", "WAVE", "-d", "LEI16", temporary.path, source.path])
        try? FileManager.default.removeItem(at: temporary)
        let file = try AVAudioFile(forReading: source)
        return Fixture(
            label: label,
            url: source,
            kind: .lossless,
            sourceRate: Int(file.fileFormat.sampleRate),
            sourceChannels: Int(file.fileFormat.channelCount),
            sourceFrames: file.length,
            tags: [:],
            hasCover: false,
            isReal: false
        )
    }

    static func makeM4A(
        label: String,
        in directory: URL,
        codec: String,
        rate: Int,
        seconds: Double,
        tagged: Bool,
        cover: Data?
    ) async throws -> Fixture {
        let wav = directory.appendingPathComponent("\(label).wav")
        let plain = directory.appendingPathComponent("\(label)-plain.m4a")
        let final = directory.appendingPathComponent("\(label).m4a")
        try writeWAV(to: wav, rate: rate, channels: 2, bits: 16, seconds: seconds)
        var arguments = ["-f", "m4af", "-d", codec]
        if codec == "aac" { arguments += ["-b", "192000"] }
        try run(afconvert, arguments + [wav.path, plain.path])
        try? FileManager.default.removeItem(at: wav)
        if tagged {
            try await exportTagged(plain, to: final, cover: cover)
            try? FileManager.default.removeItem(at: plain)
        } else {
            try FileManager.default.moveItem(at: plain, to: final)
        }
        let file = try AVAudioFile(forReading: final)
        return Fixture(
            label: label,
            url: final,
            kind: codec == "alac" ? .lossless : .lossy,
            sourceRate: Int(file.fileFormat.sampleRate),
            sourceChannels: Int(file.fileFormat.channelCount),
            sourceFrames: file.length,
            tags: tagged ? syntheticTagMap() : [:],
            hasCover: tagged && cover != nil,
            isReal: false
        )
    }

    private static func exportTagged(_ source: URL, to destination: URL, cover: Data?) async throws {
        let asset = AVURLAsset(url: source)
        guard let session = AVAssetExportSession(asset: asset, presetName: AVAssetExportPresetPassthrough) else {
            throw CocoaError(.fileWriteUnknown)
        }
        session.metadata = tagItems(cover: cover)
        try await session.export(to: destination, as: .m4a)
    }

    private static func tagItems(cover: Data?) -> [AVMetadataItem] {
        func text(_ identifier: AVMetadataIdentifier, _ value: String) -> AVMetadataItem {
            let item = AVMutableMetadataItem()
            item.identifier = identifier
            item.value = value as NSString
            item.dataType = kCMMetadataBaseDataType_UTF8 as String
            item.extendedLanguageTag = "und"
            return item
        }
        let map = syntheticTagMap()
        var items = [
            text(.iTunesMetadataSongName, map["TITLE"]!),
            text(.iTunesMetadataArtist, map["ARTIST"]!),
            text(.iTunesMetadataAlbum, map["ALBUM"]!),
            text(.iTunesMetadataAlbumArtist, map["ALBUMARTIST"]!),
            text(.iTunesMetadataUserGenre, map["GENRE"]!),
            text(.iTunesMetadataReleaseDate, map["DATE"]!),
            text(.iTunesMetadataComposer, map["COMPOSER"]!),
        ]
        let track = AVMutableMetadataItem()
        track.identifier = .iTunesMetadataTrackNumber
        track.value = Data([0, 0, 0, 3, 0, 12, 0, 0]) as NSData
        track.dataType = kCMMetadataBaseDataType_RawData as String
        let disc = AVMutableMetadataItem()
        disc.identifier = .iTunesMetadataDiscNumber
        disc.value = Data([0, 0, 0, 2, 0, 2]) as NSData
        disc.dataType = kCMMetadataBaseDataType_RawData as String
        items += [track, disc]
        if let cover {
            let art = AVMutableMetadataItem()
            art.identifier = .iTunesMetadataCoverArt
            art.value = cover as NSData
            art.dataType = kCMMetadataBaseDataType_PNG as String
            items.append(art)
        }
        return items
    }

    static func makeMP3Stub(in directory: URL) throws -> URL {
        let url = directory.appendingPathComponent("stub.mp3")
        try Data(repeating: 0xFF, count: 4096).write(to: url)
        return url
    }

    static func tryEncodeMP3(from wav: URL, to destination: URL) -> Bool {
        (try? run(afconvert, ["-f", "MPG3", "-d", ".mp3", wav.path, destination.path])) != nil
    }
}
