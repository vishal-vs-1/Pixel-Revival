# Revival Graphics Protocol v1

Status: frozen v1 contract; U01 verified Windows x64 layouts, U02 implemented context lifecycle and bounded owned queues, U03 added internal bounded texture allocation and row uploads, U04 added internal surface operations, U05 added internal GPU Scale2x, U06 added completed-frame upload/presentation and U07 added ordered command execution with Java JNI transport. All four public feature bits are enabled for the implemented synthetic contracts; emulator integration remains separate work.

The public C declarations are in `../include/revival_graphics.h`. Python policy and wire validation live in `graphics/policy.py` and `graphics/protocol.py`. Integer encodings, names, enum values, sizes, and limits in those files must match this document. U01 compiler `static_assert` checks verify the layouts with the installed Windows x64 compiler; later compilers/platforms still need their own ABI verification. `RV_FEATURE_SURFACE_SCALE` advertises the U08 operation while retaining ABI version 1.

## ABI rules

- ABI version is `1`; all descriptor structs start with `uint32_t struct_bytes` at offset 0 and `uint32_t abi_version` at offset 4.
- Windows exports use `extern "C"`, fixed-width integers, `RV_API`, and `RV_CALL` (`__cdecl`). Handles are opaque `uint64_t`. No C++ object, STL value, C enum, C++ exception, implicit `bool`, or borrowed host pointer crosses the ABI.
- Structures use natural C alignment; there is no `#pragma pack`. The offsets and sizes below are the v1 contract. U01 must verify them with the native compiler before exporting a working binary.
- v1 callers pass the exact structure size. A smaller or larger unrecognized descriptor is rejected as unsupported; fields called `reserved_zero` must be zero.
- `RvBuildInfo.build_id` is exactly 64 lowercase ASCII hexadecimal bytes; it is not NUL-terminated. `feature_bits` is a bitwise combination of the `RV_FEATURE_*` constants.

| Structure | Size | Field offsets (decimal) |
| --- | ---: | --- |
| `RvBuildInfo` | 76 | `struct_bytes` 0, `abi_version` 4, `build_id[64]` 8, `feature_bits` 72 |
| `RvAttachConfig` | 48 | `struct_bytes` 0, `abi_version` 4, `session_epoch` 8, `texture_budget_bytes` 16, `max_queued_bytes` 24, `max_queued_ops` 28, `max_pending_frames` 32, `detail_scale` 36, `input_mode` 40, `reserved_zero` 44 |
| `RvFrameDesc` | 56 | `struct_bytes` 0, `abi_version` 4, `frame_sequence` 8, `physical_width` 16, `physical_height` 20, `physical_stride_bytes` 24, `logical_width` 28, `logical_height` 32, `source_scale` 36, `desired_scale` 40, `pixel_format` 44, `alpha_mode` 48, `origin` 52 |
| `RvViewport` | 32 | `struct_bytes` 0, `abi_version` 4, `framebuffer_width` 8, `framebuffer_height` 12, `aspect_num` 16, `aspect_den` 20, `filter` 24, `fit` 28 |
| `RvReadbackRect` | 32 | `struct_bytes` 0, `abi_version` 4, `x` 8, `y` 12, `width` 16, `height` 20, `output_format` 24, `output_alpha_mode` 28 |
| `RvPumpStats` | 40 | `struct_bytes` 0, `abi_version` 4, `operations_processed` 8, `frames_completed` 12, `queued_bytes` 16, `queued_ops` 20, `uploaded_bytes` 24, `elapsed_ns` 32 |
| `RvStats` | 64 | `struct_bytes` 0, `abi_version` 4, `packets_submitted` 8, `packets_rejected` 16, `operations_executed` 24, `bytes_uploaded` 32, `frames_presented` 40, `queued_bytes` 48, `queued_ops` 52, `pending_frames` 56, `reserved_zero` 60 |

`RvPumpStats` and `RvStats` are output snapshots; implementations set their `struct_bytes` and `abi_version` fields. Their integer counters are unsigned and wrap only according to ordinary unsigned C arithmetic. No counter is a measured GPU time unless explicitly named `elapsed_ns` and populated by a monotonic clock.

## Stable values and bounds

