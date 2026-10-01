#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#import <dlfcn.h>
static void dump(const char *cn, BOOL all) {
  Class c = objc_getClass(cn); if (!c) { printf("!! %s missing\n", cn); return; }
  printf("== %s : %s\n", cn, class_getName(class_getSuperclass(c)));
  unsigned n = 0; Method *m = class_copyMethodList(object_getClass(c), &n);
  for (unsigned i = 0; i < n; i++) printf("   + %s\n", sel_getName(method_getName(m[i]))); free(m);
  m = class_copyMethodList(c, &n);
  for (unsigned i = 0; i < n; i++) { const char *s = sel_getName(method_getName(m[i])); if (all || !strstr(s, "cxx")) printf("   - %s\n", s); } free(m);
}
static void proto(const char *pn) {
  Protocol *p = objc_getProtocol(pn); if (!p) { printf("!! protocol %s missing\n", pn); return; }
  printf("== <%s>\n", pn);
  for (int req = 0; req < 2; req++) { unsigned n = 0; struct objc_method_description *d = protocol_copyMethodDescriptionList(p, req, YES, &n);
    for (unsigned i = 0; i < n; i++) printf("   %s %s  %s\n", req ? "req" : "opt", sel_getName(d[i].name), d[i].types); free(d); }
}
int main(void) {
  dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
  dlopen("/System/Library/Frameworks/QuartzCore.framework/QuartzCore", RTLD_NOW);
  dump("SLVirtualDisplayMode", YES); dump("SLVirtualDisplay", NO); dump("SLVirtualDisplayCapabilities", NO);
  proto("SLVirtualDisplayDelegate");
  Class c = objc_getClass("CAWindowServerVirtualDisplay");
  for (; c; c = class_getSuperclass(c)) { if (!strcmp(class_getName(c), "NSObject")) break; dump(class_getName(c), NO); }
  return 0;
}
