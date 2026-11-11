// Draws the 1024×1024 app icon: the LaunchX arrow on a blue rounded square.
// Usage: swiftc icon.swift -o icon && ./icon out.png
import AppKit

let side = 1024
let output = CommandLine.arguments.dropFirst().first ?? "icon.png"
guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side, bitsPerSample: 8, samplesPerPixel: 4,
    hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
else { fatalError("no bitmap") }
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)

// macOS icon grid: an 824-point body centred on a 1024 canvas.
let body = NSRect(x: 100, y: 100, width: 824, height: 824)
let shape = NSBezierPath(roundedRect: body, xRadius: 185, yRadius: 185)

NSGraphicsContext.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = NSColor.black.withAlphaComponent(0.28)
shadow.shadowBlurRadius = 28
shadow.shadowOffset = NSSize(width: 0, height: -12)
shadow.set()
NSColor(red: 0.23, green: 0.36, blue: 0.86, alpha: 1).setFill()
shape.fill()
NSGraphicsContext.restoreGraphicsState()

NSGradient(
    starting: NSColor(red: 0.36, green: 0.49, blue: 0.98, alpha: 1),
    ending: NSColor(red: 0.18, green: 0.28, blue: 0.74, alpha: 1)
)!.draw(in: shape, angle: -90)

// The favicon's arrow, "M9 22 16 8l7 14-7-4z" on a 32-unit grid, scaled onto the body.
let scale = 824.0 / 32.0
func point(_ x: Double, _ y: Double) -> NSPoint { NSPoint(x: 100 + x * scale, y: 100 + (32 - y) * scale) }
let arrow = NSBezierPath()
arrow.move(to: point(9, 22.5))
arrow.line(to: point(16, 7.5))
arrow.line(to: point(23, 22.5))
arrow.line(to: point(16, 18.2))
arrow.close()
arrow.lineJoinStyle = .round
NSColor.white.setFill()
arrow.fill()

NSGraphicsContext.restoreGraphicsState()
guard let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("no png") }
try png.write(to: URL(fileURLWithPath: output))
