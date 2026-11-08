import AppKit
import ImageIO
import UniformTypeIdentifiers
let folder=URL(fileURLWithPath:CommandLine.arguments[1])
for key in ["A","B","C"] {
    let source=CGImageSourceCreateWithURL(folder.appendingPathComponent("hero-\(key).png") as CFURL,nil)!
    let image=CGImageSourceCreateImageAtIndex(source,0,nil)!
    assert(image.width==1024 && image.height==1024)
    let ctx=CGContext(data:nil,width:128,height:128,bitsPerComponent:8,bytesPerRow:512,
        space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.interpolationQuality = .high
    ctx.draw(image,in:CGRect(x:0,y:0,width:128,height:128))
    let bytes=ctx.data!.assumingMemoryBound(to:UInt8.self)
    for p in 0..<(128*128) {
        let i=p*4, alpha=Double(bytes[i+3])/255
        if alpha==0 { continue }
        let luminance=(0.2126*Double(bytes[i]) + 0.7152*Double(bytes[i+1]) + 0.0722*Double(bytes[i+2]))/(255*alpha)
        for (channel,pair) in [(16.0,206.0),(30.0,225.0),(44.0,243.0)].enumerated() {
            bytes[i+channel]=UInt8(max(0,min(255,(pair.0+(pair.1-pair.0)*luminance)*alpha)))
        }
    }
    assert(bytes[3]==0)
    let dest=CGImageDestinationCreateWithURL(folder.appendingPathComponent("hero-\(key)-tinted.png") as CFURL,UTType.png.identifier as CFString,1,nil)!
    CGImageDestinationAddImage(dest,ctx.makeImage()!,nil)
    assert(CGImageDestinationFinalize(dest))
}
