// Spike: one virtual display per process, driven over a line protocol.
//   create <serial> <pixW> <pixH> <scale 1|2>  -> ok <id> <ptW> <ptH> <pixW> <pixH> | err <msg>
//   mode <pixW> <pixH> <scale>                 -> same
//   quit                                        -> destroys the display and exits
// Every create / mode / destroy runs under one machine-wide lock and holds it until
// WindowServer has stopped reconfiguring displays: overlapping virtual-display hotplugs
// abort WindowServer (SkyLight GenerateModeListForDisplay), logging out every session.
// Rules proven in spikes/hidpi + spikes/skylight: fresh process per display, a global
// dispatch queue, hiDPI=1, explicit app-scoped mode selection verified against the
// active mode, and never CGRestorePermanentDisplayConfiguration.
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#include <fcntl.h>
#include <sys/file.h>

@interface CGVirtualDisplayDescriptor : NSObject
@property (retain, nonatomic) dispatch_queue_t queue;
@property (retain, nonatomic) NSString *name;
@property (nonatomic) unsigned int maxPixelsWide, maxPixelsHigh, productID, vendorID, serialNum;
@property (nonatomic) CGSize sizeInMillimeters;
@property (nonatomic) CGPoint redPrimary, greenPrimary, bluePrimary, whitePoint;
@end
@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(unsigned int)w height:(unsigned int)h refreshRate:(double)r;
@end
@interface CGVirtualDisplaySettings : NSObject
@property (retain, nonatomic) NSArray *modes;
@property (nonatomic) unsigned int hiDPI;
@end
@interface CGVirtualDisplay : NSObject
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)d;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)s;
@property (readonly, nonatomic) unsigned int displayID;
@end

static CGVirtualDisplay *gDisplay;
static unsigned gVendor, gSerial;
static int gLockFd = -1;


static void reply(NSString *s) { printf("%s\n", s.UTF8String); fflush(stdout); }

static BOOL modeIs(CGDirectDisplayID did, unsigned ptw, unsigned pw, unsigned *out) {
  CGDisplayModeRef m = CGDisplayCopyDisplayMode(did);
  if (!m) return NO;
  out[0] = (unsigned)CGDisplayModeGetWidth(m); out[1] = (unsigned)CGDisplayModeGetHeight(m);
  out[2] = (unsigned)CGDisplayModeGetPixelWidth(m); out[3] = (unsigned)CGDisplayModeGetPixelHeight(m);
  CGDisplayModeRelease(m);
  return out[0] == ptw && out[2] == pw;
}

// Explicitly selecting a mode freezes this display's mode list for the life of the host
// (later applySettings publish nothing new), so a host selects at most once; after that
// every re-mode is answered "err needs-replace" and the supervisor starts a fresh host.
static int gPinned = 0;

// Let CoreGraphics process display-change notifications between polls.
static void pump(void) { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, false); }

static CGDisplayModeRef findMode(CGDirectDisplayID did, unsigned ptw, unsigned pth, unsigned pw, int *isDefault) {
  NSDictionary *o = @{(__bridge id)kCGDisplayShowDuplicateLowResolutionModes: @YES};
  NSArray *ms = CFBridgingRelease(CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)o));
  for (id m in ms) {
    CGDisplayModeRef r = (__bridge CGDisplayModeRef)m;
    if (CGDisplayModeGetWidth(r) == ptw && CGDisplayModeGetHeight(r) == pth && CGDisplayModeGetPixelWidth(r) == pw
        && CGDisplayModeIsUsableForDesktopGUI(r)) {
      *isDefault = (CGDisplayModeGetIOFlags(r) & 0x4) != 0;  // kDisplayModeDefaultFlag
      return CGDisplayModeRetain(r);
    }
  }
  return NULL;
}

