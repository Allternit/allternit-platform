// Allternit: the iOS Simulator SDK's Metal doesn't export these two error
// domains, which mlx-swift's Cmlx references from static initializers, so a
// simulator build of the app fails to link. Kokoro never runs on the
// simulator (KokoroVoices.deviceSupported), so plain string constants are
// enough to link. Device builds compile nothing here.
#include <TargetConditionals.h>
#if TARGET_OS_SIMULATOR
#import <Foundation/Foundation.h>
NSString *const MTLTensorDomain = @"MTLTensorDomain";
NSString *const MTLIOErrorDomain = @"MTLIOErrorDomain";
#endif
