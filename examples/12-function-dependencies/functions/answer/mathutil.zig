//! A second Zig file, reached by `@import("mathutil.zig")` from the root. This
//! is the "extra code" a directory allows: split a Zig function across files and
//! the root just imports the rest.

/// The product 1..=n, the classic little pure function to put in another file.
pub fn factorial(n: u64) u64 {
    var acc: u64 = 1;
    var i: u64 = 2;
    while (i <= n) : (i += 1) acc *= i;
    return acc;
}
