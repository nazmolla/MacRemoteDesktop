// BGRA -> full-range BT.709 NV12 on the GPU (Metal), for the H.264 encoder.
//
// The kernel is a line-for-line port of `bgra_to_nv12_full_range` in
// src/videotoolbox.rs (same constants, same order of operations, round half
// away from zero), compiled without fast math or fp contraction so its float
// results match the CPU path. Output goes straight into the encoder's
// IOSurface-backed NV12 CVPixelBuffer through a CVMetalTextureCache.
//
// Everything here is best effort: any failure returns non-zero and the caller
// converts on the CPU instead.

#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>

static NSString *const kShader = @
"#include <metal_stdlib>\n"
"#pragma METAL fp math_mode(safe)\n"
"#pragma METAL fp contract(off)\n"
"using namespace metal;\n"
"kernel void bgra_to_nv12(device const uchar *src [[buffer(0)]],\n"
"                         constant uint &stride [[buffer(1)]],\n"
"                         texture2d<uint, access::write> yt [[texture(0)]],\n"
"                         texture2d<uint, access::write> ct [[texture(1)]],\n"
"                         uint2 gid [[thread_position_in_grid]]) {\n"
"  if (gid.x >= ct.get_width() || gid.y >= ct.get_height()) return;\n"
"  const float KR = 0.2126f, KG = 0.7152f, KB = 0.0722f;\n"
"  float rs = 0.0f, gs = 0.0f, bs = 0.0f;\n"
"  for (uint dy = 0; dy < 2; dy++) {\n"
"    for (uint dx = 0; dx < 2; dx++) {\n"
"      uint x = gid.x * 2 + dx, y = gid.y * 2 + dy;\n"
"      device const uchar *p = src + y * stride + x * 4;\n"
"      float b = float(p[0]), g = float(p[1]), r = float(p[2]);\n"
"      float luma = KR * r + KG * g + KB * b;\n"
"      yt.write(uint4(uint(clamp(round(luma), 0.0f, 255.0f))), uint2(x, y));\n"
"      bs += b; gs += g; rs += r;\n"
"    }\n"
"  }\n"
"  float r = rs / 4.0f, g = gs / 4.0f, b = bs / 4.0f;\n"
"  float luma = KR * r + KG * g + KB * b;\n"
"  float cb = (b - luma) / 1.8556f + 128.0f;\n"
"  float cr = (r - luma) / 1.5748f + 128.0f;\n"
"  ct.write(uint4(uint(clamp(round(cb), 0.0f, 255.0f)), uint(clamp(round(cr), 0.0f, 255.0f)), 0, 0), gid);\n"
"}\n"
"// Same, reading the capture buffer as a texture (no copy). unorm reads are\n"
"// rounded back to the exact source bytes before the identical maths.\n"
"kernel void bgra_tex_to_nv12(texture2d<float, access::read> st [[texture(2)]],\n"
"                             texture2d<uint, access::write> yt [[texture(0)]],\n"
"                             texture2d<uint, access::write> ct [[texture(1)]],\n"
"                             uint2 gid [[thread_position_in_grid]]) {\n"
"  if (gid.x >= ct.get_width() || gid.y >= ct.get_height()) return;\n"
"  const float KR = 0.2126f, KG = 0.7152f, KB = 0.0722f;\n"
"  float rs = 0.0f, gs = 0.0f, bs = 0.0f;\n"
"  for (uint dy = 0; dy < 2; dy++) {\n"
"    for (uint dx = 0; dx < 2; dx++) {\n"
"      uint x = gid.x * 2 + dx, y = gid.y * 2 + dy;\n"
"      float4 px = st.read(uint2(x, y));\n"
"      float b = round(px.b * 255.0f), g = round(px.g * 255.0f), r = round(px.r * 255.0f);\n"
"      float luma = KR * r + KG * g + KB * b;\n"
"      yt.write(uint4(uint(clamp(round(luma), 0.0f, 255.0f))), uint2(x, y));\n"
"      bs += b; gs += g; rs += r;\n"
"    }\n"
"  }\n"
"  float r = rs / 4.0f, g = gs / 4.0f, b = bs / 4.0f;\n"
"  float luma = KR * r + KG * g + KB * b;\n"
"  float cb = (b - luma) / 1.8556f + 128.0f;\n"
"  float cr = (r - luma) / 1.5748f + 128.0f;\n"
"  ct.write(uint4(uint(clamp(round(cb), 0.0f, 255.0f)), uint(clamp(round(cr), 0.0f, 255.0f)), 0, 0), gid);\n"
"}\n";

