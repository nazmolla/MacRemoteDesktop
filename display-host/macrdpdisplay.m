// macrdpdisplay: owns ONE virtual display for its whole life, driven by Portico over stdin.
//
//   create <serial> <pixW> <pixH> <scale 1|2>  -> ok <id> <ptW> <ptH> <pixW> <pixH> | err <msg>
//   mode <pixW> <pixH> <scale 1|2>             -> same, or "err needs-replace"
//   quit (or end of input)                      -> removes the display and exits
//
// Why a separate process: a second CGVirtualDisplay created in the same process never comes
// online, and an explicit mode selection freezes a display's mode list for the life of the
// process. A fresh process per display avoids both (docs/research/2026-10-01-virtual-display-
// stability.md, spikes/displayhost).
//
// Every create / mode / destroy runs under a per-user lock and holds it until WindowServer has
// stopped reconfiguring: overlapping display changes have aborted WindowServer.
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <signal.h>
#include <sys/file.h>
#include <unistd.h>

@interface CGVirtualDisplayDescriptor : NSObject
@property(retain, nonatomic) dispatch_queue_t queue;
@property(retain, nonatomic) NSString *name;
@property(nonatomic) unsigned int maxPixelsWide, maxPixelsHigh, productID, vendorID, serialNum;
@property(nonatomic) CGSize sizeInMillimeters;
@property(nonatomic) CGPoint redPrimary, greenPrimary, bluePrimary, whitePoint;
@end
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)w height:(unsigned int)h refreshRate:(double)r;
@end
@interface CGVirtualDisplaySettings : NSObject
@property(retain, nonatomic) NSArray *modes;
@property(nonatomic) unsigned int hiDPI;
@end
@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)d;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)s;
@property(readonly, nonatomic) unsigned int displayID;
@end

// "macr", the same identity the in-process display uses.
static const unsigned kVendor = 0x6D616372, kProduct = 0x6D616372;

static CGVirtualDisplay *gDisplay;
static NSString *gName = @"Portico";
static unsigned gSerial;
static int gLockFd = -1;
// Set once a mode was selected explicitly; that freezes the mode list, so later re-modes
// must go through a fresh host.
static int gPinned;

static void reply(NSString *s) {
  printf("%s\n", s.UTF8String);
  fflush(stdout);
}

// Let CoreGraphics process display-change notifications between polls.
static void pump(void) { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, false); }

static BOOL currentIs(CGDirectDisplayID did, unsigned ptw, unsigned pw, unsigned out[4]) {
  CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
  if (!m) return NO;
  out[0] = (unsigned)CGDisplayModeGetWidth(m);
  out[1] = (unsigned)CGDisplayModeGetHeight(m);
  out[2] = (unsigned)CGDisplayModeGetPixelWidth(m);
  out[3] = (unsigned)CGDisplayModeGetPixelHeight(m);
  CGDisplayModeRelease(m);
  return out[0] == ptw && out[2] == pw;
}

// The process that creates a display can be left with an empty mode cache for it
// (CGDisplayCopyDisplayMode returns NULL while other processes see the mode), so the 1×
// check uses values WindowServer answers directly. At 1× points equal pixels.
static BOOL oneXIs(CGDirectDisplayID did, unsigned pw, unsigned ph, unsigned cur[4]) {
  CGRect b = CGDisplayBounds(did);
  cur[0] = (unsigned)b.size.width;
  cur[1] = (unsigned)b.size.height;
  cur[2] = (unsigned)CGDisplayPixelsWide(did);
  cur[3] = (unsigned)CGDisplayPixelsHigh(did);
  return cur[0] == pw && cur[1] == ph && cur[2] == pw && cur[3] == ph;
}

static BOOL waitOneX(CGDirectDisplayID did, unsigned pw, unsigned ph, double seconds, unsigned cur[4]) {
  CFAbsoluteTime end = CFAbsoluteTimeGetCurrent() + seconds;
  do {
    if (oneXIs(did, pw, ph, cur)) return YES;
    pump();
  } while (CFAbsoluteTimeGetCurrent() < end);
  return oneXIs(did, pw, ph, cur);
}

static NSString *okReply(CGDirectDisplayID did, const unsigned cur[4]) {
  return [NSString stringWithFormat:@"ok %u %u %u %u %u", did, cur[0], cur[1], cur[2], cur[3]];
}

static CGDisplayModeRef findMode(CGDirectDisplayID did, unsigned ptw, unsigned pth, unsigned pw) {
  NSDictionary *o = @{(__bridge id)kCGDisplayShowDuplicateLowResolutionModes : @YES};
  NSArray *ms = CFBridgingRelease(CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)o));
  for (id m in ms) {
    CGDisplayModeRef r = (__bridge CGDisplayModeRef)m;
    if (CGDisplayModeGetWidth(r) == ptw && CGDisplayModeGetHeight(r) == pth &&
        CGDisplayModeGetPixelWidth(r) == pw && CGDisplayModeIsUsableForDesktopGUI(r))
      return CGDisplayModeRetain(r);
  }
  return NULL;
}

