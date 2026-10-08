const std = @import("std");

// First.
fn one(x: u32) u32 {
    const y = x + 1;
    return y * 2;
}

/// Second.
pub fn two(items: []const u32) u32 {
    var total: u32 = 0;
    for (items) |item| {
        total += item;
    }
    return total;
}
