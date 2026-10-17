import Foundation

nonisolated enum FlacTagging {

    struct Block {
        let type: UInt8
        let payload: Data
    }

    static func blocks(of data: Data) -> (blocks: [Block], audioOffset: Int)? {
        guard data.count > 4, data.prefix(4) == Data("fLaC".utf8) else { return nil }
        var offset = 4
        var result: [Block] = []
        while offset + 4 <= data.count {
            let flag = data[offset]
            let length = Int(data[offset + 1]) << 16 | Int(data[offset + 2]) << 8 | Int(data[offset + 3])
            let start = offset + 4
            guard start + length <= data.count else { return nil }
            result.append(Block(type: flag & 0x7F, payload: data.subdata(in: start..<(start + length))))
            offset = start + length
            if flag & 0x80 != 0 { break }
        }
        return (result, offset)
    }

    static func readComments(_ url: URL) -> [String: String] {
        guard let data = try? Data(contentsOf: url, options: .mappedIfSafe), let parsed = blocks(of: data),
              let block = parsed.blocks.first(where: { $0.type == 4 })
        else { return [:] }
        let bytes = [UInt8](block.payload)
        var offset = 0
        func readLE() -> Int? {
            guard offset + 4 <= bytes.count else { return nil }
            defer { offset += 4 }
            return Int(bytes[offset]) | Int(bytes[offset + 1]) << 8 | Int(bytes[offset + 2]) << 16 | Int(bytes[offset + 3]) << 24
        }
        guard let vendor = readLE() else { return [:] }
        offset += vendor
        guard let count = readLE() else { return [:] }
        var comments: [String: String] = [:]
        for _ in 0..<count {
            guard let length = readLE(), offset + length <= bytes.count else { break }
            let text = String(decoding: bytes[offset..<(offset + length)], as: UTF8.self)
            offset += length
            guard let separator = text.firstIndex(of: "=") else { continue }
            comments[String(text[..<separator]).uppercased()] = String(text[text.index(after: separator)...])
        }
        return comments
    }

    static func write(source: URL, to destination: URL, comments: [(String, String)], picture: Data?, mime: String) throws {
        let data = try Data(contentsOf: source)
        guard let parsed = blocks(of: data) else { throw CocoaError(.fileReadCorruptFile) }
        var kept = parsed.blocks.filter { ![1, 4, 6].contains($0.type) }
        kept.append(Block(type: 4, payload: commentPayload(comments)))
        if let picture { kept.append(Block(type: 6, payload: picturePayload(picture, mime: mime))) }
        var output = Data("fLaC".utf8)
        for (index, block) in kept.enumerated() {
            let last: UInt8 = index == kept.count - 1 ? 0x80 : 0
            output.append(block.type | last)
            let length = block.payload.count
            output.append(contentsOf: [UInt8((length >> 16) & 0xFF), UInt8((length >> 8) & 0xFF), UInt8(length & 0xFF)])
            output.append(block.payload)
        }
        output.append(data.suffix(from: parsed.audioOffset))
        try output.write(to: destination)
    }

    private static func littleEndian(_ value: Int) -> [UInt8] {
        [UInt8(value & 0xFF), UInt8((value >> 8) & 0xFF), UInt8((value >> 16) & 0xFF), UInt8((value >> 24) & 0xFF)]
    }

    private static func bigEndian(_ value: Int) -> [UInt8] {
        littleEndian(value).reversed()
    }

    private static func commentPayload(_ comments: [(String, String)]) -> Data {
        let vendor = Array("harness".utf8)
        var bytes = littleEndian(vendor.count) + vendor + littleEndian(comments.count)
        for (key, value) in comments {
            let line = Array("\(key)=\(value)".utf8)
            bytes += littleEndian(line.count) + line
        }
        return Data(bytes)
    }

    private static func picturePayload(_ image: Data, mime: String) -> Data {
        let mimeBytes = Array(mime.utf8)
        var bytes = bigEndian(3) + bigEndian(mimeBytes.count) + mimeBytes + bigEndian(0)
        bytes += bigEndian(0) + bigEndian(0) + bigEndian(24) + bigEndian(0)
        bytes += bigEndian(image.count)
        return Data(bytes) + image
    }
}
