#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <dlfcn.h>
int main(int c, char**v){ dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
 for(int i=1;i<c;i++){ Class k=objc_getClass(v[i]); printf("== %s super=%s\n",v[i],k?class_getName(class_getSuperclass(k)):"-"); if(!k)continue;
  for(int m=0;m<2;m++){unsigned n; Method*ms=class_copyMethodList(m?object_getClass(k):k,&n); for(unsigned j=0;j<n;j++) printf("  %c %s %s\n",m?'+':'-',sel_getName(method_getName(ms[j])),method_getTypeEncoding(ms[j]));}
  unsigned n; objc_property_t*ps=class_copyPropertyList(k,&n); for(unsigned j=0;j<n;j++) printf("  @ %s %s\n",property_getName(ps[j]),property_getAttributes(ps[j]));}}