- Results: `OK=0`, `INVALID_ARGUMENT=1`, `UNSUPPORTED_ABI=2`, `BAD_PACKET=3`, `QUEUE_FULL=4`, `WRONG_THREAD=5`, `STALE_HANDLE=6`, `RESOURCE_LIMIT=7`, `NO_CONTEXT=8`, `GL_FAILURE=9`, `CLOSING=10`, `UNSUPPORTED_OPERATION=11`, `INTERNAL_ERROR=12`. U01 added value 11 so the ABI skeleton can say a declared graphics operation is not implemented without misreporting an ABI mismatch; U02 adds 12 for caught unexpected native exceptions.
- Input modes: `COMMANDS=1`, `COMPLETED_FRAMES=2`. Scale values: `ORIGINAL=1`, `2X=2`, `4X=4`.
- Pixels: `RGBA8=1`, `BGRA8=2`; alpha: `OPAQUE=1`, `STRAIGHT=2`, `PREMULTIPLIED=3`; origin: `TOP_LEFT=1` only in v1.
- Sampling filters: `NEAREST=1`, `LINEAR=2`; viewport fit: `DESKTOP_HEIGHT_PRESERVE_ASPECT=1`.
- Command opcodes: `CREATE_SURFACE=1`, `BEGIN_UPLOAD=2`, `UPLOAD_ROWS=3`, `END_UPLOAD=4`, `FILL=5`, `BLIT=6`, `COPY=7`, `DESTROY_SURFACE=8`, `COMMIT_FRAME=9`, `CLEAR_FRAME=10`. A self-copy is opcode 7 with identical source and destination and must use snapshot semantics.
- Limits: packet 8 MiB; aggregate queued payload 64 MiB; queued operations 8192; pending completed frames 2; logical source area 4,194,304 pixels; each aspect component 1–16,384; texture budget nonzero and at most 512 MiB. A host may select the documented 128 MiB conservative or 512 MiB dedicated starting budget through validated policy; a GPU name alone is not evidence of dedicated memory.

## Packet wire format

Packet integers are little-endian. The header has no padding and is exactly 32 bytes, encoded as `<IHHIIQQ`:

| Offset | Type | Meaning |
| ---: | --- | --- |
| 0 | u32 | magic `0x31475652`, bytes `RVG1` |
| 4 | u16 | ABI version `1` |
| 6 | u16 | header size `32` |
| 8 | u32 | total packet bytes, including header |
| 12 | u32 | number of commands |
| 16 | u64 | session epoch |
| 24 | u64 | packet sequence |

Each command begins with an exactly 8-byte `<HHI` header: opcode u16, flags u16, inclusive command size u32. The command size includes its header. v1 has no alignment padding between commands; command sizes must be at least 8. Reserved flags are zero. The sum of command sizes must end exactly at `total_bytes`, and the number parsed must equal `command_count`. A packet with an unknown opcode, stale epoch, truncated/extra bytes, invalid count, or invalid size is rejected in full before enqueue.

U00 freezes the command identifiers and envelope only. The operation-specific payload layout and field validation are frozen by the task that implements each operation, before that operation is accepted by native code. The U00 Python validator checks packet framing and rejects unknown opcodes; it does not claim that any drawing operation is implemented.

## U04 surface command payloads

All fields below are little-endian. Coordinates and rectangle sizes are signed 32-bit logical pixels with a top-left origin; handles are nonzero u64 values. Rectangle width/height must be positive, while clip width/height may be zero. Negative clip origins and rectangles partly outside the destination are allowed and intersected with destination bounds. Surface storage scale converts logical destination coordinates to physical FBO pixels.

| Opcode | Payload bytes | Fields |
| --- | ---: | --- |
| `FILL=5` | 44 | destination u64 at 0; destination rectangle `(x,y,w,h)` i32 at 8; ARGB u32 (`0xAARRGGBB`) at 24; clip rectangle i32 at 28 |
| `BLIT=6` | 72 | source u64 at 0; destination u64 at 8; source rectangle i32 at 16; destination rectangle i32 at 32; clip rectangle i32 at 48; transform u32 at 64; blend u32 at 68 |
| `COPY=7` | 72 | same layout and rules as BLIT; identical source/destination handles use a bounded source snapshot before drawing |
| `DESTROY_SURFACE=8` | 8 | destination handle u64 at 0 |

