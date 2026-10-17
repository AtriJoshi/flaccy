import AVFoundation
import FlaccyCore
import Foundation

@MainActor
enum Scenarios {

    static func listing(_ directory: URL) -> [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []).sorted()
    }

    static func freshDirectory(_ root: URL, _ name: String) -> URL {
        let url = root.appendingPathComponent(name)
        try? FileManager.default.removeItem(at: url)
        return url
    }

    static func waitForPartial(in directory: URL, timeout: Double = 5) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if listing(directory).contains(where: { $0.hasSuffix(".partial") }) { return true }
            try? await Task.sleep(nanoseconds: 2_000_000)
        }
        return false
    }

    static func isCancelled(_ error: Error?) -> Bool {
        if case WatchTranscoder.TranscodeError.cancelled? = error as? WatchTranscoder.TranscodeError { return true }
        return false
    }

    static func cancelByTask(_ source: URL, profile: WatchAudioProfile, root: URL) async {
        let directory = freshDirectory(root, "cancel-task")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let task = Task { try await transcoder.prepare(source: source, profile: profile) }
        let sawPartial = await waitForPartial(in: directory)
        task.cancel()
        let result = await task.result
        try? await Task.sleep(nanoseconds: 300_000_000)
        var failure: Error?
        if case .failure(let error) = result { failure = error }
        Reporter.check(sawPartial && isCancelled(failure) && listing(directory).isEmpty, "cancel/task-cancel",
                       "sawPartial=\(sawPartial) error=\(String(describing: failure)) leftover=\(listing(directory))")
    }

    static func cancelBySource(_ source: URL, profile: WatchAudioProfile, root: URL) async {
        let directory = freshDirectory(root, "cancel-source")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let task = Task { try await transcoder.prepare(source: source, profile: profile) }
        let sawPartial = await waitForPartial(in: directory)
        await transcoder.cancel(source: source)
        let result = await task.result
        try? await Task.sleep(nanoseconds: 300_000_000)
        var failure: Error?
        if case .failure(let error) = result { failure = error }
        Reporter.check(sawPartial && isCancelled(failure) && listing(directory).isEmpty, "cancel/cancel-source",
                       "sawPartial=\(sawPartial) error=\(String(describing: failure)) leftover=\(listing(directory))")
    }

    static func cancelQueued(_ first: URL, _ second: URL, profile: WatchAudioProfile, root: URL) async {
        let directory = freshDirectory(root, "cancel-queued")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let running = Task { try await transcoder.prepare(source: first, profile: profile) }
        _ = await waitForPartial(in: directory)
        let queued = Task { try await transcoder.prepare(source: second, profile: profile) }
        try? await Task.sleep(nanoseconds: 20_000_000)
        queued.cancel()
        let queuedResult = await queued.result
        let runningResult = await running.result
        var queuedError: Error?
        if case .failure(let error) = queuedResult { queuedError = error }
        var runningOK = false
        if case .success = runningResult { runningOK = true }
        Reporter.check(isCancelled(queuedError) && runningOK && listing(directory).count == 1, "cancel/queued-waiter",
                       "queuedError=\(String(describing: queuedError)) runningOK=\(runningOK) files=\(listing(directory).count)")
    }

    static func cancelBySourceWhileQueued(_ first: URL, _ second: URL, profile: WatchAudioProfile, root: URL) async {
        let directory = freshDirectory(root, "cancel-queued-source")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let running = Task { try await transcoder.prepare(source: first, profile: profile) }
        _ = await waitForPartial(in: directory)
        let queued = Task { try await transcoder.prepare(source: second, profile: profile) }
        try? await Task.sleep(nanoseconds: 20_000_000)
        await transcoder.cancel(source: second)
        let queuedResult = await queued.result
        _ = await running.result
        var queuedError: Error?
        if case .failure(let error) = queuedResult { queuedError = error }
        Reporter.check(isCancelled(queuedError), "cancel/cancel-source-queued", "queuedError=\(String(describing: queuedError))")
    }

    static func sequential(_ sources: [URL], root: URL) async {
        let directory = freshDirectory(root, "sequential")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let tasks = sources.map { source in Task { try await transcoder.prepare(source: source, profile: .balanced) } }
        var maxPartials = 0
        var finished = 0
        while finished < tasks.count {
            let partials = listing(directory).filter { $0.hasSuffix(".partial") }.count
            maxPartials = max(maxPartials, partials)
            finished = 0
            for task in tasks where task.isCancelled == false { _ = task }
            let done = listing(directory).filter { $0.hasSuffix(".m4a") }.count
            finished = done
            try? await Task.sleep(nanoseconds: 1_000_000)
        }
        var allOK = true
        for task in tasks { if case .failure = await task.result { allOK = false } }
        Reporter.check(maxPartials == 1 && allOK, "sequential/one-at-a-time", "queued=\(sources.count) maxConcurrentPartials=\(maxPartials) allOK=\(allOK)")
    }

    static func fallsBackWhenOutputWouldBeBigger(_ fixture: Fixture, root: URL) async {
        let directory = freshDirectory(root, "bigger")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let result = try? await transcoder.prepare(source: fixture.url, profile: .high)
        Reporter.check(
            result?.wasEncoded == false && result?.url == fixture.url && result?.bytes == fixture.sourceBytes && listing(directory).isEmpty,
            "never-bigger/post-encode-fallback",
            String(format: "source %.1fkbps (%lldB) is above 1.15 x target %.1fkbps so it was encoded, found larger, deleted; wasEncoded=%@ leftover=%@",
                   fixture.sourceKbps, fixture.sourceBytes, fixture.targetKbps(.high), String(describing: result?.wasEncoded), listing(directory).description)
        )
    }

    static func failureLeavesNothing(root: URL) async {
        let directory = freshDirectory(root, "failure")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        let bogus = root.appendingPathComponent("bogus.flac")
        try? Data("fLaC".utf8 + Data(repeating: 0x5A, count: 20_000)).write(to: bogus)
        let outcome = await Result { try await transcoder.prepare(source: bogus, profile: .high) }
        var description = "succeeded"
        if case .failure(let error) = outcome { description = "threw \(error)" }
        Reporter.check(listing(directory).filter { $0.hasSuffix(".partial") }.isEmpty && listing(directory).isEmpty, "failure/corrupt-flac", "\(description) leftover=\(listing(directory))")

        let missing = root.appendingPathComponent("missing.flac")
        var missingError: Error?
        if case .failure(let error) = await Result({ try await transcoder.prepare(source: missing, profile: .high) }) { missingError = error }
        var isUnreadable = false
        if case WatchTranscoder.TranscodeError.unreadable? = missingError as? WatchTranscoder.TranscodeError { isUnreadable = true }
        Reporter.check(isUnreadable, "failure/missing-file", "error=\(String(describing: missingError))")
    }

    static func passthrough(_ fixtures: [Fixture], mp3Stub: URL, root: URL) async {
        let transcoder = WatchTranscoder(cacheDirectory: freshDirectory(root, "passthrough"))
        for fixture in fixtures.filter({ $0.kind == .lossless }).prefix(2) {
            let result = try? await transcoder.prepare(source: fixture.url, profile: .original)
            Reporter.check(result?.url == fixture.url && result?.wasEncoded == false && result?.destinationExtension == fixture.url.pathExtension,
                           "passthrough/original/\(fixture.label)", "ext=\(result?.destinationExtension ?? "nil")")
        }
        for fixture in fixtures where fixture.kind == .lossy {
            for profile in Exercise.profiles {
                let result = try? await transcoder.prepare(source: fixture.url, profile: profile)
                Reporter.check(result?.url == fixture.url && result?.wasEncoded == false && result?.destinationExtension == "m4a",
                               "passthrough/lossy-m4a/\(fixture.label)/\(profile.rawValue)", "wasEncoded=\(String(describing: result?.wasEncoded))")
            }
        }
        let stub = try? await transcoder.prepare(source: mp3Stub, profile: .high)
        Reporter.check(stub?.url == mp3Stub && stub?.wasEncoded == false && stub?.destinationExtension == "mp3",
                       "passthrough/mp3-extension", "stub file named .mp3 (no real MP3 encoder on this Mac)")
    }

    static func releaseAndPurge(_ fixture: Fixture, root: URL) async {
        let directory = freshDirectory(root, "housekeeping")
        let transcoder = WatchTranscoder(cacheDirectory: directory)
        guard let high = try? await transcoder.prepare(source: fixture.url, profile: .high),
              let compact = try? await transcoder.prepare(source: fixture.url, profile: .compact),
              let balanced = try? await transcoder.prepare(source: fixture.url, profile: .balanced)
        else {
            Reporter.check(false, "housekeeping/setup", "prepare failed")
            return
        }
        await transcoder.purgeStale(keeping: [high.url], olderThan: 3600)
        Reporter.check(listing(directory).count == 3, "housekeeping/purge-keeps-recent", "files=\(listing(directory).count)")
        await transcoder.purgeStale(keeping: [high.url], olderThan: 0)
        let remaining = listing(directory)
        Reporter.check(remaining == [high.url.lastPathComponent], "housekeeping/purge-spares-kept", "remaining=\(remaining.count)")
        await transcoder.release(high)
        Reporter.check(listing(directory).isEmpty, "housekeeping/release-deletes", "files=\(listing(directory).count)")
        _ = (compact, balanced)
        let passthrough = try? await transcoder.prepare(source: fixture.url, profile: .original)
        if let passthrough { await transcoder.release(passthrough) }
        Reporter.check(FileManager.default.fileExists(atPath: fixture.url.path), "housekeeping/release-spares-source", "source still exists")
    }

    static func gaplessConcatenation(_ first: EncodedRecord?, _ second: EncodedRecord?) async {
        guard let first, let second else {
            Reporter.skip("gapless/concat", "need two real consecutive tracks")
            return
        }
        guard let a = try? await Inspector.decode(first.prepared.url, keepSamples: false),
              let b = try? await Inspector.decode(second.prepared.url, keepSamples: false)
        else {
            Reporter.check(false, "gapless/concat", "decode failed")
            return
        }
        let validSum = first.facts.packetInfo.mNumberValidFrames + second.facts.packetInfo.mNumberValidFrames
        let decodedSum = Int64(a.frames + b.frames)
        let rawSum = Int64(first.facts.packetCount + second.facts.packetCount) * 1024
        Reporter.check(decodedSum == validSum, "gapless/concat", "decoded A+B=\(decodedSum) validFrames A+B=\(validSum) (without trimming it would be \(rawSum), padding removed=\(rawSum - decodedSum))")
    }

    static func clickAlignment(_ fixture: Fixture, clickAt: Int, root: URL) async {
        let transcoder = WatchTranscoder(cacheDirectory: freshDirectory(root, "click"))
        guard let prepared = try? await transcoder.prepare(source: fixture.url, profile: .high),
              let decoded = try? await Inspector.decode(prepared.url, keepSamples: true)
        else {
            Reporter.check(false, "gapless/click-alignment", "encode or decode failed")
            return
        }
        var peak = 0
        var peakValue: Float = 0
        for (index, value) in decoded.firstChannel.enumerated() where abs(value) > peakValue {
            peakValue = abs(value)
            peak = index
        }
        Reporter.check(abs(peak - clickAt) <= 16, "gapless/click-alignment", "click at \(clickAt) decoded peak at \(peak) (offset \(peak - clickAt) samples) frames=\(decoded.frames)")
    }
}

extension Result where Failure == Error {
    @MainActor
    init(_ body: () async throws -> Success) async {
        do { self = .success(try await body()) } catch { self = .failure(error) }
    }
}