static id<MTLDevice> gDevice;
static id<MTLCommandQueue> gQueue;
static id<MTLComputePipelineState> gPipeline;
static id<MTLComputePipelineState> gTexPipeline;
static CVPixelBufferPoolRef gPool;
static uint32_t gPoolW, gPoolH;
static CVMetalTextureCacheRef gCache;
static int gReady; // 1 = ready, -1 = unavailable

static void setup(void) {
    gDevice = MTLCreateSystemDefaultDevice();
    if (!gDevice) { gReady = -1; return; }
    MTLCompileOptions *opts = [MTLCompileOptions new];
    if (@available(macOS 15.0, *)) {
        opts.mathMode = MTLMathModeSafe;
    } else {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        opts.fastMathEnabled = NO; // macOS 14: mathMode does not exist yet
#pragma clang diagnostic pop
    }
    NSError *err = nil;
    id<MTLLibrary> lib = [gDevice newLibraryWithSource:kShader options:opts error:&err];
    id<MTLFunction> fn = [lib newFunctionWithName:@"bgra_to_nv12"];
    gPipeline = fn ? [gDevice newComputePipelineStateWithFunction:fn error:&err] : nil;
    id<MTLFunction> tfn = [lib newFunctionWithName:@"bgra_tex_to_nv12"];
    gTexPipeline = tfn ? [gDevice newComputePipelineStateWithFunction:tfn error:&err] : nil;
    gQueue = [gDevice newCommandQueue];
    if (!gPipeline || !gTexPipeline || !gQueue ||
        CVMetalTextureCacheCreate(kCFAllocatorDefault, NULL, gDevice, NULL, &gCache) != kCVReturnSuccess) {
        if (err) NSLog(@"viga gpu_convert: %@", err);
        gReady = -1;
        return;
    }
    gReady = 1;
}

static int ready(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{ setup(); });
    return gReady == 1;
}

// Attributes for an NV12 buffer the GPU can write: IOSurface-backed and
// Metal-compatible. The caller releases it. NULL when Metal is unavailable.
CFDictionaryRef macrdp_gpu_nv12_attributes(void) {
    if (!ready()) return NULL;
    NSDictionary *attrs = @{
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
    };
    return (CFDictionaryRef)CFBridgingRetain(attrs);
}

static id<MTLTexture> plane_texture(CVPixelBufferRef pb, size_t plane, MTLPixelFormat fmt, CVMetalTextureRef *out) {
    size_t w = CVPixelBufferGetWidthOfPlane(pb, plane), h = CVPixelBufferGetHeightOfPlane(pb, plane);
    if (CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, gCache, pb, NULL, fmt, w, h, plane, out)
        != kCVReturnSuccess) {
        return nil;
    }
    return CVMetalTextureGetTexture(*out);
}

