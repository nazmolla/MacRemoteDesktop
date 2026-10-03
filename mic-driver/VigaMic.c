// VigaMic: a CoreAudio AudioServerPlugIn exposing one input-only device,
// "Viga Microphone". It plays the RDP client's microphone, which Viga writes to
// a ring file (RING_PATH): a header followed by interleaved int16 frames. With
// no writer, or when the writer stops, it delivers silence.
//
// Structure follows Apple's NullAudio sample (AudioServerPlugIn.h). Objects:
// the plug-in (kAudioObjectPlugInObject), one device, one input stream.
#include <CoreAudio/AudioServerPlugIn.h>
#include <fcntl.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdatomic.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#define kDevice_UID "VigaMic_Device"
#define kDevice_ModelUID "VigaMic_Model"
#define RING_PATH "/Users/Shared/Viga/mic.ring"
#define RING_MAGIC 0x5647414Du  // "VGAM"
#define kRate 44100.0
#define kChannels 2
#define kZeroTimeStampPeriod 16384

enum { kObjectID_PlugIn = kAudioObjectPlugInObject, kObjectID_Device = 2, kObjectID_Stream = 3 };

typedef struct {
    uint32_t magic, rate, channels, capacity;  // capacity in frames
    _Atomic uint64_t write_pos;                // total frames written
} RingHeader;

static AudioServerPlugInHostRef gHost;
static pthread_mutex_t gLock = PTHREAD_MUTEX_INITIALIZER;
static UInt32 gRefCount = 0, gIOCount = 0;
static Float64 gHostTicksPerFrame;
static UInt64 gAnchorHostTime, gTimeStampCount;
static const RingHeader *gRing;
static size_t gRingSize;
static uint64_t gReadPos;
static int gReadPosValid;

static void ring_open(void) {
    if (gRing) return;
    int fd = open(RING_PATH, O_RDONLY);
    if (fd < 0) return;
    struct stat st;
    if (fstat(fd, &st) == 0 && st.st_size >= (off_t)sizeof(RingHeader)) {
        void *p = mmap(NULL, (size_t)st.st_size, PROT_READ, MAP_SHARED, fd, 0);
        if (p != MAP_FAILED) {
            const RingHeader *h = p;
            size_t need = sizeof(RingHeader) + (size_t)h->capacity * h->channels * 2;
            if (h->magic == RING_MAGIC && h->channels == kChannels && (size_t)st.st_size >= need) {
                gRing = h;
                gRingSize = (size_t)st.st_size;
            } else {
                munmap(p, (size_t)st.st_size);
            }
        }
    }
    close(fd);
}

static void ring_close(void) {
    if (gRing) munmap((void *)gRing, gRingSize);
    gRing = NULL;
    gReadPosValid = 0;
}

// Fill `frames` stereo float frames from the ring, or silence.
static void ring_read(Float32 *out, UInt32 frames) {
    memset(out, 0, (size_t)frames * kChannels * sizeof(Float32));
    if (!gRing) ring_open();
    if (!gRing) return;
    uint64_t w = atomic_load_explicit(&gRing->write_pos, memory_order_acquire);
    uint32_t cap = gRing->capacity;
    // Start (or resync) about 60 ms behind the writer.
    uint64_t lag = (uint64_t)(kRate * 0.06);
    if (!gReadPosValid || gReadPos > w || w - gReadPos > cap / 2) {
        gReadPos = w > lag ? w - lag : 0;
        gReadPosValid = 1;
    }
    const int16_t *pcm = (const int16_t *)(gRing + 1);
    for (UInt32 i = 0; i < frames && gReadPos < w; i++, gReadPos++) {
        const int16_t *f = pcm + (size_t)(gReadPos % cap) * kChannels;
        out[i * 2] = f[0] / 32768.0f;
        out[i * 2 + 1] = f[1] / 32768.0f;
    }
}

// ---- property helpers ------------------------------------------------------

static CFStringRef str(const char *s) { return CFStringCreateWithCString(NULL, s, kCFStringEncodingUTF8); }

static AudioStreamBasicDescription format(void) {
    AudioStreamBasicDescription f = {0};
    f.mSampleRate = kRate;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked;
    f.mBytesPerPacket = f.mBytesPerFrame = kChannels * sizeof(Float32);
    f.mFramesPerPacket = 1;
    f.mChannelsPerFrame = kChannels;
    f.mBitsPerChannel = 32;
    return f;
}

#define PUT(type, value)                                                         \
    do {                                                                         \
        if (inDataSize < sizeof(type)) return kAudioHardwareBadPropertySizeError; \
        *(type *)outData = (value);                                              \
        *outDataSize = sizeof(type);                                             \
        return 0;                                                                \
    } while (0)

