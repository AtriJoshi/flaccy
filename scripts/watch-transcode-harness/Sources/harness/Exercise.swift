import AVFoundation
import FlaccyCore
import Foundation

struct Row {
    let label: String
    let profile: String
    let ratio: Double
    let kbps: Double
    let speed: Double
    let strategy: String
}

struct EncodedRecord {
    let prepared: WatchTranscoder.Prepared
    let facts: OutputFacts
}

@MainActor
final class Exercise {

    let transcoder: WatchTranscoder
    let cacheDirectory: URL
    var rows: [Row] = []
    var records: [String: EncodedRecord] = [:]

    init(cacheDirectory: URL) {
        self.cacheDirectory = cacheDirectory
        transcoder = WatchTranscoder(cacheDirectory: cacheDirectory)
    }

    static let profiles: [WatchAudioProfile] = [.high, .balanced, .compact]

    func expectedKbps(_ profile: WatchAudioProfile, channels: Int) -> Double {
        Double(profile.bitRate ?? 0) / (channels == 1 ? 2 : 1)
    }

    func encodeAll(_ fixture: Fixture) async {
        for profile in Self.profiles { await encode(fixture, profile) }
    }

    func encode(_ fixture: Fixture, _ profile: WatchAudioProfile) async {
        let name = "\(fixture.label)/\(profile.rawValue)"
        let clock = ContinuousClock()
        let started = clock.now
        let prepared: WatchTranscoder.Prepared
        do {
            prepared = try await transcoder.prepare(source: fixture.url, profile: profile)
        } catch {
            Reporter.check(false, "\(name)/encode", "threw \(error)")
            return
        }
        let seconds = durationSeconds(clock.now - started)
        if fixture.isLean(for: profile) {
            let leftover = Scenarios.listing(cacheDirectory).filter { $0.hasSuffix(".partial") }
            Reporter.check(!prepared.wasEncoded && prepared.url == fixture.url && prepared.destinationExtension == fixture.url.pathExtension && leftover.isEmpty,
                           "\(name)/lean-passthrough", String(format: "source %.1fkbps <= 1.15 x target %.1fkbps", fixture.sourceKbps, fixture.targetKbps(profile)))
            return
        }
        Reporter.check(prepared.wasEncoded && prepared.destinationExtension == "m4a" && prepared.url != fixture.url,
                       "\(name)/encoded", String(format: "source %.1fkbps > 1.15 x target %.1fkbps; ", fixture.sourceKbps, fixture.targetKbps(profile)) + "ext=\(prepared.destinationExtension) bytes=\(prepared.bytes)")
        guard let facts = try? await Inspector.facts(of: prepared.url) else {
            Reporter.check(false, "\(name)/open", "AVURLAsset could not open output")
            return
        }
        records[name] = EncodedRecord(prepared: prepared, facts: facts)
        checkFormat(name, fixture, facts)
        checkDuration(name, fixture, facts)
        checkBitRate(name, fixture, profile, facts)
        checkSize(name, fixture, profile, facts, prepared, encodeSeconds: seconds)
        await checkTags(name, fixture, prepared.url)
        checkGapless(name, fixture, facts)
        await checkDecodeCount(name, facts, prepared.url)
        await checkCacheHit(name, fixture, profile, first: prepared)
    }

    func durationSeconds(_ duration: Duration) -> Double {
        Double(duration.components.seconds) + Double(duration.components.attoseconds) / 1e18
    }

    func checkFormat(_ name: String, _ fixture: Fixture, _ facts: OutputFacts) {
        let isAAC = facts.formatID == kAudioFormatMPEG4AAC
        Reporter.check(facts.audioTracks == 1 && facts.allTracks == 1 && isAAC, "\(name)/one-aac-track",
                       "tracks=\(facts.allTracks) audio=\(facts.audioTracks) format=\(fourCC(facts.formatID))")
        Reporter.check(facts.rate == fixture.expectedOutputRate && facts.channels == fixture.expectedOutputChannels,
                       "\(name)/rate-channels", "got \(facts.rate)Hz/\(facts.channels)ch want \(fixture.expectedOutputRate)Hz/\(fixture.expectedOutputChannels)ch (source \(fixture.sourceRate)Hz/\(fixture.sourceChannels)ch)")
    }