Transform indices match `GpuRenderer.CORNERS`: `0=(0,1,2,3)`, `1=(3,2,1,0)`, `2=(1,0,3,2)`, `3=(2,3,0,1)`, `4=(0,3,2,1)`, `5=(3,0,1,2)`, `6=(1,2,3,0)`, `7=(2,1,0,3)`. The four source-corner numbers are top-left, top-right, bottom-right and bottom-left. Blend values are `REPLACE=1` and premultiplied `SOURCE_OVER=2`; the latter uses additive `ONE, ONE_MINUS_SRC_ALPHA` factors for RGB and alpha, matching the existing Java renderer. Fill uses the Java renderer's floating-point `channel/255 * alpha/255` color calculation until blending; U03's integer rounding rule applies to uploaded pixel data.

The decoder checks exact payload lengths and field bounds before producing typed commands, and SurfaceOperations validates typed command values again at its boundary. It preflights the whole ordered sequence for handle existence, source initialization, destruction/use ordering and source-rectangle bounds before issuing any GL draw. This prevents malformed-command or dependency errors from partially drawing a sequence; a later driver/runtime failure is reported but does not roll back GPU writes already issued. Self-copy snapshots the source into an extra texture charged to the context budget and releases it after the blit. Nearest sampling and the existing corner mapping are used; U04 adds no new interpolation.

Validation uses remaining-length comparisons before advancing offsets or allocating memory. Native parsers must use explicit little-endian reads; they must never cast arbitrary packet bytes to a packed C struct. Packet input is copied before `rv_submit` returns and the caller retains ownership of its bytes.

## U07 surface lifecycle and frame command payloads

All values are little-endian, with no padding. A surface handle is a nonzero context-local u64. One packet is the atomic pump work unit: the consumer validates all command schemas and resource dependencies before executing its first command, then preserves the listed order. The soft pump budget is checked between packets. A GL/runtime failure during execution can still leave earlier GPU mutations from that packet applied; the consumer must return the failure and must not publish a frame whose commit command did not finish.

| Opcode | Payload bytes | Fields |
| --- | ---: | --- |
| `CREATE_SURFACE=1` | 20 | surface u64 at 0; logical width u32 at 8; logical height u32 at 12; storage scale u32 at 16 |
| `BEGIN_UPLOAD=2` | 68 | surface u64 at 0; serialized `RvFrameDesc` fields (56 bytes, field order in the ABI table) at 8; total source pixel bytes u32 at 64 |
| `UPLOAD_ROWS=3` | 24 + data | surface u64 at 0; first row u32 at 8; row count u32 at 12; source stride bytes u32 at 16; data bytes u32 at 20; exactly that many source bytes at 24 |
| `END_UPLOAD=4` | 8 | surface u64 at 0 |
| `COMMIT_FRAME=9` | 36 | source surface u64 at 0; nonzero frame sequence u64 at 8; desired scale u32 at 16; logical content rectangle `(x,y,w,h)` i32 at 20 |
| `CLEAR_FRAME=10` | 0 | no payload |

The embedded `RvFrameDesc` uses the normal fixed v1 field widths/order and is decoded field-by-field; it is not copied by casting packet bytes to a C struct. For `BEGIN_UPLOAD`, `frame_sequence` must be zero and `total_source_pixel_bytes` must equal `physical_stride_bytes * physical_height`; normal frame descriptor dimension, density, format, alpha and 64 MiB bounds apply. `UPLOAD_ROWS.data_bytes` must equal `row_count * source_stride_bytes`, fit in this command and cover only valid, previously uncovered rows. `END_UPLOAD` is valid only when every row is covered exactly once. A new upload does not replace the current active surface until `END_UPLOAD` succeeds.

`COMMIT_FRAME` snapshots the current source into the internal immutable front/back presentation surfaces; the internal candidate handle is never exposed to the packet writer. Its sequence must increase across commits and remains monotonic after `CLEAR_FRAME`. The source must be initialized and the content rectangle must fit its logical dimensions. These packet schemas were frozen before enabling command-mode submission; unknown/unsupported payloads are rejected before queue admission. The public Java writer uses positive low-half surface handles; the high bit is reserved for private presenter surfaces.

