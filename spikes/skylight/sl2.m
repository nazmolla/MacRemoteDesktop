#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <dlfcn.h>
typedef struct { float x, y; } F2; typedef struct { F2 r, g, b, w; } Chroma; typedef struct { unsigned w, h; } U2;
@interface NSObject (SL)
- (instancetype)initWithSizeInPixels:(U2)px sizeInPoints:(U2)pt refreshRate:(float)hz error:(NSError **)e;
- (instancetype)initWithNativeMode:(id)n preferredMode:(id)p optionalModes:(NSArray *)o rotations:(unsigned long long)r error:(NSError **)e;
- (instancetype)initWithName:(NSString *)n vendorID:(unsigned long long)v productID:(unsigned long long)p serialNumber:(unsigned long long)s sizeInMillimeters:(F2)mm maximumSizeInPixels:(U2)mx chromaticities:(Chroma)c error:(NSError **)e;
- (instancetype)initWithConfiguration:(id)c error:(NSError **)e;
- (BOOL)applySettings:(id)s error:(NSError **)e;
- (unsigned)displayID;
@end
static void list(unsigned did, const char *tag) {
  NSDictionary *o = @{(__bridge id)kCGDisplayShowDuplicateLowResolutionModes: @YES};
  NSArray *ms = CFBridgingRelease(CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)o));
  CGDisplayModeRef cur = CGDisplayCopyDisplayMode(did);
  printf("[%s] current %zux%zu@%zux%zu; %lu modes:", tag, cur ? CGDisplayModeGetWidth(cur) : 0, cur ? CGDisplayModeGetHeight(cur) : 0, cur ? CGDisplayModeGetPixelWidth(cur) : 0, cur ? CGDisplayModeGetPixelHeight(cur) : 0, (unsigned long)ms.count);
  for (id m in ms) { CGDisplayModeRef r = (__bridge CGDisplayModeRef)m; printf(" %zux%zu@%zu", CGDisplayModeGetWidth(r), CGDisplayModeGetHeight(r), CGDisplayModeGetPixelWidth(r)); }
  printf("\n"); if (cur) CGDisplayModeRelease(cur);
}
int main(int argc, char **argv) {
  @autoreleasepool {
    dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
    float mm = argc > 1 ? atof(argv[1]) : 600; unsigned pw = argc > 2 ? atoi(argv[2]) : 1920, ph = argc > 3 ? atoi(argv[3]) : 1080, ptw = argc > 4 ? atoi(argv[4]) : 1920, pth = argc > 5 ? atoi(argv[5]) : 1080;
    NSError *e = nil; Chroma c = {{0.64f,0.33f},{0.30f,0.60f},{0.15f,0.06f},{0.3127f,0.3290f}};
    id cfg = [[NSClassFromString(@"SLVirtualDisplayConfiguration") alloc] initWithName:@"sl2" vendorID:0x6D616372 productID:0x6D616372 serialNumber:getpid() sizeInMillimeters:(F2){mm, mm*(float)ph/(float)pw} maximumSizeInPixels:(U2){8192,8192} chromaticities:c error:&e];
    id vd = [[NSClassFromString(@"SLVirtualDisplay") alloc] initWithConfiguration:cfg error:&e];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.5]];
    list([vd displayID], "created");
    id m150 = [[NSClassFromString(@"SLVirtualDisplayMode") alloc] initWithSizeInPixels:(U2){pw,ph} sizeInPoints:(U2){ptw,pth} refreshRate:60 error:&e];
    id m100 = [[NSClassFromString(@"SLVirtualDisplayMode") alloc] initWithSizeInPixels:(U2){1920,1080} sizeInPoints:(U2){1920,1080} refreshRate:60 error:&e];
    id st = [[NSClassFromString(@"SLVirtualDisplaySettings") alloc] initWithNativeMode:m150 preferredMode:m150 optionalModes:@[] rotations:0 error:&e];
    BOOL ok = [vd applySettings:st error:&e];
    printf("apply=%d ", ok);
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:1.0]];
    list([vd displayID], "after 150% settings");
  }
}
