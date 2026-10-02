// Spike: create a background session logged in as a user, the way
// screensharingd's LoginUser does: binary plist of
// {username, UserPasswordKey, SessionStartedBy} passed to
// CGSCreateLoginSessionWithDataAndVisibility. The password is read from a file
// and never printed. Must run as root.
// Usage: loginuser <user> <password-file> <visibility> [hold-seconds]
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern int CGSCreateLoginSessionWithDataAndVisibility(const void *data, size_t len, int visibility,
                                                      int *outSession, void *unused);
extern int CGSReleaseSession(int session);

static void console_owner(const char *when) {
    char cmd[128];
    snprintf(cmd, sizeof cmd, "printf '%s console=' ; stat -f %%Su /dev/console", when);
    system(cmd);
}

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: loginuser <user> <password-file> <visibility> [hold]\n"); return 2; }
    if (geteuid() != 0) { fprintf(stderr, "must run as root\n"); return 2; }
    const char *user = argv[1];
    int visibility = atoi(argv[3]);
    int hold = argc > 4 ? atoi(argv[4]) : 20;

    char pw[512] = {0};
    FILE *f = fopen(argv[2], "r");
    if (!f || !fgets(pw, sizeof pw, f)) { fprintf(stderr, "cannot read password file\n"); return 2; }
    fclose(f);
    pw[strcspn(pw, "\r\n")] = 0;
    if (!pw[0]) { fprintf(stderr, "password file is empty\n"); return 2; }

    CFMutableDictionaryRef d = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks,
                                                         &kCFTypeDictionaryValueCallBacks);
    CFStringRef u = CFStringCreateWithCString(NULL, user, kCFStringEncodingUTF8);
    CFStringRef p = CFStringCreateWithCString(NULL, pw, kCFStringEncodingUTF8);
    memset(pw, 0, sizeof pw);
    CFDictionaryAddValue(d, CFSTR("username"), u);
    CFDictionaryAddValue(d, CFSTR("UserPasswordKey"), p);
    CFDictionaryAddValue(d, CFSTR("SessionStartedBy"), CFSTR("ScreenSharing"));
    CFDataRef plist = CFPropertyListCreateData(NULL, d, kCFPropertyListBinaryFormat_v1_0, 0, NULL);
    CFRelease(p);
    if (!plist) { fprintf(stderr, "plist encode failed\n"); return 1; }

    console_owner("before");
    int session = -1;
    int err = CGSCreateLoginSessionWithDataAndVisibility(CFDataGetBytePtr(plist), (size_t)CFDataGetLength(plist),
                                                         visibility, &session, NULL);
    CFRelease(plist);
    printf("create: visibility=%d err=%d session=%d\n", visibility, err, session);
    fflush(stdout);
    if (err != 0) return 1;
    sleep(3);
    console_owner("after-create");
    system("ps -A -o user,command | grep '[l]oginwindow' | cut -c1-40");
    sleep((unsigned)hold);
    printf("release: err=%d\n", CGSReleaseSession(session));
    sleep(3);
    console_owner("after-release");
    return 0;
}
