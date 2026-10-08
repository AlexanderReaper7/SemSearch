#import <Foundation/Foundation.h>

@implementation Numbers

// First.
- (int)one:(int)x {
    int y = x + 1;
    return y * 2;
}

/// Second.
- (int)two:(NSArray<NSNumber *> *)items {
    int total = 0;
    for (NSNumber *item in items) {
        total += item.intValue;
    }
    return total;
}

@end
