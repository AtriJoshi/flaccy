import AVFoundation
import FlaccyCore
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

struct Fixture {

    enum Kind {
        case lossless
        case lossy
    }

    let label: String
    let url: URL
    let kind: Kind
    let sourceRate: Int
    let sourceChannels: Int
    let sourceFrames: Int64
    let tags: [String: String]
    let hasCover: Bool
    let isReal: Bool
    var fallsBackAfterEncode = false

    var sourceBytes: Int64 {
        ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber)?.int64Value ?? 0
    }

    var sourceKbps: Double { Double(sourceBytes) * 8 / seconds / 1000 }

    func targetKbps(_ profile: WatchAudioProfile) -> Double {
        Double(profile.bitRate ?? 0) / 1000 / (expectedOutputChannels == 1 ? 2 : 1)
    }

    func isLean(for profile: WatchAudioProfile) -> Bool {
        kind == .lossless && sourceKbps <= 1.15 * targetKbps(profile)
    }

    var seconds: Double { Double(sourceFrames) / Double(sourceRate) }

    var expectedOutputRate: Int {
        switch sourceRate {
        case 44_100, 48_000: sourceRate
        case 88_200, 176_400, 352_800: 44_100
        default: 48_000
        }
    }

    var expectedOutputChannels: Int { min(sourceChannels, 2) }
}

