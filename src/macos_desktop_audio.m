#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreGraphics/CoreGraphics.h>

typedef void (*WgoAudioCallback)(void *, const float *, size_t, unsigned int);
typedef void (*WgoErrorCallback)(void *, const char *);

@interface WgoDesktopOutput : NSObject <SCStreamOutput, SCStreamDelegate>
@property (nonatomic) void *context;
@property (nonatomic) WgoAudioCallback audioCallback;
@property (nonatomic) WgoErrorCallback errorCallback;
@end

@implementation WgoDesktopOutput
- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
        ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeAudio || !CMSampleBufferIsValid(sampleBuffer)) return;
    const AudioStreamBasicDescription *format =
        CMAudioFormatDescriptionGetStreamBasicDescription(CMSampleBufferGetFormatDescription(sampleBuffer));
    if (!format || format->mFormatID != kAudioFormatLinearPCM ||
        !(format->mFormatFlags & kAudioFormatFlagIsFloat) ||
        format->mBitsPerChannel != 32 || format->mChannelsPerFrame == 0 ||
        format->mSampleRate <= 0) {
        self.errorCallback(self.context, "Unsupported ScreenCaptureKit audio format");
        return;
    }
    UInt32 frames = (UInt32)CMSampleBufferGetNumSamples(sampleBuffer);
    if (!frames) return;
    size_t listSize = offsetof(AudioBufferList, mBuffers) +
        sizeof(AudioBuffer) * format->mChannelsPerFrame;
    AudioBufferList *buffers = malloc(listSize);
    if (!buffers) {
        self.errorCallback(self.context, "Out of memory receiving desktop audio");
        return;
    }
    CMBlockBufferRef block = NULL;
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, NULL, buffers, listSize, NULL, NULL,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, &block);
    if (status == noErr && buffers->mNumberBuffers > 0) {
        float *mono = malloc((size_t)frames * sizeof(float));
        if (mono) {
            BOOL nonInterleaved = (format->mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
            UInt32 channels = format->mChannelsPerFrame;
            for (UInt32 i = 0; i < frames; i++) {
                float sum = 0;
                for (UInt32 c = 0; c < channels; c++) {
                    UInt32 bufferIndex = nonInterleaved ? c : 0;
                    size_t index = nonInterleaved ? i : (size_t)i * channels + c;
                    if (bufferIndex < buffers->mNumberBuffers && buffers->mBuffers[bufferIndex].mData &&
                        (index + 1) * sizeof(float) <= buffers->mBuffers[bufferIndex].mDataByteSize) {
                        float *samples = (float *)buffers->mBuffers[bufferIndex].mData;
                        sum += samples[index];
                    }
                }
                mono[i] = sum / channels;
            }
            self.audioCallback(self.context, mono, frames, (unsigned int)format->mSampleRate);
            free(mono);
        } else {
            self.errorCallback(self.context, "Out of memory converting desktop audio");
        }
    } else {
        self.errorCallback(self.context, "Could not read desktop audio samples");
    }
    if (block) CFRelease(block);
    free(buffers);
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    self.errorCallback(self.context, error.localizedDescription.UTF8String ?: "Desktop capture stopped");
}
@end

@interface WgoDesktopCapture : NSObject
@property (nonatomic, strong) SCStream *stream;
@property (nonatomic, strong) WgoDesktopOutput *output;
@property (nonatomic, strong) dispatch_queue_t queue;
@end
@implementation WgoDesktopCapture
@end

static void setError(char *message, size_t capacity, NSString *text) {
    if (capacity) snprintf(message, capacity, "%s", text.UTF8String ?: "Unknown desktop capture error");
}

void *wgo_desktop_start(int sampleRate, void *context, WgoAudioCallback audioCallback,
                        WgoErrorCallback errorCallback, char *message, size_t capacity,
                        int *retainContext) {
    *retainContext = 0;
    if (@available(macOS 13.0, *)) {
        if (!CGPreflightScreenCaptureAccess() && !CGRequestScreenCaptureAccess()) {
            setError(message, capacity, @"Screen & System Audio Recording access was denied. Enable wgo in System Settings → Privacy & Security → Screen & System Audio Recording, then restart wgo");
            return NULL;
        }
        __block SCShareableContent *content = nil;
        __block NSError *error = nil;
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *value, NSError *failure) {
            content = value;
            error = failure;
            dispatch_semaphore_signal(done);
        }];
        if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 15 * NSEC_PER_SEC))) {
            setError(message, capacity, @"Timed out getting screen capture permission or displays");
            return NULL;
        }
        if (!content.displays.count) {
            setError(message, capacity, error.localizedDescription ?: @"No display available for system audio capture");
            return NULL;
        }
        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:content.displays.firstObject excludingWindows:@[]];
        SCStreamConfiguration *config = [SCStreamConfiguration new];
        config.capturesAudio = YES;
        config.excludesCurrentProcessAudio = NO;
        config.sampleRate = sampleRate;
        config.channelCount = 1;
        config.width = 16;
        config.height = 16;
        config.minimumFrameInterval = CMTimeMake(1, 1);
        WgoDesktopCapture *capture = [WgoDesktopCapture new];
        capture.output = [WgoDesktopOutput new];
        capture.output.context = context;
        capture.output.audioCallback = audioCallback;
        capture.output.errorCallback = errorCallback;
        capture.queue = dispatch_queue_create("wgo.desktop.audio", DISPATCH_QUEUE_SERIAL);
        capture.stream = [[SCStream alloc] initWithFilter:filter configuration:config delegate:capture.output];
        if (![capture.stream addStreamOutput:capture.output type:SCStreamOutputTypeAudio
                             sampleHandlerQueue:capture.queue error:&error]) {
            setError(message, capacity, error.localizedDescription);
            return NULL;
        }
        done = dispatch_semaphore_create(0);
        [capture.stream startCaptureWithCompletionHandler:^(NSError *failure) {
            error = failure;
            dispatch_semaphore_signal(done);
        }];
        if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 15 * NSEC_PER_SEC))) {
            dispatch_semaphore_t stopped = dispatch_semaphore_create(0);
            [capture.stream stopCaptureWithCompletionHandler:^(NSError *failure) {
                dispatch_semaphore_signal(stopped);
            }];
            if (dispatch_semaphore_wait(stopped, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC))) {
                // The framework may still call our delegate. Retain its context to avoid a use-after-free.
                *retainContext = 1;
                (void)CFBridgingRetain(capture);
            } else {
                dispatch_sync(capture.queue, ^{});
            }
            setError(message, capacity, @"Timed out starting desktop audio capture");
            return NULL;
        }
        if (error) {
            dispatch_sync(capture.queue, ^{});
            setError(message, capacity, error.localizedDescription);
            return NULL;
        }
        return (__bridge_retained void *)capture;
    }
    setError(message, capacity, @"System audio capture requires macOS 13 or later");
    return NULL;
}

int wgo_desktop_stop(void *handle) {
    if (!handle) return 1;
    WgoDesktopCapture *capture = (__bridge WgoDesktopCapture *)handle;
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [capture.stream stopCaptureWithCompletionHandler:^(NSError *error) {
        dispatch_semaphore_signal(done);
    }];
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC))) {
        // Retain the stream and callback context if macOS never confirms the stop.
        return 0;
    }
    dispatch_sync(capture.queue, ^{});
    CFRelease(handle);
    return 1;
}