    func fourCC(_ value: UInt32) -> String {
        String(bytes: [UInt8((value >> 24) & 0xFF), UInt8((value >> 16) & 0xFF), UInt8((value >> 8) & 0xFF), UInt8(value & 0xFF)], encoding: .macOSRoman) ?? "\(value)"
    }

    func checkDuration(_ name: String, _ fixture: Fixture, _ facts: OutputFacts) {
        let delta = abs(facts.duration - fixture.seconds)
        Reporter.check(delta <= 0.030, "\(name)/duration", String(format: "out=%.4fs src=%.4fs delta=%.1fms", facts.duration, fixture.seconds, delta * 1000))
    }

    func checkBitRate(_ name: String, _ fixture: Fixture, _ profile: WatchAudioProfile, _ facts: OutputFacts) {
        let target = expectedKbps(profile, channels: fixture.expectedOutputChannels)
        let ratio = facts.estimatedRate / target
        Reporter.check(abs(ratio - 1) <= 0.15, "\(name)/bitrate", String(format: "avg=%.1fkbps target=%.0fkbps (%+.1f%%)", facts.estimatedRate / 1000, target / 1000, (ratio - 1) * 100))
    }

    func checkSize(_ name: String, _ fixture: Fixture, _ profile: WatchAudioProfile, _ facts: OutputFacts, _ prepared: WatchTranscoder.Prepared, encodeSeconds: Double) {
        let sourceBytes = ((try? FileManager.default.attributesOfItem(atPath: fixture.url.path))?[.size] as? NSNumber)?.int64Value ?? 1
        let ratio = Double(sourceBytes) / Double(max(prepared.bytes, 1))
        let speed = fixture.seconds / max(encodeSeconds, 0.0001)
        Reporter.check(prepared.bytes > 0 && prepared.bytes == facts.bytes, "\(name)/size",
                       String(format: "src=%lldB out=%lldB reduction=%.2fx speed=%.1fx realtime", sourceBytes, prepared.bytes, ratio, speed))
        rows.append(Row(label: fixture.label, profile: profile.rawValue, ratio: ratio, kbps: facts.estimatedRate / 1000, speed: speed, strategy: ""))
    }

    func checkTags(_ name: String, _ fixture: Fixture, _ output: URL) async {
        let outTags = await Inspector.tags(of: output)
        var mismatches: [String] = []
        for (key, expected) in fixture.tags {
            let actual = outTags[key]
            if key == "TRACKNUMBER" || key == "DISCNUMBER" {
                let want = Int(expected.split(separator: "/").first ?? "")
                let got = Int(actual ?? "")
                if want != got { mismatches.append("\(key) want \(String(describing: want)) got \(String(describing: got))") }
            } else if expected != actual {
                mismatches.append("\(key) differs")
            }
        }
        Reporter.check(mismatches.isEmpty, "\(name)/tags-itunes", "checked=\(fixture.tags.keys.sorted().joined(separator: ",")) \(mismatches.joined(separator: "; "))")

        let source = await AudioMetadataReader.read(from: fixture.url)
        let result = await AudioMetadataReader.read(from: output)
        let truth = fixture.tags
        let wantTrack = Int((truth["TRACKNUMBER"] ?? "").split(separator: "/").first ?? "") ?? 0
        let same = result.title == truth["TITLE"] && result.artist == truth["ARTIST"] && result.albumTitle == truth["ALBUM"]
            && result.albumArtist == truth["ALBUMARTIST"] && result.trackNumber == wantTrack
        let sourceAgrees = source.title == result.title && source.artist == result.artist && source.albumTitle == result.albumTitle
            && source.albumArtist == result.albumArtist && source.trackNumber == result.trackNumber
        Reporter.check(same, "\(name)/tags-reader", "track \(source.trackNumber)->\(result.trackNumber) (truth \(wantTrack)) title \(result.title == truth["TITLE"]) artist \(result.artist == truth["ARTIST"]) album \(result.albumTitle == truth["ALBUM"]) albumArtist \(result.albumArtist == truth["ALBUMARTIST"]) sourceReaderAgrees=\(sourceAgrees)")
        await checkArtwork(name, fixture, source: source, result: result)
    }

