// Spike: create ONE off-console login-window session the way screensharingd's
// CreateOffConsoleLoginWindowSession does, observe it, then release it.
// Signature and data recovered from screensharingd (macOS 27, arm64e):
//   err = CGSCreateLoginSessionWithDataAndVisibility(bytes, len, 0, &session, NULL)
//   bytes = binary plist of {"SessionStartedBy": <caller>}
// Must run as root. Usage: loginsession [hold-seconds]
#include <CoreFoundation/CoreFoundation.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

extern int CGSCreateLoginSessionWithDataAndVisibility(const void *data, size_t len, int visibility,
                                                      int *outSession, void *unused);
extern int CGSReleaseSession(int session);

int main(int argc, char **argv) {
    int hold = argc > 1 ? atoi(argv[1]) : 15;
    if (geteuid() != 0) { fprintf(stderr, "must run as root\n"); return 2; }

    CFMutableDictionaryRef d = CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks,
                                                         &kCFTypeDictionaryValueCallBacks);
    CFDictionaryAddValue(d, CFSTR("SessionStartedBy"), CFSTR("ScreenSharing"));
    CFErrorRef perr = NULL;
    CFDataRef plist = CFPropertyListCreateData(NULL, d, kCFPropertyListBinaryFormat_v1_0, 0, &perr);
    if (!plist) { fprintf(stderr, "plist encode failed\n"); return 1; }

    int session = -1;
    int err = CGSCreateLoginSessionWithDataAndVisibility(CFDataGetBytePtr(plist), (size_t)CFDataGetLength(plist),
                                                         0, &session, NULL);
    printf("create: err=%d session=%d\n", err, session);
    fflush(stdout);
    if (err != 0) return 1;

    system("echo '-- loginwindow processes:'; pgrep -lf loginwindow; echo '-- who:'; who");
    sleep((unsigned)hold);
    int rerr = CGSReleaseSession(session);
    printf("release: err=%d\n", rerr);
    return 0;
}