## Descriptor semantics

- `RvAttachConfig.session_epoch` identifies one context lifetime. Queue caps may not exceed the protocol maxima; `max_pending_frames` is 1 or 2; `max_queued_bytes` and `max_queued_ops` are positive. `detail_scale` is 2 or 4 for an enhanced context. `input_mode` selects either ordered surface commands or completed-frame submission. `texture_budget_bytes` is positive and no greater than 512 MiB.
- `RvFrameDesc` separates logical game pixels from physical uploaded pixels. In v1 physical width and height equal the respective logical dimension times `source_scale` (1, 2, or 4); `desired_scale` is 1, 2, or 4. Source crop is applied before this descriptor is built. The row stride is bytes and is at least `physical_width * 4`; `pixel_bytes` must equal `physical_stride_bytes * physical_height`, fit u32, and stay within the 64 MiB queued-payload cap. Logical area is bounded by 4,194,304 pixels. Format, alpha and origin use the stable values above.
- `RvViewport` dimensions are physical framebuffer pixels, not UI DIPs. Aspect components are nonzero and bounded; fit and filter use the stable values above.
- `RvReadbackRect` is in physical source-storage pixels. Output row stride and destination capacity are validated by the API; the rectangle is not a logical game-coordinate rectangle.
- `rv_submit_completed_frame` and `rv_submit_overlay` validate then copy their payload before returning. Overlay slots are 0 and 1. An overlay can be cleared only with all physical/logical dimensions and stride zero plus a zero-byte payload; other descriptor fields still use v1 values.
- `rv_submit` validates and copies complete command packets before queue admission. `rv_pump` processes one whole command packet at a time in command mode, or atomic overlay/frame units in completed-frame mode; it checks its soft time budget between these units. A zero budget performs no queue work. One packet or upload/commit may exceed the requested budget because it is not interrupted midway.
- Completed frames must use strictly increasing, nonzero sequence values for the lifetime of a context. The bounded mailbox may replace an older queued complete frame, but never drops a dependency within an ordered command packet. Queued and currently consumed pixel copies both count against the memory cap until processing frees the copy.
- `rv_present` redraws the current immutable committed snapshot at the validated desktop-height/aspect-fit viewport, with centered side/top bars when needed. Resize is represented by a new viewport and does not rescale or mutate the stored logical frame. The viewport aspect must match the committed frame. Each overlay canvas is independently aspect-fitted and centered over the framebuffer, so a desktop-sized HUD can differ from the game frame without making presentation fail.
- `rv_clear_frame` invalidates the visible committed snapshot while retaining context resources and frame-sequence monotonicity. Overlay clear is submitted using the zero-size overlay descriptor described above.
- `rv_get_device_info` writes NUL-terminated vendor, renderer and version strings into caller-owned buffers; each buffer must fit its string including the terminator. It runs on the context owner thread with the matching current GL context.
- Context attach, GL pump/present, diagnostics readback and destroy run on the thread that owns the current compatible GL context. Producers may submit copied packets/frames from other threads. The DLL retains no caller pixel/packet pointer after a submit call.
- `rv_attach_current_context` requires a current WGL context and device context, validates the required GL entry points, then returns a monotonically allocated opaque handle. Context handles are not reused; packet epochs are checked against the epoch stored in that context. Hosts must supply a new nonzero session epoch for each lifetime.
- Each context owns its GL function table, bounded queue, lifecycle state and resource-ID counter. Resource IDs are local to their context and must always be resolved together with that context handle.
- Queue limits apply to owned packet/frame payload bytes, parsed packet operations and pending committed frames. Invalid packets are fully parsed before queue admission; successful submissions own a copy before returning. `rv_begin_close` is safe from any producer thread, atomically closes admission and releases queued payloads; context destruction additionally requires the original owner thread and the same current WGL context.
- Required GL entry points are `glGetString`, `glGetIntegerv`, `glGetError`, `glGenTextures`, `glBindTexture`, `glDeleteTextures`, `glTexImage2D`, `glTexSubImage2D`, `glTexParameteri`, `glPixelStorei`, `glGenFramebuffers`, `glBindFramebuffer`, `glFramebufferTexture2D`, `glCheckFramebufferStatus`, `glDeleteFramebuffers`, `glBlendFuncSeparate` and `glBlendEquationSeparate`. Framebuffer and blend functions accept their `EXT` aliases where available. WGL null/sentinel addresses (`0`, `1`, `2`, `3`, pointer `-1`) fall back to `opengl32.dll`; a missing required export fails attach with `GL_FAILURE`.
- U02 supports attach validation, copied submission, stats, begin-close and owner-thread destruction. U03 adds internal `TextureStore` support for bounded RGBA8 texture/FBO allocation, staged row upload and atomic publication; it is not reachable through a successful public draw API. Straight alpha is premultiplied with `(channel*alpha+127)/255`, premultiplied data is not multiplied again, opaque input forces alpha to 255, and zero-alpha RGB is cleared. RGBA/BGRA become premultiplied RGBA; source rows are top-left ordered and are mapped to matching logical FBO orientation. Upload chunks must match the descriptor stride, cover each row once, and fit the 8 MiB packet limit. An incomplete END leaves the staging version available for more rows; abort or GL failure discards staging and keeps the active generation. Replacements account for both generations against the context budget until the atomic swap retires the old texture. Mutations and inspection are restricted to the owning thread/current WGL context; deleted currently bound resources are unbound while unrelated bindings and unpack alignment are preserved during creation/upload.
- U04 adds an internal typed command decoder and owner-thread SurfaceOperations/OpenGL device for fill, blit, clip, eight transforms, replace/source-over blend, destruction and self-copy snapshots. Drawing uses isolated identity matrices and restores caller matrices and relevant GL state. Queue admission validates FILL/BLIT/COPY/DESTROY payloads, but no pump executes them yet.
- U05 adds one internal native Scale2x renderer with a single embedded GLSL 1.20 source. It compares RGBA values, clamps neighbors, performs one pass to 2x or two passes to 4x, honors declared source density, skips same-handle already-detailed content, and reports a generic compatibility-sampling fallback separately from Scale2x. It is not wired to the artwork cache.
- U06 adds a completed-frame public path: bounded copied frame and overlay mailboxes, immutable front/back commits, a soft owner-thread pump, aspect-fit presentation, clear, device strings and diagnostic surface readback. U07 adds the separate ordered command packet consumer and enables `RV_FEATURE_SURFACE_COMMANDS`. The U07 opt-in WGL harness covers command preflight/order/upload/fill/commit/presentation and caller-state preservation. These synthetic checks do not establish RTX behavior, visible gameplay, emulator parity or NES compatibility.
- U03–U07 CPU fixtures cover conversion, padding/chunks, atomic replacement, budgets, device dimensions, retirement, exact transform maps, clips, blending, self-copy, dependency preflight, Scale2x corner/alpha/edge rules, frame sequence/coalescing/in-flight bounds, command packet schemas/order and readback validation. The opt-in hidden WGL harness verifies actual upload/readback, all eight asymmetric blit transforms, clipped fill, premultiplied source-over, self-copy, 2x and two-pass 4x Scale2x byte-for-byte against the CPU reference, completed-frame/overlay presentation and ordered command execution on AMD Radeon(TM) Graphics / OpenGL 4.6 Compatibility Profile. This does not establish RTX behavior, visible gameplay, emulator parity or NES compatibility.

`rv_readback_for_check` is diagnostic only and never establishes the normal frame transport path. Its rectangle uses top-left physical surface pixels; output supports RGBA8/BGRA8 and opaque/straight/premultiplied alpha, padded destination rows, and a maximum 64 MiB destination. It writes destination bytes only after validation and successful GL readback. Device/error text uses caller-owned UTF-8 buffers with explicit capacities; v1 returns no allocated string pointer.

## CPU fixture status

`tests/fixtures/graphics_v1_packets.json` contains fixed wire bytes, independent of the parser. `tests/test_graphics_protocol.py` checks those bytes, policy constraints, modeled ctypes field offsets/sizes and header constants. Native C11 and C++17 `sizeof`/`offsetof` assertions also compile in U01 with the recorded Windows x64 toolchain; another compiler or platform needs its own verification.