    func checkArtwork(_ name: String, _ fixture: Fixture, source: AudioMetadataReader.Result, result: AudioMetadataReader.Result) async {
        guard fixture.hasCover else {
            Reporter.check(true, "\(name)/artwork", "fixture has no cover; output artwork \(result.artworkData == nil ? "absent" : "present")")
            return
        }
        guard let data = result.artworkData, !data.isEmpty else {
            Reporter.check(false, "\(name)/artwork", "output has no artwork (source via AudioMetadataReader: \(source.artworkData == nil ? "none" : "present"))")
            return
        }
        let size = Inspector.imageSize(data)
        let isJPEG = data.prefix(2) == Data([0xFF, 0xD8])
        let fits = (size?.width ?? 9999) <= 400 && (size?.height ?? 9999) <= 400
        Reporter.check(data.count <= 60_000 && isJPEG && fits, "\(name)/artwork",
                       "\(data.count)B jpeg=\(isJPEG) \(size?.width ?? 0)x\(size?.height ?? 0) (source reader sees artwork: \(source.artworkData != nil))")
    }

    func checkGapless(_ name: String, _ fixture: Fixture, _ facts: OutputFacts) {
        let info = facts.packetInfo
        Reporter.check(facts.hasPacketInfo && info.mPrimingFrames > 0, "\(name)/gapless-priming",
                       "priming=\(info.mPrimingFrames) remainder=\(info.mRemainderFrames) valid=\(info.mNumberValidFrames)")
        let total = Int64(info.mPrimingFrames) + info.mNumberValidFrames + Int64(info.mRemainderFrames)
        Reporter.check(facts.packetCount > 0 && total == Int64(facts.packetCount) * 1024, "\(name)/gapless-accounting",
                       "priming+valid+remainder=\(total) packets*1024=\(Int64(facts.packetCount) * 1024)")
        let validSeconds = Double(info.mNumberValidFrames) / Double(max(facts.rate, 1))
        Reporter.check(abs(validSeconds - fixture.seconds) <= 0.030, "\(name)/gapless-valid-length",
                       String(format: "valid=%.4fs src=%.4fs", validSeconds, fixture.seconds))
    }

    func checkDecodeCount(_ name: String, _ facts: OutputFacts, _ url: URL) async {
        guard let decoded = try? await Inspector.decode(url, keepSamples: false) else {
            Reporter.check(false, "\(name)/decode-count", "decode failed")
            return
        }
        Reporter.check(Int64(decoded.frames) == facts.packetInfo.mNumberValidFrames, "\(name)/decode-count",
                       "decoded=\(decoded.frames) validFrames=\(facts.packetInfo.mNumberValidFrames)")
    }

    func checkCacheHit(_ name: String, _ fixture: Fixture, _ profile: WatchAudioProfile, first: WatchTranscoder.Prepared) async {
        let clock = ContinuousClock()
        let started = clock.now
        let again = try? await transcoder.prepare(source: fixture.url, profile: profile)
        let millis = durationSeconds(clock.now - started) * 1000
        Reporter.check(again?.url == first.url && again?.wasEncoded == true && millis < 50, "\(name)/cache-hit", String(format: "%.2fms", millis))
    }
}
