//! A complete apiplant function in Zig.
//!
//! Two endpoints from one file:
//!
//!   POST /api/functions/hello   public         — greets someone, using config
//!   GET  /api/functions/notes   authenticated  — counts rows via the host's DB
//!
//! `apiplant build` compiles this with `zig build-lib -dynamic` and drops
//! libhello.so next to it. Zig reaches the ABI by `@cImport`ing the same
//! `apiplant.h` a C function includes, so the contract below is identical to
//! example 09 — this is what it looks like with slices, errors and `defer`
//! instead of raw pointers and manual frees.

const std = @import("std");

const c = @cImport({
    @cInclude("apiplant.h");
});

/// Everything handed across the boundary is allocated with libc's malloc, so the
/// host's `apiplant_free` below is just `free`. Using Zig's own allocators here
/// would mean tracking which side allocated what, for no benefit.
const allocator = std.heap.c_allocator;

// ---- the manifest ----------------------------------------------------------

/// Static, because the host never frees it. `visibility` defaults to "private",
/// so both entries state it explicitly.
const manifest =
    \\[
    \\  {
    \\    "name": "hello",
    \\    "version": "1.0.0",
    \\    "description": "Greets someone from Zig.",
    \\    "visibility": "public",
    \\    "method": "POST",
    \\    "input_schema": {
    \\      "type": "object",
    \\      "required": ["name"],
    \\      "properties": { "name": { "type": "string", "description": "Who to greet." } }
    \\    },
    \\    "output_schema": {
    \\      "type": "object",
    \\      "properties": {
    \\        "message":     { "type": "string" },
    \\        "compiled_by": { "type": "string" }
    \\      }
    \\    }
    \\  },
    \\  {
    \\    "name": "notes",
    \\    "version": "1.0.0",
    \\    "description": "Counts notes, to show a query from Zig.",
    \\    "visibility": "authenticated",
    \\    "method": "GET",
    \\    "output_schema": {
    \\      "type": "object",
    \\      "properties": {
    \\        "notes":  { "type": "integer" },
    \\        "caller": { "type": "string"  }
    \\      }
    \\    }
    \\  }
    \\]
;

export fn apiplant_abi_version() u32 {
    return c.APIPLANT_ABI_VERSION;
}

export fn apiplant_manifest() [*:0]const u8 {
    return manifest;
}

export fn apiplant_free(string: ?[*:0]u8) void {
    if (string) |s| std.c.free(s);
}

// ---- talking to the host ---------------------------------------------------

/// Borrow a string from the host, run `body` with it, and hand it back.
///
/// The host owns what it returns, so every one of these calls has to be paired
/// with `free_string` — `defer` is what makes that hard to get wrong.
fn withHostString(
    host: *const c.ApiplantHost,
    getter: ?*const fn (?*anyopaque) callconv(.c) [*c]u8,
    comptime T: type,
    body: *const fn ([:0]const u8) T,
) ?T {
    const raw = getter.?(host.ctx);
    if (raw == null) return null;
    defer host.free_string.?(host.ctx, raw);
    return body(std.mem.span(@as([*:0]const u8, @ptrCast(raw))));
}

/// Copy a Zig slice into malloc'd, NUL-terminated storage for the host.
fn toHost(text: []const u8) ?[*:0]u8 {
    const buffer = allocator.allocSentinel(u8, text.len, 0) catch return null;
    @memcpy(buffer, text);
    return buffer.ptr;
}

// ---- a very small JSON reader ----------------------------------------------

