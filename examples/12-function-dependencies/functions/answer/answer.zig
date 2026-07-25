//! A Zig function split across two files.
//!
//! This is the root — named for its directory (`answer/answer.zig`), which is how
//! `apiplant build` knows it is the entry point. It `@import`s `mathutil.zig`
//! beside it; a single-file `.zig` function has nothing to import from. The ABI
//! is reached through `@cImport` of `apiplant.h`, exactly as in
//! examples/10-zig-functions.

const std = @import("std");
const mathutil = @import("mathutil.zig");
const c = @cImport({
    @cDefine("APIPLANT_NO_PROTOTYPES", "1");
    @cInclude("apiplant.h");
});

const allocator = std.heap.c_allocator;

export fn apiplant_abi_version() u32 {
    return c.APIPLANT_ABI_VERSION;
}

export fn apiplant_manifest() [*:0]const u8 {
    return "[{" ++
        "\"name\":\"answer\"," ++
        "\"description\":\"Returns 10! via a helper in a second file.\"," ++
        "\"method\":\"POST\"," ++
        "\"visibility\":\"public\"" ++
        "}]";
}

export fn apiplant_invoke(
    name: [*:0]const u8,
    input: [*:0]const u8,
    host: ?*anyopaque,
    out: *[*:0]u8,
) i32 {
    _ = name;
    _ = input;
    _ = host;

    const value = mathutil.factorial(10);

    var buf: [32]u8 = undefined;
    const text = std.fmt.bufPrint(&buf, "{{\"answer\":{d}}}", .{value}) catch
        return c.APIPLANT_ERR_INTERNAL;

    // malloc'd + NUL-terminated, to be handed back to apiplant_free below.
    const dst = allocator.allocSentinel(u8, text.len, 0) catch
        return c.APIPLANT_ERR_INTERNAL;
    @memcpy(dst, text);
    out.* = dst.ptr;
    return c.APIPLANT_OK;
}

export fn apiplant_free(string: ?[*:0]u8) void {
    if (string) |s| std.c.free(s);
}
