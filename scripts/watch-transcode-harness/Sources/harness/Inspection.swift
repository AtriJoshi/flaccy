import AVFoundation
import AudioToolbox
import FlaccyCore
import Foundation
import ImageIO

enum Reporter {

    static var failures = 0
    static var passes = 0

    static func check(_ condition: Bool, _ name: String, _ detail: String) {
        if condition {
            passes += 1
            print("PASS \(name) \(detail)")
        } else {
            failures += 1
            print("FAIL \(name) \(detail)")
        }
    }

    static func note(_ name: String, _ detail: String) {
        print("INFO \(name) \(detail)")
    }

    static func skip(_ name: String, _ detail: String) {
        print("SKIP \(name) \(detail)")
    }
}

struct OutputFacts {
    var allTracks = 0
    var audioTracks = 0
    var formatID: AudioFormatID = 0
    var rate = 0
    var channels = 0
    var duration = 0.0
    var estimatedRate = 0.0
    var bytes: Int64 = 0
    var packetInfo = AudioFilePacketTableInfo()
    var packetCount: UInt64 = 0
    var hasPacketInfo = false
}

enum Inspector {

    static func facts(of url: URL) async throws -> OutputFacts {
        var facts = OutputFacts()
        let asset = AVURLAsset(url: url)
        facts.allTracks = try await asset.load(.tracks).count
        let audio = try await asset.loadTracks(withMediaType: .audio)
        facts.audioTracks = audio.count
        facts.duration = CMTimeGetSeconds(try await asset.load(.duration))
        if let track = audio.first {
            facts.estimatedRate = Double(try await track.load(.estimatedDataRate))
            if let description = try await track.load(.formatDescriptions).first,
               let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description)?.pointee {
                facts.formatID = asbd.mFormatID
                facts.rate = Int(asbd.mSampleRate)
                facts.channels = Int(asbd.mChannelsPerFrame)
            }
        }
        facts.bytes = ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber)?.int64Value ?? 0
        readPacketTable(of: url, into: &facts)
        return facts
    }

    private static func readPacketTable(of url: URL, into facts: inout OutputFacts) {
        var file: AudioFileID?
        guard AudioFileOpenURL(url as CFURL, .readPermission, 0, &file) == noErr, let file else { return }
        defer { AudioFileClose(file) }
        var info = AudioFilePacketTableInfo()
        var size = UInt32(MemoryLayout<AudioFilePacketTableInfo>.size)
        if AudioFileGetProperty(file, kAudioFilePropertyPacketTableInfo, &size, &info) == noErr {
            facts.packetInfo = info
            facts.hasPacketInfo = true
        }
        var count: UInt64 = 0
        var countSize = UInt32(MemoryLayout<UInt64>.size)
        if AudioFileGetProperty(file, kAudioFilePropertyAudioDataPacketCount, &countSize, &count) == noErr {
            facts.packetCount = count
        }
    }

    struct Decoded {
        var frames = 0
        var firstChannel: [Float] = []
    }

    static func decode(_ url: URL, keepSamples: Bool) async throws -> Decoded {
        let asset = AVURLAsset(url: url)
        guard let track = try await asset.loadTracks(withMediaType: .audio).first else { throw CocoaError(.fileReadCorruptFile) }
        let reader = try AVAssetReader(asset: asset)
        let output = AVAssetReaderTrackOutput(track: track, outputSettings: [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVLinearPCMBitDepthKey: 32,
            AVLinearPCMIsFloatKey: true,
            AVLinearPCMIsBigEndianKey: false,
            AVLinearPCMIsNonInterleaved: false,
        ])
        reader.add(output)
        reader.startReading()
        var decoded = Decoded()
        var channels = 1
        if let description = try await track.load(.formatDescriptions).first,
           let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(description)?.pointee {
            channels = Int(asbd.mChannelsPerFrame)
        }
        while let buffer = output.copyNextSampleBuffer() {
            let frames = CMSampleBufferGetNumSamples(buffer)
            decoded.frames += frames
            guard keepSamples, let block = CMSampleBufferGetDataBuffer(buffer) else { continue }
            var length = 0
            var pointer: UnsafeMutablePointer<Int8>?
            CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length, dataPointerOut: &pointer)
            guard let pointer else { continue }
            pointer.withMemoryRebound(to: Float.self, capacity: length / 4) { floats in
                for frame in 0..<frames { decoded.firstChannel.append(floats[frame * channels]) }
            }
        }
        if reader.status == .failed { throw reader.error ?? CocoaError(.fileReadUnknown) }
        return decoded
    }

    static func tags(of url: URL) async -> [String: String] {
        let asset = AVURLAsset(url: url)
        var items = (try? await asset.load(.metadata)) ?? []
        for format in (try? await asset.load(.availableMetadataFormats)) ?? [] {
            items += (try? await asset.loadMetadata(for: format)) ?? []
        }
        var result: [String: String] = [:]
        let map: [(AVMetadataIdentifier, String)] = [
            (.iTunesMetadataSongName, "TITLE"), (.iTunesMetadataArtist, "ARTIST"), (.iTunesMetadataAlbum, "ALBUM"),
            (.iTunesMetadataAlbumArtist, "ALBUMARTIST"), (.iTunesMetadataUserGenre, "GENRE"),
            (.iTunesMetadataReleaseDate, "DATE"), (.iTunesMetadataComposer, "COMPOSER"),
        ]
        for (identifier, key) in map {
            let match = AVMetadataItem.metadataItems(from: items, filteredByIdentifier: identifier).first
            if let value = try? await match?.load(.stringValue) { result[key] = value }
        }
        for (identifier, key) in [(AVMetadataIdentifier.iTunesMetadataTrackNumber, "TRACKNUMBER"), (.iTunesMetadataDiscNumber, "DISCNUMBER")] {
            guard let item = AVMetadataItem.metadataItems(from: items, filteredByIdentifier: identifier).first else { continue }
            if item.dataType == (kCMMetadataBaseDataType_RawData as String), let data = try? await item.load(.dataValue), data.count >= 4 {
                result[key] = String(Int(data[2]) << 8 | Int(data[3]))
            } else if let number = try? await item.load(.numberValue) {
                result[key] = String(number.intValue)
            }
        }
        return result
    }

    static func imageSize(_ data: Data) -> (width: Int, height: Int)? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int,
              let height = properties[kCGImagePropertyPixelHeight] as? Int
        else { return nil }
        return (width, height)
    }
}
