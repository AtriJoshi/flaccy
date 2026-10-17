import AVFoundation
import Foundation

@MainActor
func runHarness() async {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("flaccy-watch-harness-\(ProcessInfo.processInfo.processIdentifier)")
    try? FileManager.default.removeItem(at: root)
    try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let keep = ProcessInfo.processInfo.environment["FLACCY_HARNESS_KEEP"] != nil
    if keep { print("INFO workdir \(root.path)") }

    var fixtures: [Fixture] = []
    let cover = FixtureBuilder.coverPNG()
    let realFiles = FixtureBuilder.twoConsecutiveRealFiles()
    for (index, url) in realFiles.enumerated() {
        if let fixture = FixtureBuilder.describeReal(url, label: "real-\(index == 0 ? "A" : "B")") { fixtures.append(fixture) }
    }
    do {
        let synthDir = root.appendingPathComponent("synth")
        try FileManager.default.createDirectory(at: synthDir, withIntermediateDirectories: true)
        fixtures.append(try FixtureBuilder.makeFLAC(label: "hires96-24", in: synthDir, rate: 96_000, channels: 2, bits: 24, seconds: 20, tagged: true, cover: cover))
        fixtures.append(try FixtureBuilder.makeFLAC(label: "hires88-24", in: synthDir, rate: 88_200, channels: 2, bits: 24, seconds: 8, tagged: true, cover: cover))
        fixtures.append(try FixtureBuilder.makeFLAC(label: "mono44-16", in: synthDir, rate: 44_100, channels: 1, bits: 16, seconds: 10, tagged: true, cover: cover))
        fixtures.append(try FixtureBuilder.makeFLAC(label: "short0.4s", in: synthDir, rate: 44_100, channels: 2, bits: 16, seconds: 0.4, tagged: true, cover: cover))
        fixtures.append(try FixtureBuilder.makeFLAC(label: "rate32k", in: synthDir, rate: 32_000, channels: 2, bits: 16, seconds: 6, tagged: true, cover: nil))
        fixtures.append(try await FixtureBuilder.makeM4A(label: "alac44", in: synthDir, codec: "alac", rate: 44_100, seconds: 10, tagged: true, cover: cover))
        fixtures.append(try await FixtureBuilder.makeM4A(label: "aac-lossy", in: synthDir, codec: "aac", rate: 44_100, seconds: 6, tagged: false, cover: nil))
        fixtures.append(try FixtureBuilder.makeWAV(label: "wav8k-mono", in: synthDir, rate: 8_000, channels: 1, seconds: 6))
        fixtures.append(try FixtureBuilder.makeWAV(label: "wav22k-mono", in: synthDir, rate: 22_050, channels: 1, seconds: 6))
        do {
            fixtures.append(try FixtureBuilder.makeFLAC(label: "surround6ch", in: synthDir, rate: 48_000, channels: 6, bits: 24, seconds: 6, tagged: false, cover: nil))
        } catch {
            Reporter.skip("fixture/surround6ch", "could not synthesize 5.1 FLAC: \(error.localizedDescription.prefix(160))")
        }
    } catch {
        Reporter.check(false, "fixtures", "synthesis failed: \(error)")
        exit(2)
    }
    if realFiles.count < 2 { Reporter.skip("fixture/real", "no library FLACs found; real-track checks skipped") }

    var tiny = try? FixtureBuilder.makeFLAC(label: "tiny-quiet", in: root.appendingPathComponent("synth"), rate: 44_100, channels: 2, bits: 16, seconds: 0.4, tagged: true, cover: FixtureBuilder.tiledCoverPNG(), level: 0.003)
    tiny?.fallsBackAfterEncode = true
    let click = try? FixtureBuilder.makeFLAC(label: "click", in: root.appendingPathComponent("synth"), rate: 44_100, channels: 2, bits: 16, seconds: 3, tagged: false, cover: nil, clickAt: 66_150)
    let mp3Stub = try? FixtureBuilder.makeMP3Stub(in: root.appendingPathComponent("synth"))
    reportMP3Support(root: root)

    for fixture in fixtures {
        Reporter.note("fixture", "\(fixture.label) \(fixture.sourceRate)Hz \(fixture.sourceChannels)ch \(String(format: "%.2f", fixture.seconds))s \(fixture.kind == .lossless ? "lossless" : "lossy")")
    }

    let exercise = Exercise(cacheDirectory: root.appendingPathComponent("cache"))
    for fixture in fixtures where fixture.kind == .lossless { await exercise.encodeAll(fixture) }
    let lossless = fixtures.filter { $0.kind == .lossless }

    if let mp3Stub { await Scenarios.passthrough(fixtures, mp3Stub: mp3Stub, root: root) }
    await Scenarios.gaplessConcatenation(exercise.records["real-A/compact"], exercise.records["real-B/compact"])
    if let click { await Scenarios.clickAlignment(click, clickAt: 66_150, root: root) }

    if let longest = fixtures.first(where: { $0.isReal }) ?? lossless.max(by: { $0.seconds < $1.seconds }) {
        let other = lossless.first(where: { $0.label == "hires88-24" }) ?? longest
        await Scenarios.cancelByTask(longest.url, profile: .compact, root: root)
        await Scenarios.cancelBySource(longest.url, profile: .compact, root: root)
        await Scenarios.cancelQueued(longest.url, other.url, profile: .compact, root: root)
        await Scenarios.cancelBySourceWhileQueued(longest.url, other.url, profile: .compact, root: root)
        await Scenarios.sequential(Array(lossless.prefix(3)).map(\.url), root: root)
        await Scenarios.releaseAndPurge(other, root: root)
    }
    if let tiny { await Scenarios.fallsBackWhenOutputWouldBeBigger(tiny, root: root) }
    await Scenarios.failureLeavesNothing(root: root)

    print("TABLE label profile reduction avg-kbps speed")
    for row in exercise.rows {
        print("TABLE \(row.label) \(row.profile) \(String(format: "%.2fx", row.ratio)) \(String(format: "%.1f", row.kbps)) \(String(format: "%.0fx", row.speed))")
    }
    print("SUMMARY pass=\(Reporter.passes) fail=\(Reporter.failures)")
    if !keep { try? FileManager.default.removeItem(at: root) }
    exit(Reporter.failures == 0 ? 0 : 1)
}

@MainActor
func reportMP3Support(root: URL) {
    let wav = root.appendingPathComponent("synth/mp3probe.wav")
    try? FixtureBuilder.writeWAV(to: wav, rate: 44_100, channels: 2, bits: 16, seconds: 1)
    let ok = FixtureBuilder.tryEncodeMP3(from: wav, to: root.appendingPathComponent("synth/probe.mp3"))
    if ok {
        Reporter.note("mp3", "afconvert can encode MP3 here")
    } else {
        Reporter.skip("fixture/mp3", "afconvert cannot encode MP3 on this Mac; a stub file named .mp3 covers the extension path")
    }
}

await runHarness()