static Boolean has(AudioObjectID obj, const AudioObjectPropertyAddress *a) {
    switch (obj) {
    case kObjectID_PlugIn:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass: case kAudioObjectPropertyOwner:
        case kAudioObjectPropertyManufacturer: case kAudioObjectPropertyOwnedObjects:
        case kAudioPlugInPropertyDeviceList: case kAudioPlugInPropertyTranslateUIDToDevice:
        case kAudioPlugInPropertyResourceBundle:
            return true;
        }
        return false;
    case kObjectID_Device:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass: case kAudioObjectPropertyOwner:
        case kAudioObjectPropertyName: case kAudioObjectPropertyManufacturer: case kAudioObjectPropertyOwnedObjects:
        case kAudioDevicePropertyDeviceUID: case kAudioDevicePropertyModelUID: case kAudioDevicePropertyTransportType:
        case kAudioDevicePropertyRelatedDevices: case kAudioDevicePropertyClockDomain:
        case kAudioDevicePropertyDeviceIsAlive: case kAudioDevicePropertyDeviceIsRunning:
        case kAudioDevicePropertyDeviceCanBeDefaultDevice: case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
        case kAudioDevicePropertyLatency: case kAudioDevicePropertyStreams: case kAudioObjectPropertyControlList:
        case kAudioDevicePropertySafetyOffset: case kAudioDevicePropertyNominalSampleRate:
        case kAudioDevicePropertyAvailableNominalSampleRates: case kAudioDevicePropertyIsHidden:
        case kAudioDevicePropertyZeroTimeStampPeriod: case kAudioDevicePropertyPreferredChannelsForStereo:
            return true;
        }
        return false;
    case kObjectID_Stream:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: case kAudioObjectPropertyClass: case kAudioObjectPropertyOwner:
        case kAudioObjectPropertyOwnedObjects: case kAudioStreamPropertyIsActive: case kAudioStreamPropertyDirection:
        case kAudioStreamPropertyTerminalType: case kAudioStreamPropertyStartingChannel: case kAudioStreamPropertyLatency:
        case kAudioStreamPropertyVirtualFormat: case kAudioStreamPropertyPhysicalFormat:
        case kAudioStreamPropertyAvailableVirtualFormats: case kAudioStreamPropertyAvailablePhysicalFormats:
            return true;
        }
        return false;
    }
    return false;
}

