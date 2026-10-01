// Throwaway feasibility probe: drive SkyLight's SLVirtualDisplay directly.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>
#import <dlfcn.h>
typedef struct { float x, y; } F2;
typedef struct { F2 r, g, b, w; } Chroma;
typedef struct { unsigned w, h; } U2;
@interface SLVirtualDisplayMode : NSObject
- (instancetype)initWithSizeInPixels:(U2)px sizeInPoints:(U2)pt refreshRate:(float)hz error:(NSError **)e;
@end
@interface SLVirtualDisplaySettings : NSObject
- (instancetype)initWithNativeMode:(id)n preferredMode:(id)p optionalModes:(NSArray *)o rotations:(unsigned long long)r error:(NSError **)e;
@end
@interface SLVirtualDisplayConfiguration : NSObject
- (instancetype)initWithName:(NSString *)n vendorID:(unsigned long long)v productID:(unsigned long long)p serialNumber:(unsigned long long)s sizeInMillimeters:(F2)mm maximumSizeInPixels:(U2)mx chromaticities:(Chroma)c error:(NSError **)e;
@end
@interface SLVirtualDisplay : NSObject
- (instancetype)initWithConfiguration:(id)c error:(NSError **)e;
- (BOOL)applySettings:(id)s error:(NSError **)e;
- (unsigned)displayID;
- (void)destroy;
@end

static NSString *active(CGDirectDisplayID id) {
  CGDisplayModeRef m = CGDisplayCopyDisplayMode(id);
  if (!m) return @"nomode";
  NSString *s = [NSString stringWithFormat:@"%zux%zu pt @ %zux%zu px", CGDisplayModeGetWidth(m), CGDisplayModeGetHeight(m), CGDisplayModeGetPixelWidth(m), CGDisplayModeGetPixelHeight(m)];
  CGDisplayModeRelease(m); return s;
}
static BOOL waitMode(CGDirectDisplayID id, unsigned pw, unsigned ph, unsigned ptw, double *secs) {
  NSDate *t0 = [NSDate date];
  while ([[NSDate date] timeIntervalSinceDate:t0] < 5) {
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(id);
    BOOL ok = m && CGDisplayModeGetPixelWidth(m) == pw && CGDisplayModeGetPixelHeight(m) == ph && CGDisplayModeGetWidth(m) == ptw;
    if (m) CGDisplayModeRelease(m);
    if (ok) { *secs = [[NSDate date] timeIntervalSinceDate:t0]; return YES; }
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
  }
  return NO;
}
static SLVirtualDisplay *make(unsigned serial, NSError **e) {
  Chroma srgb = {{0.64f, 0.33f}, {0.30f, 0.60f}, {0.15f, 0.06f}, {0.3127f, 0.3290f}};
  id cfg = [[NSClassFromString(@"SLVirtualDisplayConfiguration") alloc] initWithName:@"slprobe" vendorID:0x6D616372 productID:0x6D616372 serialNumber:serial sizeInMillimeters:(F2){600, 338} maximumSizeInPixels:(U2){8192, 8192} chromaticities:srgb error:e];
  if (!cfg) return nil;
  return [[NSClassFromString(@"SLVirtualDisplay") alloc] initWithConfiguration:cfg error:e];
}
static BOOL setMode(SLVirtualDisplay *vd, unsigned pw, unsigned ph, unsigned ptw, unsigned pth, NSString *tag) {
  NSError *e = nil;
  id mode = [[NSClassFromString(@"SLVirtualDisplayMode") alloc] initWithSizeInPixels:(U2){pw, ph} sizeInPoints:(U2){ptw, pth} refreshRate:60 error:&e];
  if (!mode) { printf("  %s: mode init error %s\n", tag.UTF8String, e.description.UTF8String); return NO; }
  id st = [[NSClassFromString(@"SLVirtualDisplaySettings") alloc] initWithNativeMode:mode preferredMode:mode optionalModes:@[] rotations:0 error:&e];
  if (!st) { printf("  %s: settings error %s\n", tag.UTF8String, e.description.UTF8String); return NO; }
  if (![vd applySettings:st error:&e]) { printf("  %s: apply error %s\n", tag.UTF8String, e.description.UTF8String); return NO; }
  double t = 0; BOOL ok = waitMode(vd.displayID, pw, ph, ptw, &t);
  printf("  %-22s %s  (%s, %.2fs)\n", tag.UTF8String, ok ? "OK  " : "FAIL", active(vd.displayID).UTF8String, t);
  return ok;
}
int main(void) {
  @autoreleasepool {
    dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
    NSError *e = nil; int ok = 0, total = 0;
    SLVirtualDisplay *vd = make(getpid() * 16, &e);
    if (!vd) { printf("create error: %s\n", e.description.UTF8String); return 1; }
    printf("created id=%u\n", vd.displayID);
    struct { unsigned pw, ph, ptw, pth; const char *tag; } seq[] = {
      {3840, 2160, 2560, 1440, "150% 4K"}, {1920, 1080, 1920, 1080, "100% 1080p"}, {3840, 2160, 1920, 1080, "200% 4K"},
      {1714, 1288, 1714, 1288, "100% odd window"}, {3000, 2000, 2000, 1333, "150% 3000x2000"}, {2560, 1440, 2048, 1152, "125% 1440p"},
      {1714, 1287, 1143, 858, "150% odd height"}, {1920, 1080, 1920, 1080, "back to 100%"}, {5120, 2880, 2560, 1440, "200% 5K"},
      {3440, 1440, 3440, 1440, "100% ultrawide"} };
    for (unsigned i = 0; i < sizeof seq / sizeof *seq; i++) { total++; ok += setMode(vd, seq[i].pw, seq[i].ph, seq[i].ptw, seq[i].pth, [NSString stringWithUTF8String:seq[i].tag]); }
    NSScreen *scr = nil; for (NSScreen *s in NSScreen.screens) if ([s.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == vd.displayID) scr = s;
    printf("NSScreen backingScale=%.2f frame=%.0fx%.0f\n", scr.backingScaleFactor, scr.frame.size.width, scr.frame.size.height);
    printf("re-modes: %d/%d\n", ok, total);
    int rc = 0;
    for (int i = 0; i < 5; i++) {
      [vd destroy]; vd = nil; [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.3]];
      vd = make(getpid() * 16 + 1 + i, &e);
      if (vd && setMode(vd, 1920, 1080, 1920, 1080, [NSString stringWithFormat:@"recreate #%d", i + 1])) rc++;
      else if (!vd) printf("  recreate #%d error %s\n", i + 1, e.description.UTF8String);
    }
    printf("destroy+recreate in same process: %d/5\n", rc);
    SLVirtualDisplay *vd2 = make(getpid() * 16 + 9, &e);
    BOOL two = vd2 && setMode(vd2, 2560, 1440, 2560, 1440, @"second display");
    printf("two simultaneous displays: %s (ids %u, %u)\n", two ? "OK" : "FAIL", vd.displayID, vd2.displayID);
    [vd2 destroy]; [vd destroy];
  }
  return 0;
}