/// The value of a top-level `"key": "..."` pair, decoded into fresh storage.
///
/// Zig ships a real JSON parser in `std.json`; this is here only so the example
/// stays comparable to the C one line for line. Reach for `std.json` in anger.
/// Handles the single-character escapes and refuses `\uXXXX`.
fn jsonString(json: []const u8, key: []const u8) ?[]u8 {
    var pattern_buf: [64]u8 = undefined;
    const pattern = std.fmt.bufPrint(&pattern_buf, "\"{s}\"", .{key}) catch return null;

    const key_at = std.mem.indexOf(u8, json, pattern) orelse return null;
    var i = key_at + pattern.len;

    // Skip to the colon, then past any whitespace, and require a string.
    i = std.mem.indexOfScalarPos(u8, json, i, ':') orelse return null;
    i += 1;
    while (i < json.len and (json[i] == ' ' or json[i] == '\t' or
        json[i] == '\n' or json[i] == '\r')) i += 1;
    if (i >= json.len or json[i] != '"') return null;
    i += 1;

    // Decoding only ever shortens, so the rest of the input is a safe bound.
    var out = allocator.alloc(u8, json.len - i) catch return null;
    var written: usize = 0;
    var ok = false;

    while (i < json.len) : (i += 1) {
        if (json[i] == '"') {
            ok = true;
            break;
        }
        if (json[i] != '\\') {
            out[written] = json[i];
            written += 1;
            continue;
        }
        i += 1;
        if (i >= json.len) break;
        out[written] = switch (json[i]) {
            '"', '\\', '/' => json[i],
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => 8,
            'f' => 12,
            // \uXXXX needs UTF-16 surrogate handling; refuse rather than guess.
            else => {
                allocator.free(out);
                return null;
            },
        };
        written += 1;
    }
    if (!ok) {
        allocator.free(out);
        return null;
    }
    return out[0..written];
}

/// Append `text` to `list`, escaping what would break out of a JSON string.
fn appendEscaped(list: *std.ArrayList(u8), text: []const u8) !void {
    for (text) |ch| switch (ch) {
        '"', '\\' => try list.appendSlice(allocator, &[_]u8{ '\\', ch }),
        '\n' => try list.appendSlice(allocator, "\\n"),
        '\r' => try list.appendSlice(allocator, "\\r"),
        '\t' => try list.appendSlice(allocator, "\\t"),
        // Remaining control characters would need \u00XX; a space keeps the
        // output valid JSON, which is all this example needs.
        0...7, 11, 14...31 => try list.append(allocator, ' '),
        else => try list.append(allocator, ch),
    };
}

// ---- hello -----------------------------------------------------------------

fn hello(input: []const u8, host: *const c.ApiplantHost, out: *?[*:0]u8) i32 {
    const name = jsonString(input, "name") orelse {
        out.* = toHost("`name` is required and must be a string");
        return c.APIPLANT_ERR_REQUEST;
    };
    defer allocator.free(name);

    // functions/hello.toml, converted to JSON by the host.
    var greeting: []u8 = allocator.dupe(u8, "Hello") catch return c.APIPLANT_ERR_INTERNAL;
    defer allocator.free(greeting);
    if (withHostString(host, host.config, ?[]u8, struct {
        fn f(config: [:0]const u8) ?[]u8 {
            return jsonString(config, "greeting");
        }
    }.f)) |configured| {
        if (configured) |g| {
            allocator.free(greeting);
            greeting = g;
        }
    }

    host.log.?(host.ctx, c.APIPLANT_INFO, "hello invoked from Zig");

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(allocator);

    body.appendSlice(allocator, "{\"message\":\"") catch return c.APIPLANT_ERR_INTERNAL;
    appendEscaped(&body, greeting) catch return c.APIPLANT_ERR_INTERNAL;
    body.appendSlice(allocator, ", ") catch return c.APIPLANT_ERR_INTERNAL;
    appendEscaped(&body, name) catch return c.APIPLANT_ERR_INTERNAL;
    body.appendSlice(allocator, "!\",\"compiled_by\":\"zig ") catch return c.APIPLANT_ERR_INTERNAL;
    body.appendSlice(allocator, @import("builtin").zig_version_string) catch
        return c.APIPLANT_ERR_INTERNAL;
    body.appendSlice(allocator, "\"}") catch return c.APIPLANT_ERR_INTERNAL;

    out.* = toHost(body.items);
    return if (out.* == null) c.APIPLANT_ERR_INTERNAL else c.APIPLANT_OK;
}