// Convert `height` rows of BGRA (`stride` bytes apart) into `dst`, an even-sized
// 420f buffer created with `macrdp_gpu_nv12_attributes`. 0 = done.
int macrdp_gpu_bgra_to_nv12(const uint8_t *bgra, size_t stride, uint32_t width, uint32_t height,
                            CVPixelBufferRef dst) {
    if (!ready() || !bgra || !dst || (width & 1) || (height & 1)) return 1;
    if (CVPixelBufferGetWidth(dst) != width || CVPixelBufferGetHeight(dst) != height) return 2;
    @autoreleasepool {
        id<MTLBuffer> src = [gDevice newBufferWithBytes:bgra length:stride * height
                                                options:MTLResourceStorageModeShared];
        CVMetalTextureRef yRef = NULL, cRef = NULL;
        id<MTLTexture> yt = plane_texture(dst, 0, MTLPixelFormatR8Uint, &yRef);
        id<MTLTexture> ct = plane_texture(dst, 1, MTLPixelFormatRG8Uint, &cRef);
        int rc = 3;
        if (src && yt && ct) {
            id<MTLCommandBuffer> cb = [gQueue commandBuffer];
            id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
            uint32_t s = (uint32_t)stride;
            [enc setComputePipelineState:gPipeline];
            [enc setBuffer:src offset:0 atIndex:0];
            [enc setBytes:&s length:sizeof s atIndex:1];
            [enc setTexture:yt atIndex:0];
            [enc setTexture:ct atIndex:1];
            MTLSize grid = MTLSizeMake(width / 2, height / 2, 1);
            NSUInteger tw = gPipeline.threadExecutionWidth;
            NSUInteger th = MAX((NSUInteger)1, gPipeline.maxTotalThreadsPerThreadgroup / tw);
            [enc dispatchThreads:grid threadsPerThreadgroup:MTLSizeMake(tw, th, 1)];
            [enc endEncoding];
            [cb commit];
            [cb waitUntilCompleted];
            rc = cb.status == MTLCommandBufferStatusCompleted ? 0 : 4;
        }
        if (yRef) CFRelease(yRef);
        if (cRef) CFRelease(cRef);
        return rc;
    }
}

// A GPU-writable NV12 buffer from a reusable pool (+1; the caller releases).
// Called from the single encode thread. NULL when Metal is unavailable.
CVPixelBufferRef macrdp_gpu_create_nv12(uint32_t width, uint32_t height) {
    if (!ready()) return NULL;
    if (!gPool || gPoolW != width || gPoolH != height) {
        if (gPool) CVPixelBufferPoolRelease(gPool);
        gPool = NULL;
        NSDictionary *attrs = @{
            (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
            (__bridge NSString *)kCVPixelBufferWidthKey : @(width),
            (__bridge NSString *)kCVPixelBufferHeightKey : @(height),
            (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
            (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
        };
        if (CVPixelBufferPoolCreate(kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)attrs, &gPool)
            != kCVReturnSuccess) {
            gPool = NULL;
            return NULL;
        }
        gPoolW = width;
        gPoolH = height;
        CVMetalTextureCacheFlush(gCache, 0);
    }
    CVPixelBufferRef pb = NULL;
    if (CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, gPool, &pb) != kCVReturnSuccess) return NULL;
    return pb;
}

// Convert the capture buffer `src` (32BGRA, same size as `dst`) without copying it.
// 0 = done; non-zero = not done (the caller falls back).
int macrdp_gpu_convert_surface(CVPixelBufferRef src, CVPixelBufferRef dst) {
    if (!ready() || !src || !dst) return 1;
    size_t w = CVPixelBufferGetWidth(dst), h = CVPixelBufferGetHeight(dst);
    if (CVPixelBufferGetPixelFormatType(src) != kCVPixelFormatType_32BGRA ||
        CVPixelBufferGetWidth(src) != w || CVPixelBufferGetHeight(src) != h || (w & 1) || (h & 1)) {
        return 2;
    }
    @autoreleasepool {
        CVMetalTextureRef sRef = NULL, yRef = NULL, cRef = NULL;
        id<MTLTexture> st = nil;
        if (CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, gCache, src, NULL,
                                                      MTLPixelFormatBGRA8Unorm, w, h, 0, &sRef)
            == kCVReturnSuccess) {
            st = CVMetalTextureGetTexture(sRef);
        }
        id<MTLTexture> yt = plane_texture(dst, 0, MTLPixelFormatR8Uint, &yRef);
        id<MTLTexture> ct = plane_texture(dst, 1, MTLPixelFormatRG8Uint, &cRef);
        int rc = 3;
        if (st && yt && ct) {
            id<MTLCommandBuffer> cb = [gQueue commandBuffer];
            id<MTLComputeCommandEncoder> enc = [cb computeCommandEncoder];
            [enc setComputePipelineState:gTexPipeline];
            [enc setTexture:yt atIndex:0];
            [enc setTexture:ct atIndex:1];
            [enc setTexture:st atIndex:2];
            NSUInteger tw = gTexPipeline.threadExecutionWidth;
            NSUInteger th = MAX((NSUInteger)1, gTexPipeline.maxTotalThreadsPerThreadgroup / tw);
            [enc dispatchThreads:MTLSizeMake(w / 2, h / 2, 1) threadsPerThreadgroup:MTLSizeMake(tw, th, 1)];
            [enc endEncoding];
            [cb commit];
            [cb waitUntilCompleted];
            rc = cb.status == MTLCommandBufferStatusCompleted ? 0 : 4;
        }
        if (sRef) CFRelease(sRef);
        if (yRef) CFRelease(yRef);
        if (cRef) CFRelease(cRef);
        return rc;
    }
}

