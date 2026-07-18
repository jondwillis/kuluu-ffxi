import AppKit
import Foundation
import Vision

guard CommandLine.arguments.count > 1,
    let img = NSImage(contentsOfFile: CommandLine.arguments[1]),
    let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil)
else {
    FileHandle.standardError.write("usage: swift hxi-ocr.swift <png>\n".data(using: .utf8)!)
    exit(2)
}

let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
req.usesLanguageCorrection = false
try VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])

let w = CGFloat(cg.width)
let h = CGFloat(cg.height)
for obs in req.results ?? [] {
    guard let cand = obs.topCandidates(1).first else { continue }
    let b = obs.boundingBox  // normalized, bottom-left origin
    let cx = Int(b.midX * w)
    let cy = Int((1 - b.midY) * h)
    print("\(cand.string)\t\(cx)\t\(cy)")
}