nonisolated enum FixtureBuilder {

    static let syntheticTags: [(String, String)] = [
        ("TITLE", "Fixture Title \u{00FC}\u{00EF}"),
        ("ARTIST", "Fixture Artist"),
        ("ALBUM", "Fixture Album"),
        ("ALBUMARTIST", "Fixture Album Artist"),
        ("TRACKNUMBER", "3"),
        ("TRACKTOTAL", "12"),
        ("DISCNUMBER", "2"),
        ("DISCTOTAL", "2"),
        ("DATE", "2011"),
        ("GENRE", "Fixture Rock"),
        ("COMPOSER", "Fixture Composer"),
    ]

    static func realLibraryFiles() -> [URL] {
        let root = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/flaccy/Music")
        let enumerator = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil)
        let files = (enumerator?.allObjects as? [URL] ?? []).filter { $0.pathExtension.lowercased() == "flac" }
        return files.sorted { $0.path < $1.path }
    }

    static func twoConsecutiveRealFiles() -> [URL] {
        let files = realLibraryFiles()
        guard let first = files.first else { return [] }
        let album = files.filter { $0.deletingLastPathComponent() == first.deletingLastPathComponent() }
        return Array(album.prefix(2))
    }

    static func describeReal(_ url: URL, label: String) -> Fixture? {
        guard let file = try? AVAudioFile(forReading: url) else { return nil }
        let rate = Int(file.fileFormat.sampleRate)
        let comments = FlacTagging.readComments(url)
        var tags: [String: String] = [:]
        for (key, value) in comments where tagKeys.contains(key) { tags[key] = value }
        return Fixture(
            label: label,
            url: url,
            kind: .lossless,
            sourceRate: rate,
            sourceChannels: Int(file.fileFormat.channelCount),
            sourceFrames: file.length,
            tags: tags,
            hasCover: false,
            isReal: true
        )
    }

    static let tagKeys: Set<String> = ["TITLE", "ARTIST", "ALBUM", "ALBUMARTIST", "TRACKNUMBER", "DISCNUMBER", "DATE", "GENRE", "COMPOSER"]

    static func run(_ tool: String, _ arguments: [String]) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        let sink = Pipe()
        process.standardOutput = sink
        process.standardError = sink
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(decoding: sink.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            throw NSError(domain: "fixture", code: Int(process.terminationStatus), userInfo: [NSLocalizedDescriptionKey: "\(tool) \(arguments.joined(separator: " ")): \(message)"])
        }
    }

    static func writeWAV(to url: URL, rate: Int, channels: Int, bits: Int, seconds: Double, clickAt: Int? = nil, level: Float = 1) throws {
        var settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: rate,
            AVNumberOfChannelsKey: channels,
            AVLinearPCMBitDepthKey: bits,
            AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false,
            AVLinearPCMIsNonInterleaved: false,
        ]
        if channels == 6 {
            var layout = AudioChannelLayout()
            layout.mChannelLayoutTag = kAudioChannelLayoutTag_MPEG_5_1_A
            settings[AVChannelLayoutKey] = withUnsafeBytes(of: &layout) { Data($0) }
        }
        let file = try AVAudioFile(forWriting: url, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        let total = Int(seconds * Double(rate))
        let block = 16_384
        var generator = SignalGenerator(seed: UInt64(rate &+ channels))
        var written = 0
        while written < total {
            let count = min(block, total - written)
            guard let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(count)) else {
                throw CocoaError(.fileWriteUnknown)
            }
            buffer.frameLength = AVAudioFrameCount(count)
            for channel in 0..<channels {
                let pointer = buffer.floatChannelData![channel]
                for index in 0..<count {
                    let position = written + index
                    if let clickAt {
                        pointer[index] = position == clickAt ? 0.9 : 0
                    } else {
                        pointer[index] = level * generator.sample(at: position, rate: rate, channel: channel)
                    }
                }
            }
            try file.write(from: buffer)
            written += count
        }
    }

    static func coverPNG() -> Data {
        busyPNG(size: 1000, circles: 500)
    }

    static func tiledCoverPNG() -> Data {
        let size = 400
        let tile = 4
        let space = CGColorSpaceCreateDeviceRGB()
        let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        var generator = SignalGenerator(seed: 7)
        var colors: [CGColor] = []
        for _ in 0..<(tile * tile) {
            colors.append(CGColor(red: CGFloat(abs(generator.nextNoise())), green: CGFloat(abs(generator.nextNoise())), blue: CGFloat(abs(generator.nextNoise())), alpha: 1))
        }
        for y in 0..<size {
            for x in 0..<size {
                context.setFillColor(colors[(y % tile) * tile + (x % tile)])
                context.fill(CGRect(x: x, y: y, width: 1, height: 1))
            }
        }
        let buffer = NSMutableData()
        let destination = CGImageDestinationCreateWithData(buffer, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, context.makeImage()!, nil)
        CGImageDestinationFinalize(destination)
        return buffer as Data
    }

    static func busyPNG(size: Int, circles: Int) -> Data {
        let space = CGColorSpaceCreateDeviceRGB()
        let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        let gradient = CGGradient(colorsSpace: space, colors: [CGColor(red: 0.1, green: 0.2, blue: 0.6, alpha: 1), CGColor(red: 0.9, green: 0.4, blue: 0.2, alpha: 1)] as CFArray, locations: [0, 1])!
        context.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: size, y: size), options: [])
        var generator = SignalGenerator(seed: 99)
        for _ in 0..<circles {
            let x = CGFloat(abs(generator.nextNoise()) * Float(size))
            let y = CGFloat(abs(generator.nextNoise()) * Float(size))
            let radius = CGFloat(10 + abs(generator.nextNoise()) * 60)
            context.setFillColor(CGColor(red: CGFloat(abs(generator.nextNoise())), green: CGFloat(abs(generator.nextNoise())), blue: CGFloat(abs(generator.nextNoise())), alpha: 0.6))
            context.fillEllipse(in: CGRect(x: x - radius, y: y - radius, width: radius * 2, height: radius * 2))
        }
        let image = context.makeImage()!
        let buffer = NSMutableData()
        let destination = CGImageDestinationCreateWithData(buffer, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, image, nil)
        CGImageDestinationFinalize(destination)
        return buffer as Data
    }
}

nonisolated struct SignalGenerator {

    private var state: UInt64

    init(seed: UInt64) {
        state = seed &* 6364136223846793005 &+ 1442695040888963407
    }

    mutating func nextNoise() -> Float {
        state ^= state << 13
        state ^= state >> 7
        state ^= state << 17
        return Float(Int64(bitPattern: state) >> 11) / Float(1 << 52)
    }

    mutating func sample(at position: Int, rate: Int, channel: Int) -> Float {
        let time = Double(position) / Double(rate)
        var value: Float = 0
        for partial in 1...6 {
            let frequency = 110.0 * Double(partial) * (1 + 0.003 * Double(channel))
            let envelope = 0.5 + 0.5 * sin(2 * Double.pi * (0.3 * Double(partial) * time + Double(channel) * 0.2))
            value += Float(sin(2 * Double.pi * frequency * time) * envelope) / Float(partial)
        }
        let burst: Float = (position / (rate / 4)) % 2 == 0 ? 0.12 : 0.04
        return 0.25 * value + burst * nextNoise()
    }
}