static BOOL registerMode(unsigned ptw, unsigned pth, int hidpi) {
  CGVirtualDisplaySettings *s = [[CGVirtualDisplaySettings alloc] init];
  s.hiDPI = hidpi ? 1 : 0;
  s.modes = @[ [[CGVirtualDisplayMode alloc] initWithWidth:ptw height:pth refreshRate:60] ];
  return [gDisplay applySettings:s];
}

// Wait up to `seconds` for the wanted mode to become current on its own.
static BOOL waitCurrent(CGDirectDisplayID did, unsigned ptw, unsigned pw, double seconds, unsigned cur[4]) {
  CFAbsoluteTime end = CFAbsoluteTimeGetCurrent() + seconds;
  do {
    if (currentIs(did, ptw, pw, cur)) return YES;
    pump();
  } while (CFAbsoluteTimeGetCurrent() < end);
  return currentIs(did, ptw, pw, cur);
}

// 1×: register the single mode and let it become current by itself, re-registering once
// (a 1× registration right after a Retina request can be ignored). Never selects explicitly,
// matching the in-process display that works today.
// Retina: register with hiDPI=1; if WindowServer's choice is not the Retina variant, select it
// explicitly. That pins this host: later re-modes answer "err needs-replace".
static NSString *commit(unsigned pw, unsigned ph, unsigned scale) {
  if (scale != 1 && scale != 2) return @"err scale must be 1 or 2";
  if (pw == 0 || ph == 0 || pw > 8192 || ph > 8192) return @"err pixel size out of range";
  if ((pw % scale) || (ph % scale)) return @"err pixel size not divisible by scale";
  if (gPinned) return @"err needs-replace";
  unsigned ptw = pw / scale, pth = ph / scale, cur[4] = {0};
  CGDirectDisplayID did = gDisplay.displayID;

  if (scale == 1) {
    if (oneXIs(did, pw, ph, cur)) return okReply(did, cur);
    for (int attempt = 0; attempt < 2; attempt++) {
      if (!registerMode(ptw, pth, 0)) return @"err applySettings rejected";
      if (waitOneX(did, pw, ph, 3.0, cur)) return okReply(did, cur);
    }
    CGDirectDisplayID ids[32];
    uint32_t n = 0;
    CGGetOnlineDisplayList(32, ids, &n);
    CGRect b = CGDisplayBounds(did);
    return [NSString stringWithFormat:@"err %ux%u did not become current (current %ux%u@%ux%u; "
                                      @"id %u online=%d active=%d bounds %.0fx%.0f; %u online)",
                                      ptw, pth, cur[0], cur[1], cur[2], cur[3], did,
                                      CGDisplayIsOnline(did), CGDisplayIsActive(did),
                                      b.size.width, b.size.height, n];
  }

  if (currentIs(did, ptw, pw, cur)) return okReply(did, cur);
  if (!registerMode(ptw, pth, 1)) return @"err applySettings rejected";
  if (waitCurrent(did, ptw, pw, 2.0, cur)) return okReply(did, cur);
  CGDisplayModeRef want = NULL;
  for (int i = 0; i < 100 && !want; i++) {
    want = findMode(did, ptw, pth, pw);
    if (!want) pump();
  }
  if (!want) return [NSString stringWithFormat:@"err %ux%u@2x not published", ptw, pth];
  fprintf(stderr, "Retina %ux%u was not WindowServer's default; selecting it\n", ptw, pth);
  CGDisplayConfigRef cfg = NULL;
  CGError e = CGBeginDisplayConfiguration(&cfg);
  if (e == kCGErrorSuccess) {
    CGConfigureDisplayWithDisplayMode(cfg, did, want, NULL);
    e = CGCompleteDisplayConfiguration(cfg, kCGConfigureForAppOnly);
  }
  CGDisplayModeRelease(want);
  gPinned = 1;
  if (e != kCGErrorSuccess) return @"err needs-replace";
  if (waitCurrent(did, ptw, pw, 3.0, cur)) return okReply(did, cur);
  return @"err needs-replace";
}

static void lockAll(void) {
  if (gLockFd < 0) {
    char dir[1024];
    size_t n = confstr(_CS_DARWIN_USER_TEMP_DIR, dir, sizeof dir);
    NSString *base = (n > 0 && n <= sizeof dir) ? @(dir) : NSTemporaryDirectory();
    NSString *path = [base stringByAppendingPathComponent:@"portico-displayhost.lock"];
    gLockFd = open(path.fileSystemRepresentation, O_CREAT | O_RDWR | O_CLOEXEC, 0600);
  }
  if (gLockFd >= 0) flock(gLockFd, LOCK_EX);
}

static void unlockAll(void) {
  if (gLockFd >= 0) flock(gLockFd, LOCK_UN);
}

