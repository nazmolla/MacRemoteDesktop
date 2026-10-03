// Lists the BL/B call targets inside an Objective-C method or C function, resolved with dladdr.
// usage: calls <Class> <selector> [bytes]   |   calls - <cfunction> [bytes]
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <dlfcn.h>
#include <ptrauth.h>
int main(int c, char **v) {
  dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
  dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_NOW);
  void *f;
  if (strcmp(v[1], "-") == 0) f = dlsym(RTLD_DEFAULT, v[2]);
  else { Class k = objc_getClass(v[1]); Method m = class_getInstanceMethod(k, sel_registerName(v[2])); if (!m) m = class_getClassMethod(k, sel_registerName(v[2])); f = m ? (void *)method_getImplementation(m) : 0; }
  if (!f) { printf("not found\n"); return 1; }
  uint32_t *p = ptrauth_strip(f, ptrauth_key_asia);
  int n = c > 3 ? atoi(v[3]) / 4 : 400;
  for (int i = 0; i < n; i++) {
    uint32_t ins = p[i];
    if ((ins & 0x7C000000) == 0x14000000) { // B / BL
      int32_t off = (int32_t)((ins & 0x03FFFFFF) << 6) >> 6;
      void *t = (char *)&p[i] + off * 4; Dl_info di;
      if (dladdr(t, &di) && di.dli_sname) printf("%4d %s %s\n", i * 4, (ins & 0x80000000) ? "bl" : "b ", di.dli_sname);
      if (!(ins & 0x80000000) && i > 4) {}
    }
    if (ins == 0xd65f03c0 || ins == 0xd65f0fff) printf("%4d ret\n", i * 4);
  }
}