// Test hook: convert into a fresh GPU buffer and copy both planes out
// (tightly packed: `width` bytes per Y row, `width` bytes per CbCr row).
int macrdp_gpu_selftest(const uint8_t *bgra, size_t stride, uint32_t width, uint32_t height,
                        uint8_t *y_out, uint8_t *cbcr_out) {
    CFDictionaryRef attrs = macrdp_gpu_nv12_attributes();
    if (!attrs) return 10;
    CVPixelBufferRef pb = NULL;
    CVReturn st = CVPixelBufferCreate(kCFAllocatorDefault, width, height,
                                      kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, attrs, &pb);
    CFRelease(attrs);
    if (st != kCVReturnSuccess || !pb) return 11;
    int rc = macrdp_gpu_bgra_to_nv12(bgra, stride, width, height, pb);
    if (rc == 0) {
        CVPixelBufferLockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
        for (int plane = 0; plane < 2; plane++) {
            const uint8_t *base = CVPixelBufferGetBaseAddressOfPlane(pb, plane);
            size_t bpr = CVPixelBufferGetBytesPerRowOfPlane(pb, plane);
            size_t rows = plane == 0 ? height : height / 2;
            uint8_t *out = plane == 0 ? y_out : cbcr_out;
            for (size_t r = 0; r < rows; r++) memcpy(out + r * width, base + r * bpr, width);
        }
        CVPixelBufferUnlockBaseAddress(pb, kCVPixelBufferLock_ReadOnly);
    }
    CFRelease(pb);
    return rc;
}

// Test hook for the zero-copy path: wrap `bgra` in an IOSurface-backed 32BGRA
// buffer, convert it with macrdp_gpu_convert_surface, copy both planes out.
int macrdp_gpu_selftest_surface(const uint8_t *bgra, uint32_t width, uint32_t height,
                                uint8_t *y_out, uint8_t *cbcr_out) {
    if (!ready()) return 10;
    NSDictionary *attrs = @{
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
    };
    CVPixelBufferRef src = NULL;
    if (CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA,
                            (__bridge CFDictionaryRef)attrs, &src) != kCVReturnSuccess) return 11;
    CVPixelBufferLockBaseAddress(src, 0);
    uint8_t *base = CVPixelBufferGetBaseAddress(src);
    size_t bpr = CVPixelBufferGetBytesPerRow(src);
    for (uint32_t r = 0; r < height; r++) memcpy(base + r * bpr, bgra + (size_t)r * width * 4, (size_t)width * 4);
    CVPixelBufferUnlockBaseAddress(src, 0);
    CVPixelBufferRef dst = macrdp_gpu_create_nv12(width, height);
    int rc = dst ? macrdp_gpu_convert_surface(src, dst) : 12;
    if (rc == 0) {
        CVPixelBufferLockBaseAddress(dst, kCVPixelBufferLock_ReadOnly);
        for (int plane = 0; plane < 2; plane++) {
            const uint8_t *pb = CVPixelBufferGetBaseAddressOfPlane(dst, plane);
            size_t pbpr = CVPixelBufferGetBytesPerRowOfPlane(dst, plane);
            size_t rows = plane == 0 ? height : height / 2;
            uint8_t *out = plane == 0 ? y_out : cbcr_out;
            for (size_t r = 0; r < rows; r++) memcpy(out + r * width, pb + r * pbpr, width);
        }
        CVPixelBufferUnlockBaseAddress(dst, kCVPixelBufferLock_ReadOnly);
    }
    if (dst) CFRelease(dst);
    CFRelease(src);
    return rc;
}