// ---- notes -----------------------------------------------------------------

fn notes(host: *const c.ApiplantHost, out: *?[*:0]u8) i32 {
    // Same request shape the Rust side uses: {"sql": ..., "params": [...]}.
    // A SELECT comes back as a JSON array of row objects.
    const request = "{\"sql\":\"SELECT count(*)::int AS n FROM apiplant_note\",\"params\":[]}";
    const raw = host.query.?(host.ctx, request);
    if (raw == null) {
        out.* = toHost("query returned nothing");
        return c.APIPLANT_ERR_INTERNAL;
    }
    defer host.free_string.?(host.ctx, raw);
    const rows = std.mem.span(@as([*:0]const u8, @ptrCast(raw)));

    // An object with "error" means the query failed; rows arrive as an array.
    if (jsonString(rows, "error")) |failure| {
        defer allocator.free(failure);
        const message = std.fmt.allocPrint(allocator, "query failed: {s}", .{failure}) catch
            return c.APIPLANT_ERR_INTERNAL;
        defer allocator.free(message);
        out.* = toHost(message);
        return c.APIPLANT_ERR_INTERNAL;
    }

    // [{"n":3}] — find the number after the field name.
    var count: i64 = 0;
    if (std.mem.indexOf(u8, rows, "\"n\"")) |n_at| {
        if (std.mem.indexOfScalarPos(u8, rows, n_at, ':')) |colon| {
            var end = colon + 1;
            while (end < rows.len and (rows[end] == ' ')) end += 1;
            const start = end;
            while (end < rows.len and (std.ascii.isDigit(rows[end]) or rows[end] == '-')) end += 1;
            count = std.fmt.parseInt(i64, rows[start..end], 10) catch 0;
        }
    }

    const caller_raw = host.principal_id.?(host.ctx);
    defer if (caller_raw != null) host.free_string.?(host.ctx, caller_raw);
    const caller = if (caller_raw == null)
        ""
    else
        std.mem.span(@as([*:0]const u8, @ptrCast(caller_raw)));

    const prefix = std.fmt.allocPrint(
        allocator,
        "{{\"notes\":{d},\"caller\":\"",
        .{count},
    ) catch return c.APIPLANT_ERR_INTERNAL;
    defer allocator.free(prefix);

    var body: std.ArrayList(u8) = .empty;
    defer body.deinit(allocator);
    body.appendSlice(allocator, prefix) catch return c.APIPLANT_ERR_INTERNAL;
    appendEscaped(&body, caller) catch return c.APIPLANT_ERR_INTERNAL;
    body.appendSlice(allocator, "\"}") catch return c.APIPLANT_ERR_INTERNAL;

    out.* = toHost(body.items);
    return if (out.* == null) c.APIPLANT_ERR_INTERNAL else c.APIPLANT_OK;
}

// ---- dispatch --------------------------------------------------------------

/// One library, several functions — the host passes the manifest name so this
/// routes on it, exactly as the Rust `functions!` macro does behind the scenes.
export fn apiplant_invoke(
    name: [*:0]const u8,
    input_json: [*:0]const u8,
    host: *const c.ApiplantHost,
    out: *?[*:0]u8,
) i32 {
    out.* = null;

    const which = std.mem.span(name);
    const input = std.mem.span(input_json);

    if (std.mem.eql(u8, which, "hello")) return hello(input, host, out);
    if (std.mem.eql(u8, which, "notes")) return notes(host, out);

    const message = std.fmt.allocPrint(
        allocator,
        "no function named `{s}` in this library",
        .{which},
    ) catch return c.APIPLANT_ERR_INTERNAL;
    defer allocator.free(message);
    out.* = toHost(message);
    return c.APIPLANT_ERR_INTERNAL;
}