// Returns the size in *outDataSize; writes data when outData is non-NULL.
static OSStatus get(AudioObjectID obj, const AudioObjectPropertyAddress *a, UInt32 inDataSize,
                    UInt32 *outDataSize, void *outData) {
    void *probe = outData;
    char scratch[256];
    if (!probe) { outData = scratch; inDataSize = sizeof(scratch); }
    OSStatus r = kAudioHardwareUnknownPropertyError;
    switch (obj) {
    case kObjectID_PlugIn:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: PUT(AudioClassID, kAudioObjectClassID);
        case kAudioObjectPropertyClass: PUT(AudioClassID, kAudioPlugInClassID);
        case kAudioObjectPropertyOwner: PUT(AudioObjectID, kAudioObjectUnknown);
        case kAudioObjectPropertyManufacturer: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str("Viga"));
        case kAudioObjectPropertyOwnedObjects: case kAudioPlugInPropertyDeviceList: PUT(AudioObjectID, kObjectID_Device);
        case kAudioPlugInPropertyTranslateUIDToDevice: PUT(AudioObjectID, kObjectID_Device);
        case kAudioPlugInPropertyResourceBundle: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str(""));
        }
        break;
    case kObjectID_Device:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: PUT(AudioClassID, kAudioObjectClassID);
        case kAudioObjectPropertyClass: PUT(AudioClassID, kAudioDeviceClassID);
        case kAudioObjectPropertyOwner: PUT(AudioObjectID, kObjectID_PlugIn);
        case kAudioObjectPropertyName: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str("Viga Microphone"));
        case kAudioObjectPropertyManufacturer: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str("Viga"));
        case kAudioDevicePropertyDeviceUID: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str(kDevice_UID));
        case kAudioDevicePropertyModelUID: if (!probe) { *outDataSize = sizeof(CFStringRef); return 0; } PUT(CFStringRef, str(kDevice_ModelUID));
        case kAudioDevicePropertyTransportType: PUT(UInt32, kAudioDeviceTransportTypeVirtual);
        case kAudioDevicePropertyRelatedDevices: PUT(AudioObjectID, kObjectID_Device);
        case kAudioDevicePropertyClockDomain: PUT(UInt32, 0);
        case kAudioDevicePropertyDeviceIsAlive: PUT(UInt32, 1);
        case kAudioDevicePropertyDeviceIsRunning: PUT(UInt32, gIOCount > 0);
        case kAudioDevicePropertyDeviceCanBeDefaultDevice: PUT(UInt32, a->mScope == kAudioObjectPropertyScopeInput);
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: PUT(UInt32, 0);
        case kAudioDevicePropertyIsHidden: PUT(UInt32, 0);
        case kAudioDevicePropertyLatency: case kAudioDevicePropertySafetyOffset: PUT(UInt32, 0);
        case kAudioDevicePropertyZeroTimeStampPeriod: PUT(UInt32, kZeroTimeStampPeriod);
        case kAudioDevicePropertyNominalSampleRate: PUT(Float64, kRate);
        case kAudioDevicePropertyAvailableNominalSampleRates: {
            AudioValueRange v = {kRate, kRate};
            PUT(AudioValueRange, v);
        }
        case kAudioDevicePropertyPreferredChannelsForStereo: {
            if (inDataSize < 2 * sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
            ((UInt32 *)outData)[0] = 1;
            ((UInt32 *)outData)[1] = 2;
            *outDataSize = 2 * sizeof(UInt32);
            return 0;
        }
        case kAudioObjectPropertyOwnedObjects: case kAudioDevicePropertyStreams:
            if (a->mScope == kAudioObjectPropertyScopeOutput) { *outDataSize = 0; return 0; }
            PUT(AudioObjectID, kObjectID_Stream);
        case kAudioObjectPropertyControlList: *outDataSize = 0; return 0;
        }
        break;
    case kObjectID_Stream:
        switch (a->mSelector) {
        case kAudioObjectPropertyBaseClass: PUT(AudioClassID, kAudioObjectClassID);
        case kAudioObjectPropertyClass: PUT(AudioClassID, kAudioStreamClassID);
        case kAudioObjectPropertyOwner: PUT(AudioObjectID, kObjectID_Device);
        case kAudioObjectPropertyOwnedObjects: *outDataSize = 0; return 0;
        case kAudioStreamPropertyIsActive: PUT(UInt32, 1);
        case kAudioStreamPropertyDirection: PUT(UInt32, 1);  // input
        case kAudioStreamPropertyTerminalType: PUT(UInt32, kAudioStreamTerminalTypeMicrophone);
        case kAudioStreamPropertyStartingChannel: PUT(UInt32, 1);
        case kAudioStreamPropertyLatency: PUT(UInt32, 0);
        case kAudioStreamPropertyVirtualFormat: case kAudioStreamPropertyPhysicalFormat:
            PUT(AudioStreamBasicDescription, format());
        case kAudioStreamPropertyAvailableVirtualFormats: case kAudioStreamPropertyAvailablePhysicalFormats: {
            AudioStreamRangedDescription d = {format(), {kRate, kRate}};
            PUT(AudioStreamRangedDescription, d);
        }
        }
        break;
    }
    return r;
}

// ---- driver interface --------------------------------------------------------

static HRESULT QueryInterface(void *self, REFIID iid, LPVOID *out);
static ULONG AddRef(void *self) { pthread_mutex_lock(&gLock); ULONG r = ++gRefCount; pthread_mutex_unlock(&gLock); return r; }
static ULONG Release(void *self) { pthread_mutex_lock(&gLock); ULONG r = gRefCount ? --gRefCount : 0; pthread_mutex_unlock(&gLock); return r; }

static OSStatus Initialize(AudioServerPlugInDriverRef d, AudioServerPlugInHostRef host) {
    gHost = host;
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    Float64 ticksPerSec = 1e9 * (Float64)tb.denom / (Float64)tb.numer;
    gHostTicksPerFrame = ticksPerSec / kRate;
    return 0;
}
static OSStatus CreateDevice(AudioServerPlugInDriverRef d, CFDictionaryRef desc, const AudioServerPlugInClientInfo *c, AudioObjectID *out) { return kAudioHardwareUnsupportedOperationError; }
static OSStatus DestroyDevice(AudioServerPlugInDriverRef d, AudioObjectID id) { return kAudioHardwareUnsupportedOperationError; }
static OSStatus AddDeviceClient(AudioServerPlugInDriverRef d, AudioObjectID id, const AudioServerPlugInClientInfo *c) { return 0; }
static OSStatus RemoveDeviceClient(AudioServerPlugInDriverRef d, AudioObjectID id, const AudioServerPlugInClientInfo *c) { return 0; }
static OSStatus PerformConfigChange(AudioServerPlugInDriverRef d, AudioObjectID id, UInt64 a, void *i) { return 0; }
static OSStatus AbortConfigChange(AudioServerPlugInDriverRef d, AudioObjectID id, UInt64 a, void *i) { return 0; }