// Register the point-size mode; keep WindowServer's default when it is the wanted variant,
// otherwise select it explicitly (once per host). Always verify the current mode.
static NSString *commit(unsigned pw, unsigned ph, unsigned scale) {
  if (scale != 1 && scale != 2) return @"err scale must be 1 or 2";
  if ((pw % scale) || (ph % scale)) return @"err pixel size not divisible by scale";
  if (gPinned) return @"err needs-replace";
  unsigned ptw = pw / scale, pth = ph / scale;
  CGVirtualDisplaySettings *s = [[CGVirtualDisplaySettings alloc] init];
  s.hiDPI = scale == 2 ? 1 : 0;
  s.modes = @[[[CGVirtualDisplayMode alloc] initWithWidth:ptw height:pth refreshRate:60]];
  if (![gDisplay applySettings:s]) return @"err applySettings rejected";
  CGDirectDisplayID did = gDisplay.displayID;
  unsigned cur[4] = {0};
  CGDisplayModeRef want = NULL; int isDefault = 0;
  for (int i = 0; i < 100 && !want; i++) { want = findMode(did, ptw, pth, pw, &isDefault); if (!want) pump(); }
  if (!want) { modeIs(did, ptw, pw, cur); return [NSString stringWithFormat:@"err %ux%u@%ux not published (current %ux%u@%ux%u)", ptw, pth, scale, cur[0], cur[1], cur[2], cur[3]]; }
  // Default first; if WindowServer lands elsewhere (it can remember an earlier choice for
  // this identity), select explicitly — once per host, since selection freezes the list.
  if (isDefault && !getenv("DH_SELECT_ALL")) {
    for (int i = 0; i < 100; i++) {
      if (modeIs(did, ptw, pw, cur)) { CGDisplayModeRelease(want); fprintf(stderr, "how=default\n"); return [NSString stringWithFormat:@"ok %u %u %u %u %u", did, cur[0], cur[1], cur[2], cur[3]]; }
      pump();
    }
    fprintf(stderr, "default %ux%u@%ux did not land (current %ux%u@%ux%u); selecting\n", ptw, pth, scale, cur[0], cur[1], cur[2], cur[3]);
  }
  CGDisplayConfigRef cfg = NULL;
  CGBeginDisplayConfiguration(&cfg);
  CGConfigureDisplayWithDisplayMode(cfg, did, want, NULL);
  CGError e = CGCompleteDisplayConfiguration(cfg, kCGConfigureForAppOnly);
  CGDisplayModeRelease(want);
  gPinned = 1;
  if (e != kCGErrorSuccess) return @"err needs-replace";
  for (int i = 0; i < 150; i++) {
    if (modeIs(did, ptw, pw, cur)) { fprintf(stderr, "how=selected\n"); return [NSString stringWithFormat:@"ok %u %u %u %u %u", did, cur[0], cur[1], cur[2], cur[3]]; }
    pump();
  }
  return @"err needs-replace";
}

static void dump(CGDirectDisplayID did) {
  NSDictionary *o = @{(__bridge id)kCGDisplayShowDuplicateLowResolutionModes: @YES};
  NSArray *ms = CFBridgingRelease(CGDisplayCopyAllDisplayModes(did, (__bridge CFDictionaryRef)o));
  for (id m in ms) { CGDisplayModeRef r = (__bridge CGDisplayModeRef)m;
    fprintf(stderr, "  mode %zux%zu px %zux%zu flags=0x%x usable=%d\n", CGDisplayModeGetWidth(r), CGDisplayModeGetHeight(r), CGDisplayModeGetPixelWidth(r), CGDisplayModeGetPixelHeight(r), CGDisplayModeGetIOFlags(r), CGDisplayModeIsUsableForDesktopGUI(r)); }
}

// A replaced host's display leaves WindowServer asynchronously after the old process dies;
// creating the same identity before it is gone is rejected. Wait (<=5 s) until it is gone.
static void lockAll(void) {
  if (gLockFd < 0) gLockFd = open("/tmp/macrdp-displayhost.lock", O_CREAT | O_RDWR, 0666);
  flock(gLockFd, LOCK_EX);
}
static void unlockAll(void) { flock(gLockFd, LOCK_UN); }

