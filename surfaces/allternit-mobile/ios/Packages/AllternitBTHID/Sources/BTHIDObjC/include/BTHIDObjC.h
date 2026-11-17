// Turns an Objective-C exception into a value. CoreBluetooth raises
// NSExceptions (not Swift errors) for some arguments it does not accept, for
// example a descriptor type it does not support; Swift cannot catch those.
// AllternitBTHID builds its GATT objects through this so a refusal becomes
// the route's "not available" state instead of a crash. Our own code.

#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Runs `block`. Returns nil when it finished, or the exception's name and reason when it raised.
FOUNDATION_EXPORT NSString *_Nullable BTHIDCatchException(NS_NOESCAPE void (^block)(void));

NS_ASSUME_NONNULL_END