static Boolean HasProperty(AudioServerPlugInDriverRef d, AudioObjectID obj, pid_t pid, const AudioObjectPropertyAddress *a) { return has(obj, a); }
static OSStatus IsPropertySettable(AudioServerPlugInDriverRef d, AudioObjectID obj, pid_t pid, const AudioObjectPropertyAddress *a, Boolean *out) {
    if (!has(obj, a)) return kAudioHardwareUnknownPropertyError;
    *out = false;
    return 0;
}
static OSStatus GetPropertyDataSize(AudioServerPlugInDriverRef d, AudioObjectID obj, pid_t pid, const AudioObjectPropertyAddress *a, UInt32 qs, const void *q, UInt32 *out) {
    if (!has(obj, a)) return kAudioHardwareUnknownPropertyError;
    return get(obj, a, 0, out, NULL);
}
static OSStatus GetPropertyData(AudioServerPlugInDriverRef d, AudioObjectID obj, pid_t pid, const AudioObjectPropertyAddress *a, UInt32 qs, const void *q, UInt32 inSize, UInt32 *outSize, void *out) {
    if (!has(obj, a)) return kAudioHardwareUnknownPropertyError;
    return get(obj, a, inSize, outSize, out);
}
static OSStatus SetPropertyData(AudioServerPlugInDriverRef d, AudioObjectID obj, pid_t pid, const AudioObjectPropertyAddress *a, UInt32 qs, const void *q, UInt32 sz, const void *data) {
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus StartIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client) {
    pthread_mutex_lock(&gLock);
    if (gIOCount++ == 0) {
        gAnchorHostTime = mach_absolute_time();
        gTimeStampCount = 0;
        ring_open();
    }
    pthread_mutex_unlock(&gLock);
    return 0;
}
static OSStatus StopIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client) {
    pthread_mutex_lock(&gLock);
    if (gIOCount && --gIOCount == 0) ring_close();
    pthread_mutex_unlock(&gLock);
    return 0;
}
static OSStatus GetZeroTimeStamp(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client, Float64 *sample, UInt64 *host, UInt64 *seed) {
    UInt64 now = mach_absolute_time();
    Float64 period = gHostTicksPerFrame * kZeroTimeStampPeriod;
    UInt64 next = gAnchorHostTime + (UInt64)((gTimeStampCount + 1) * period);
    if (next <= now) gTimeStampCount++;
    *sample = gTimeStampCount * kZeroTimeStampPeriod;
    *host = gAnchorHostTime + (UInt64)(gTimeStampCount * period);
    *seed = 1;
    return 0;
}
static OSStatus WillDoIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client, UInt32 op, Boolean *will, Boolean *inPlace) {
    *will = (op == kAudioServerPlugInIOOperationReadInput);
    *inPlace = true;
    return 0;
}
static OSStatus BeginIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client, UInt32 op, UInt32 frames, const AudioServerPlugInIOCycleInfo *info) { return 0; }
static OSStatus DoIO(AudioServerPlugInDriverRef d, AudioObjectID dev, AudioObjectID stream, UInt32 client, UInt32 op, UInt32 frames, const AudioServerPlugInIOCycleInfo *info, void *main, void *secondary) {
    if (op == kAudioServerPlugInIOOperationReadInput && main) ring_read((Float32 *)main, frames);
    return 0;
}
static OSStatus EndIO(AudioServerPlugInDriverRef d, AudioObjectID id, UInt32 client, UInt32 op, UInt32 frames, const AudioServerPlugInIOCycleInfo *info) { return 0; }

static AudioServerPlugInDriverInterface gInterface = {
    NULL, QueryInterface, AddRef, Release, Initialize, CreateDevice, DestroyDevice, AddDeviceClient,
    RemoveDeviceClient, PerformConfigChange, AbortConfigChange, HasProperty, IsPropertySettable,
    GetPropertyDataSize, GetPropertyData, SetPropertyData, StartIO, StopIO, GetZeroTimeStamp, WillDoIO,
    BeginIO, DoIO, EndIO,
};
static AudioServerPlugInDriverInterface *gInterfacePtr = &gInterface;
static AudioServerPlugInDriverRef gDriver = &gInterfacePtr;

static HRESULT QueryInterface(void *self, REFIID iid, LPVOID *out) {
    CFUUIDRef req = CFUUIDCreateFromUUIDBytes(NULL, iid);
    HRESULT r = E_NOINTERFACE;
    if (CFEqual(req, IUnknownUUID) || CFEqual(req, kAudioServerPlugInDriverInterfaceUUID)) {
        AddRef(self);
        *out = gDriver;
        r = S_OK;
    }
    CFRelease(req);
    return r;
}

void *VigaMic_Create(CFAllocatorRef alloc, CFUUIDRef type) {
    return CFEqual(type, kAudioServerPlugInTypeUUID) ? gDriver : NULL;
}