// Snapshot of every online display's id, bounds and current mode.
// (Do NOT use CGDisplayRegisterReconfigurationCallback here: registering it stops this
// process's own virtual display from ever coming online.)
static NSString *displaySignature(void) {
  CGDirectDisplayID ids[32]; uint32_t n = 0;
  CGGetOnlineDisplayList(32, ids, &n);
  NSMutableString *sig = [NSMutableString string];
  for (uint32_t i = 0; i < n; i++) {
    CGRect b = CGDisplayBounds(ids[i]);
    CGDisplayModeRef m = CGDisplayCopyDisplayMode(ids[i]);
    [sig appendFormat:@"%u:%.0f,%.0f,%.0f,%.0f:%zu,%zu;", ids[i], b.origin.x, b.origin.y, b.size.width, b.size.height,
         m ? CGDisplayModeGetPixelWidth(m) : 0, m ? CGDisplayModeGetPixelHeight(m) : 0];
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
    if (![now isEqualToString:last]) { last = now; stableSince = CFAbsoluteTimeGetCurrent(); }
    else if (CFAbsoluteTimeGetCurrent() - stableSince >= 0.25) return;
  }
  fprintf(stderr, "settle: still reconfiguring after 5 s\n");
}

static int identityOnline(unsigned v, unsigned serial) {
  CGDirectDisplayID ids[32]; uint32_t n = 0;
  CGGetOnlineDisplayList(32, ids, &n);
  for (uint32_t i = 0; i < n; i++)
    if (CGDisplayVendorNumber(ids[i]) == v && CGDisplayModelNumber(ids[i]) == v && CGDisplaySerialNumber(ids[i]) == serial) return 1;
  return 0;
}

static NSString *createLocked(unsigned serial, unsigned pw, unsigned ph, unsigned scale) {
  if (gDisplay) return @"err display already exists (one per process)";
  CGVirtualDisplayDescriptor *d = [[CGVirtualDisplayDescriptor alloc] init];
  d.queue = dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0);
  d.name = @"MacRemoteDesktop";
  d.maxPixelsWide = 8192; d.maxPixelsHigh = 8192;
  d.sizeInMillimeters = CGSizeMake(getenv("DH_MMW") ? atof(getenv("DH_MMW")) : 600, getenv("DH_MMH") ? atof(getenv("DH_MMH")) : 338);
  const char *vp = getenv("DH_VENDOR"); unsigned v = vp ? (unsigned)strtoul(vp, NULL, 0) : 0x6D616372; d.vendorID = v; d.productID = v; d.serialNum = serial;
  gVendor = v; gSerial = serial;
  d.redPrimary = CGPointMake(0.64, 0.33); d.greenPrimary = CGPointMake(0.30, 0.60);
  d.bluePrimary = CGPointMake(0.15, 0.06); d.whitePoint = CGPointMake(0.3127, 0.3290);
  int waited = 0;
  for (; waited < 250 && identityOnline(v, serial); waited++) pump();
  if (waited) fprintf(stderr, "waited %d ms for previous identity to leave\n", waited * 20);
  for (int i = 0; i < 2 && !gDisplay; i++) {
    gDisplay = [[CGVirtualDisplay alloc] initWithDescriptor:d];
    if (!gDisplay) usleep(500000);
  }
  if (!gDisplay) return @"err descriptor rejected";
  usleep(300000);
  CGDirectDisplayID did = gDisplay.displayID;
  fprintf(stderr, "mirror: inSet=%d mirrorsDisplay=%u primaryInSet=%u main=%u\n", CGDisplayIsInMirrorSet(did), CGDisplayMirrorsDisplay(did), CGDisplayPrimaryDisplay(did), CGMainDisplayID());
  return commit(pw, ph, scale);
}

static NSString *locked(NSString *(^op)(void)) {
  lockAll(); NSString *r = op(); settle(); unlockAll(); return r;
}

static void destroy(void) {
  if (!gDisplay) return;
  lockAll();
  gDisplay = nil;
  for (int i = 0; i < 250 && identityOnline(gVendor, gSerial); i++) pump();
  settle();
  unlockAll();
}

int main(void) {
  @autoreleasepool {
    char line[256];
    while (fgets(line, sizeof line, stdin)) {
      unsigned a, b, c, d;
      if (sscanf(line, "create %u %u %u %u", &a, &b, &c, &d) == 4) reply(locked(^{ return createLocked(a, b, c, d); }));
      else if (sscanf(line, "mode %u %u %u", &a, &b, &c) == 3) reply(gDisplay ? locked(^{ return commit(a, b, c); }) : @"err no display");
      else if (!strncmp(line, "dump", 4)) { if (gDisplay) dump(gDisplay.displayID); reply(@"ok"); }
      else if (!strncmp(line, "quit", 4)) break;
      else reply(@"err unknown command");
    }
    destroy();
  }
  return 0;
}
