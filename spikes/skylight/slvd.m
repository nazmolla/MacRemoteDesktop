// Probe: create an SLVirtualDisplay (SkyLight) and report whether it activates.
// usage: slvd <type> <options> <subtype> [seconds]
#import <Foundation/Foundation.h>
#import <CoreGraphics/CoreGraphics.h>
#include <dlfcn.h>
#include <objc/message.h>
typedef struct { float x, y; } F2; typedef struct { unsigned w, h; } U2;
typedef struct { F2 r, g, b, w; } Chroma;
int main(int c, char **v) {
  setvbuf(stdout, NULL, _IONBF, 0); fprintf(stdout, "start uid=%d\n", getuid());
  @autoreleasepool {
    dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
    unsigned long long type = c > 1 ? strtoull(v[1],0,0) : 0, opts = c > 2 ? strtoull(v[2],0,0) : 0, sub = c > 3 ? strtoull(v[3],0,0) : 0;
    int secs = c > 4 ? atoi(v[4]) : 10;
    printf("capabilities=%s\n", [[[NSClassFromString(@"SLVirtualDisplay") performSelector:@selector(capabilities)] description] UTF8String]);
    Chroma ch = {{0.64,0.33},{0.30,0.60},{0.15,0.06},{0.3127,0.3290}};
    NSError *e = nil; printf("stage: config\n");
    id cfg = [NSClassFromString(@"SLVirtualDisplayConfiguration") alloc];
    SEL s = NSSelectorFromString(@"initWithName:vendorID:productID:serialNumber:sizeInMillimeters:maximumSizeInPixels:chromaticities:error:");
    id (*initc)(id, SEL, id, unsigned long long, unsigned long long, unsigned long long, F2, U2, Chroma, NSError **) = (void *)objc_msgSend;
    cfg = initc(cfg, s, @"Viga probe", 0x7667, 0x1, getpid(), (F2){600, 340}, (U2){3840, 2160}, ch, &e);
    if (!cfg) { printf("config err %s\n", e.description.UTF8String); return 1; }
    ((void (*)(id, SEL, unsigned long long))objc_msgSend)(cfg, @selector(setType:), type);
    ((void (*)(id, SEL, unsigned long long))objc_msgSend)(cfg, @selector(setOptions:), opts);
    ((void (*)(id, SEL, unsigned long long))objc_msgSend)(cfg, @selector(setSubtype:), sub);
    printf("config=%s\n", [[cfg performSelector:@selector(dictionaryRepresentation)] description].UTF8String);
    printf("stage: create\n"); id d = ((id (*)(id, SEL, id, NSError **))objc_msgSend)([NSClassFromString(@"SLVirtualDisplay") alloc], @selector(initWithConfiguration:error:), cfg, &e);
    if (!d) { printf("display err %s\n", e.description.UTF8String); return 1; }
    printf("stage: mode\n"); id mode = ((id (*)(id, SEL, U2, U2, float, NSError **))objc_msgSend)([NSClassFromString(@"SLVirtualDisplayMode") alloc], @selector(initWithSizeInPixels:sizeInPoints:refreshRate:error:), (U2){1920,1080}, (U2){1920,1080}, 60.0f, &e);
    id st = ((id (*)(id, SEL, id, id, id, unsigned long long, NSError **))objc_msgSend)([NSClassFromString(@"SLVirtualDisplaySettings") alloc], @selector(initWithNativeMode:preferredMode:optionalModes:rotations:error:), mode, mode, @[], 0ULL, &e);
    BOOL ok = ((BOOL (*)(id, SEL, id, NSError **))objc_msgSend)(d, @selector(applySettings:error:), st, &e);
    unsigned id_ = ((unsigned (*)(id, SEL))objc_msgSend)(d, @selector(displayID));
    printf("displayID=%u apply=%d err=%s\n", id_, ok, e ? e.description.UTF8String : "-");
    for (int i = 0; i < secs * 2; i++) {
      CGRect b = CGDisplayBounds(id_);
      if (b.size.width > 0) { printf("ACTIVE after %.1fs bounds=%.0fx%.0f@%.0f,%.0f online=%d\n", i/2.0, b.size.width, b.size.height, b.origin.x, b.origin.y, CGDisplayIsOnline(id_)); break; }
      [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.5]];
      if (i == secs*2-1) printf("NOT ACTIVE after %ds\n", secs);
    }
    ((void (*)(id, SEL))objc_msgSend)(d, @selector(destroy));
  }
}
