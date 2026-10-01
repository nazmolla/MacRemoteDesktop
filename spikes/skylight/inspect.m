// Throwaway: list SkyLight / CoreGraphics Obj-C classes and methods related to virtual displays.
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#import <dlfcn.h>
int main(void) {
  dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
  dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_NOW);
  unsigned n = 0; Class *cls = objc_copyClassList(&n);
  for (unsigned i = 0; i < n; i++) {
    const char *name = class_getName(cls[i]);
    if (!strcasestr(name, "VirtualDisplay")) continue;
    const char *img = class_getImageName(cls[i]);
    printf("== %s  [%s]\n", name, img ? strrchr(img, '/') + 1 : "?");
    unsigned mc = 0; Method *ms = class_copyMethodList(cls[i], &mc);
    for (unsigned j = 0; j < mc; j++) printf("   - %s  %s\n", sel_getName(method_getName(ms[j])), method_getTypeEncoding(ms[j]));
    free(ms);
    unsigned pc = 0; objc_property_t *ps = class_copyPropertyList(cls[i], &pc);
    for (unsigned j = 0; j < pc; j++) printf("   @ %s  %s\n", property_getName(ps[j]), property_getAttributes(ps[j]));
    free(ps);
  }
  free(cls);
  return 0;
}