// Every online display's id, bounds and current mode.
// (Do NOT use CGDisplayRegisterReconfigurationCallback: registering it stops this process's
// own virtual display from ever coming online.)
static NSString *displaySignature(void) {
  CGDirectDisplayID ids[32];
  uint32_t n = 0;
  CGGetOnlineDisplayList(32, ids, &n);
  NSMutableString *sig = [NSMutableString string];
  for (uint32_t i = 0; i < n; i++) {
    CGRect b = CGDisplayBounds(ids[i]);
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(ids[i]);
    [sig appendFormat:@"%u:%.0f,%.0f,%.0f,%.0f:%zu,%zu;", ids[i], b.origin.x, b.origin.y,
                      b.size.width, b.size.height, m ? CGDisplayModeGetPixelWidth(m) : 0,
                      m ? CGDisplayModeGetPixelHeight(m) : 0];
    if (m) CGDisplayModeRelease(m);
  }
  return sig;
}

// Wait until the display configuration has been unchanged for 250 ms (cap 5 s).
static void settle(void) {
  CFAbsoluteTime start = CFAbsoluteTimeGetCurrent(), stableSince = start;
  NSString *last = displaySignature();
  while (CFAbsoluteTimeGetCurrent() - start < 5.0) {
    pump();
    NSString *now = displaySignature();
    if (![now isEqualToString:last]) {
      last = now;
      stableSince = CFAbsoluteTimeGetCurrent();
    } else if (CFAbsoluteTimeGetCurrent() - stableSince >= 0.25) {
      return;
    }
  }
  fprintf(stderr, "settle: still reconfiguring after 5 s\n");
}

static int identityOnline(unsigned serial) {
  CGDirectDisplayID ids[32];
  uint32_t n = 0;
  CGGetOnlineDisplayList(32, ids, &n);
  for (uint32_t i = 0; i < n; i++)
    if (CGDisplayVendorNumber(ids[i]) == kVendor && CGDisplayModelNumber(ids[i]) == kProduct &&
        CGDisplaySerialNumber(ids[i]) == serial)
      return 1;
  return 0;
}

static NSString *create(unsigned serial, unsigned pw, unsigned ph, unsigned scale) {
  if (gDisplay) return @"err display already exists (one per process)";
  CGVirtualDisplayDescriptor *d = [[CGVirtualDisplayDescriptor alloc] init];
  // Without a queue, display updates target the main run loop, which only `pump` drives.
  d.queue = dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0);
  d.name = gName;
  d.maxPixelsWide = 8192;
  d.maxPixelsHigh = 8192;
  d.sizeInMillimeters = CGSizeMake(600, 338);
  d.vendorID = kVendor;
  d.productID = kProduct;
  d.serialNum = serial;
  gSerial = serial;
  // sRGB primaries + D65 white: macOS colour-manages every app into sRGB for this display.
  d.redPrimary = CGPointMake(0.64, 0.33);
  d.greenPrimary = CGPointMake(0.30, 0.60);
  d.bluePrimary = CGPointMake(0.15, 0.06);
  d.whitePoint = CGPointMake(0.3127, 0.3290);
  // A replaced host's display leaves asynchronously; the same identity is rejected until then.
  int waited = 0;
  for (; waited < 250 && identityOnline(serial); waited++) pump();
  if (waited) fprintf(stderr, "waited %d ms for the previous display to leave\n", waited * 20);
  for (int i = 0; i < 2 && !gDisplay; i++) {
    gDisplay = [[CGVirtualDisplay alloc] initWithDescriptor:d];
    if (!gDisplay) usleep(500000);
  }
  if (!gDisplay) return @"err descriptor rejected";
  usleep(300000);
  return commit(pw, ph, scale);
}

static NSString *locked(NSString * (^op)(void)) {
  lockAll();
  NSString *r = op();
  settle();
  unlockAll();
  return r;
}

static void destroy(void) {
  if (!gDisplay) return;
  lockAll();
  gDisplay = nil;
  for (int i = 0; i < 250 && identityOnline(gSerial); i++) pump();
  settle();
  unlockAll();
}

int main(int argc, char **argv) {
  @autoreleasepool {
    // If Portico dies mid-reply, finish the command and still remove the display under the lock.
    signal(SIGPIPE, SIG_IGN);
    if (argc > 1) gName = @(argv[1]);
    char line[256];
    while (fgets(line, sizeof line, stdin)) {
      unsigned a, b, c, d;
      if (sscanf(line, "create %u %u %u %u", &a, &b, &c, &d) == 4)
        reply(locked(^{ return create(a, b, c, d); }));
      else if (sscanf(line, "mode %u %u %u", &a, &b, &c) == 3)
        reply(gDisplay ? locked(^{ return commit(a, b, c); }) : @"err no display");
      else if (!strncmp(line, "quit", 4))
        break;
      else
        reply(@"err unknown command");
    }
    destroy();
  }
  return 0;
}
