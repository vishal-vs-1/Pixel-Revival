//! Safe C ABI over `mesen-core` for the JAR Revival desktop tool.
//!
//! Design notes (do not regress):
//! - No global state: every call takes an explicit `*mut RsPlayer` handle.
//! - Panic-safe: every entry point catches unwinds and returns an error code.
//! - Null/invalid arguments are rejected, never dereferenced.
//! - `run_frame` returns the completed 256x240 XRGB framebuffer; the pointer
//!   stays valid until the next mutating call on that handle.
//! - Button bits match the NES shift-register order: A=0x01 B=0x02 Sel=0x04
//!   Start=0x08 Up=0x10 Down=0x20 Left=0x40 Right=0x80.
//! - Region codes: 0=NTSC 1=PAL 2=Dendy.

use std::collections::HashSet;
use std::os::raw::{c_char, c_float, c_int, c_uchar, c_uint, c_ulonglong};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

use mesen_core::hd::HdPixel;
use mesen_core::{Cartridge, Emulator};

mod hd;
use hd::HdTable;

/// Opaque player handle. Always heap-allocated, always freed by `rs_destroy`.
pub struct RsPlayer {
    emu: Emulator,
    /// Unique tiles observed while capture is armed (sprite<<63|chr<<32|pal).
    capture_keys: HashSet<u64>,
    capture_active: bool,
}

fn write_err(err: *mut c_char, err_len: usize, msg: &str) {
    if err.is_null() || err_len == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let room = err_len.saturating_sub(1);
    let take = bytes.len().min(room);
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), err as *mut u8, take);
        *err.add(take) = 0;
    }
}

fn with_player<T, F>(handle: *mut RsPlayer, err: *mut c_char, err_len: usize, op: F) -> T
where
    F: FnOnce(&mut Emulator) -> T,
    T: Default,
{
    if handle.is_null() {
        write_err(err, err_len, "null player handle");
        return T::default();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let player = unsafe { &mut *handle };
        op(&mut player.emu)
    }));
    match result {
        Ok(value) => value,
        Err(_) => {
            write_err(err, err_len, "internal emulator fault (caught)");
            T::default()
        }
    }
}

/// Load a ROM image. Returns null on failure (reason in `err` when provided).
///
/// `rom` must point to `rom_len` readable bytes (iNES/NES 2.0/UNIF/NSF/FDS).
/// `sample_rate` is the audio rate in Hz (44100 typical).
#[no_mangle]
pub unsafe extern "C" fn rs_create(
    rom: *const c_uchar,
    rom_len: usize,
    sample_rate: c_uint,
    err: *mut c_char,
    err_len: usize,
) -> *mut RsPlayer {
    if rom.is_null() || rom_len == 0 {
        write_err(err, err_len, "empty ROM image");
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let data: &[u8] = slice::from_raw_parts(rom, rom_len);
        let cart = Cartridge::from_bytes(data).map_err(|e| e.0)?;
        Ok::<Emulator, String>(Emulator::new(cart, sample_rate as u32))
    }));
    match result {
        Ok(Ok(emu)) => Box::into_raw(Box::new(RsPlayer {
            emu,
            capture_keys: HashSet::new(),
            capture_active: false,
        })),
        Ok(Err(detail)) => {
            let mut msg = String::from("unsupported ROM: ");
            msg.push_str(&detail);
            write_err(err, err_len, &msg);
            ptr::null_mut()
        }
        Err(_) => {
            write_err(err, err_len, "internal emulator fault during load (caught)");
            ptr::null_mut()
        }
    }
}

/// Free a player created by `rs_create`. Null is a no-op.
#[no_mangle]
pub unsafe extern "C" fn rs_destroy(handle: *mut RsPlayer) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Box::from_raw(handle));
    }));
}

/// Run until one frame completes. Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_run_frame(handle: *mut RsPlayer, err: *mut c_char, err_len: usize) -> c_int {
    with_player(handle, err, err_len, |emu| {
        emu.run_frame();
        0 as c_int
    })
}

/// Latest completed 256x240 XRGB framebuffer (4 bytes/pixel, row-major).
/// Valid until the next mutating call on this handle. Null on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_framebuffer(
    handle: *mut RsPlayer,
    out_w: *mut c_int,
    out_h: *mut c_int,
    err: *mut c_char,
    err_len: usize,
) -> *const u32 {
    if handle.is_null() {
        write_err(err, err_len, "null player handle");
        return ptr::null();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let player = &*handle;
        let fb = player.emu.framebuffer();
        if !out_w.is_null() {
            *out_w = Emulator::SCREEN_WIDTH as c_int;
        }
        if !out_h.is_null() {
            *out_h = Emulator::SCREEN_HEIGHT as c_int;
        }
        fb.as_ptr()
    }));
    match result {
        Ok(p) => p,
        Err(_) => {
            write_err(err, err_len, "internal emulator fault (caught)");
            ptr::null()
        }
    }
}

/// Drain pending mono audio samples (f32). Returns samples written, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_drain_audio(
    handle: *mut RsPlayer,
    out: *mut c_float,
    capacity: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if out.is_null() && capacity > 0 {
        write_err(err, err_len, "null audio buffer");
        return -1;
    }
    with_player(handle, err, err_len, |emu| {
        let mut samples: Vec<f32> = Vec::new();
        emu.drain_audio(&mut samples);
        let take = samples.len().min(capacity);
        if take > 0 {
            ptr::copy_nonoverlapping(samples.as_ptr(), out, take);
        }
        take as c_int
    })
}

/// Set controller state for `port` (0/1). Bits: A=0x01 B=0x02 Sel=0x04
/// Start=0x08 Up=0x10 Down=0x20 Left=0x40 Right=0x80.
#[no_mangle]
pub unsafe extern "C" fn rs_set_controller(
    handle: *mut RsPlayer,
    port: c_int,
    buttons: c_uchar,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    with_player(handle, err, err_len, |emu| {
        emu.set_controller(port as usize, buttons);
        0 as c_int
    })
}

/// Soft reset (CPU + bus). Returns 0, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_reset(handle: *mut RsPlayer, err: *mut c_char, err_len: usize) -> c_int {
    with_player(handle, err, err_len, |emu| {
        emu.reset();
        0 as c_int
    })
}

/// Battery-backed SRAM size in bytes, or -1 when this board has none.
#[no_mangle]
pub unsafe extern "C" fn rs_battery_size(
    handle: *mut RsPlayer,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    with_player(handle, err, err_len, |emu| match emu.battery() {
        Some(data) => data.len() as c_int,
        None => -1 as c_int,
    })
}

/// Copy SRAM bytes out. Returns 0 on success, -1 when absent/too small/failure.
#[no_mangle]
pub unsafe extern "C" fn rs_get_battery(
    handle: *mut RsPlayer,
    out: *mut c_uchar,
    len: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if out.is_null() {
        write_err(err, err_len, "null battery buffer");
        return -1;
    }
    with_player(handle, err, err_len, |emu| match emu.battery() {
        Some(data) if data.len() <= len => {
            ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
            0 as c_int
        }
        Some(_) => {
            // Caller learns the size via rs_battery_size; report, don't truncate.
            -1 as c_int
        }
        None => -1 as c_int,
    })
}

/// Restore SRAM bytes. Length must match exactly. Returns 0, -1 on mismatch/failure.
#[no_mangle]
pub unsafe extern "C" fn rs_set_battery(
    handle: *mut RsPlayer,
    data: *const c_uchar,
    len: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if data.is_null() && len > 0 {
        write_err(err, err_len, "null battery data");
        return -1;
    }
    with_player(handle, err, err_len, |emu| {
        let expected = match emu.battery() {
            Some(current) => current.len(),
            None => return -1 as c_int,
        };
        if len != expected {
            return -1 as c_int;
        }
        let bytes: &[u8] = slice::from_raw_parts(data, len);
        emu.set_battery(bytes);
        0 as c_int
    })
}

/// Save state. With `out == null` (or too small), returns the required size
/// without writing. Otherwise writes and returns bytes written. -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_save_state(
    handle: *mut RsPlayer,
    out: *mut c_uchar,
    capacity: usize,
    err: *mut c_char,
    err_len: usize,
) -> isize {
    if handle.is_null() {
        write_err(err, err_len, "null player handle");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let player = &(*handle);
        player.emu.save_state()
    }));
    let blob = match result {
        Ok(blob) => blob,
        Err(_) => {
            write_err(err, err_len, "internal emulator fault (caught)");
            return -1;
        }
    };
    if out.is_null() || capacity < blob.len() {
        return blob.len() as isize;
    }
    ptr::copy_nonoverlapping(blob.as_ptr(), out, blob.len());
    blob.len() as isize
}

/// Load a state blob produced by `rs_save_state`. Returns 0, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_load_state(
    handle: *mut RsPlayer,
    data: *const c_uchar,
    len: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if (data.is_null() && len > 0) || len == 0 {
        write_err(err, err_len, "empty save-state blob");
        return -1;
    }
    with_player(handle, err, err_len, |emu| {
        let bytes: &[u8] = slice::from_raw_parts(data, len);
        match emu.load_state(bytes) {
            Ok(()) => 0 as c_int,
            Err(_) => -1 as c_int,
        }
    })
}

/// Completed frame count. 0 on null handle.
#[no_mangle]
pub unsafe extern "C" fn rs_frame_count(handle: *mut RsPlayer) -> c_ulonglong {
    if handle.is_null() {
        return 0;
    }
    catch_unwind(AssertUnwindSafe(|| (*handle).emu.frame_count())).unwrap_or(0)
}

/// Region code: 0=NTSC 1=PAL 2=Dendy. -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_region(handle: *mut RsPlayer, err: *mut c_char, err_len: usize) -> c_int {
    with_player(handle, err, err_len, |emu| {
        use mesen_core::Region;
        match emu.region() {
            Region::Ntsc => 0 as c_int,
            Region::Pal => 1 as c_int,
            Region::Dendy => 2 as c_int,
        }
    })
}

/// Opaque HD replacement table. Owns replacement art plus one reusable
/// per-frame record buffer, so compositing allocates nothing per frame.
pub struct RsHdTable {
    table: HdTable,
    records: Vec<HdPixel>,
}

/// Create an HD table for `scale` (2 or 4). Null on misuse (scale 0).
#[no_mangle]
pub extern "C" fn rs_hd_create(scale: c_uint) -> *mut RsHdTable {
    if scale != 2 && scale != 4 {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        Box::into_raw(Box::new(RsHdTable {
            table: HdTable::new(scale),
            records: Vec::new(),
        }))
    }));
    result.unwrap_or(ptr::null_mut())
}

/// Free an HD table. Null is a no-op.
#[no_mangle]
pub unsafe extern "C" fn rs_hd_destroy(handle: *mut RsHdTable) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(Box::from_raw(handle));
    }));
}

/// Drop all replacement art (keeps the record buffer).
#[no_mangle]
pub unsafe extern "C" fn rs_hd_clear(handle: *mut RsHdTable) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        (*handle).table.clear();
    }));
}

/// Add one 8x8 tile's replacement art. `sprite` distinguishes the sprite and
/// background namespaces (matching the pack bindings). `chr`/`pal` are the
/// Mesen key (tile number + packed palette). `pixels` must hold
/// `size * size` RGBA bytes with `size == 8 * table scale`. Returns the
/// table size, or -1 on any failure.
#[no_mangle]
pub unsafe extern "C" fn rs_hd_add_tile(
    handle: *mut RsHdTable,
    sprite: c_uchar,
    chr: u32,
    pal: u32,
    pixels: *const c_uchar,
    pixel_len: usize,
    size: c_uint,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if handle.is_null() || (pixels.is_null() && pixel_len > 0) {
        write_err(err, err_len, "null HD table or pixel data");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let table = &mut (*handle).table;
        let art: Vec<u8> = slice::from_raw_parts(pixels, pixel_len).to_vec();
        if !table.add(sprite != 0, chr, pal, art, size as usize) {
            return Err("rejected HD tile (bad size or length)");
        }
        Ok(table.len() as c_int)
    }));
    match result {
        Ok(Ok(count)) => count,
        Ok(Err(message)) => {
            write_err(err, err_len, message);
            -1
        }
        Err(_) => {
            write_err(err, err_len, "internal emulator fault (caught)");
            -1
        }
    }
}

    /// Arm (`nonzero`) or disarm HD tile recording for Enhanced playback.
    /// The core derives CHR-RAM-ness from the cartridge itself.
    #[no_mangle]
    pub unsafe extern "C" fn rs_hd_arm(
    player: *mut RsPlayer,
    active: c_uchar,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    with_player(player, err, err_len, |emu| {
        emu.set_hd(active != 0);
        0 as c_int
    })
}

/// Debug probe: report (armed, chr_ram, pending_records) without draining.
/// Lets callers tell "not armed" apart from "armed but no pixels".
#[no_mangle]
pub unsafe extern "C" fn rs_hd_debug(
    player: *mut RsPlayer,
    active_out: *mut c_uchar,
    is_ram_out: *mut c_uchar,
    pending_out: *mut c_ulonglong,
) -> c_int {
    if player.is_null() {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let emu = &(*player).emu;
        emu.hd_debug()
    }));
    match result {
        Ok((active, is_ram, pending)) => {
            if !active_out.is_null() {
                *active_out = active as c_uchar;
            }
            if !is_ram_out.is_null() {
                *is_ram_out = is_ram as c_uchar;
            }
            if !pending_out.is_null() {
                *pending_out = pending as c_ulonglong;
            }
            0
        }
        Err(_) => -1,
    }
}

/// Frames composited (`lookups`) and HD tiles actually painted (`hits`)
/// since creation or the last call. Counters never wrap in practice.
#[no_mangle]
pub unsafe extern "C" fn rs_hd_stats(
    handle: *const RsHdTable,
    lookups: *mut c_ulonglong,
    hits: *mut c_ulonglong,
) {
    if handle.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let table = &*handle;
        if !lookups.is_null() {
            *lookups = table.table.lookups;
        }
        if !hits.is_null() {
            *hits = table.table.hits;
        }
    }));
}

/// Output dimensions for this table's scale: 256*S x 240*S XRGB pixels.
#[no_mangle]
pub unsafe extern "C" fn rs_hd_output_size(
    handle: *const RsHdTable,
    out_w: *mut c_int,
    out_h: *mut c_int,
) -> c_int {
    if handle.is_null() {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| (*handle).table.scale()));
    match result {
        Ok(scale) => {
            if !out_w.is_null() {
                *out_w = (256 * scale) as c_int;
            }
            if !out_h.is_null() {
                *out_h = (240 * scale) as c_int;
            }
            0
        }
        Err(_) => -1,
    }
}

/// Run one frame and composite the HD output in a single call.
///
/// Records the frame's winning tiles, then paints replacements over a
/// nearest-scaled native frame into `out` (`out_cap` bytes). Returns bytes
/// written, or -1 on failure. With an empty table the output is exactly the
/// scaled native frame, so Enhanced without a pack still renders correctly.
#[no_mangle]
pub unsafe extern "C" fn rs_hd_run_frame(
    player: *mut RsPlayer,
    hd: *mut RsHdTable,
    out: *mut u32,
    out_cap: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if player.is_null() || hd.is_null() || out.is_null() {
        write_err(err, err_len, "null player, HD table or output buffer");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let game = &mut *player;
        let table = &mut *hd;
        let frame = game.emu.run_frame();
        let native: Vec<u32> = frame.to_vec();
        game.emu.take_hd_pixels(&mut table.records);
        if game.capture_active {
            // Collect unique tile identities, bounded so a long session cannot
            // grow without limit.
            for record in table.records.iter() {
                if record.ram || game.capture_keys.len() >= 20_000 {
                    continue;
                }
                let key = ((record.sprite as u64) << 63)
                    | ((record.chr as u64) << 32)
                    | (record.pal as u64);
                game.capture_keys.insert(key);
            }
        }
        let s = table.table.scale() as usize;
        let need = 256 * s * 240 * s * 4;
        if out_cap < need {
            return Err("HD output buffer too small");
        }
        // `out` holds `need/4` XRGB pixels; build the u32 view directly so
        // `composite` sees the exact pixel count it expects.
        let pixels = slice::from_raw_parts_mut(out, need / 4);
        if table.table.composite(&table.records, &native, pixels) {
            return Ok(need as c_int);
        }
        // No replacements applied: plain nearest-scaled native frame.
        for y in 0..240 * s {
            let row = &native[(y / s) * 256..(y / s) * 256 + 256];
            let dest_row = &mut pixels[y * 256 * s..(y + 1) * 256 * s];
            for x in 0..256 * s {
                dest_row[x] = row[x / s] | 0xFF00_0000;
            }
        }
        Ok(need as c_int)
    }));
    match result {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(message)) => {
            write_err(err, err_len, message);
            -1
        }
        Err(_) => {
            write_err(err, err_len, "internal emulator fault (caught)");
            -1
        }
    }
}

/// Arm (`nonzero`) or disarm tile capture for new artwork.
#[no_mangle]
pub unsafe extern "C" fn rs_capture_arm(
    player: *mut RsPlayer,
    active: c_uchar,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    if player.is_null() {
        write_err(err, err_len, "null player handle");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let game = &mut *player;
        game.capture_active = active != 0;
        if !game.capture_active {
            game.capture_keys.clear();
        }
        0 as c_int
    }));
    result.unwrap_or(-1)
}

/// Copy unique captured keys (`sprite<<63|chr<<32|pal`) into `out`.
/// Returns the number written (or the total when `out` is null).
#[no_mangle]
pub unsafe extern "C" fn rs_capture_export(
    player: *mut RsPlayer,
    out: *mut u64,
    capacity: usize,
) -> c_int {
    if player.is_null() || (out.is_null() && capacity > 0) {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let game = &*player;
        if out.is_null() {
            return game.capture_keys.len().min(c_int::MAX as usize) as c_int;
        }
        let mut count = 0usize;
        for key in game.capture_keys.iter().take(capacity) {
            *out.add(count) = *key;
            count += 1;
        }
        count as c_int
    }));
    result.unwrap_or(-1)
}

/// Copy the current PPU sprite/OAM observation for pose recording.
/// Layout: 1 byte large-sprite flag, u16 LE sprite pattern base, 256 OAM
/// bytes, 32 palette bytes. Returns 291 on success.
#[no_mangle]
pub unsafe extern "C" fn rs_observation(
    player: *mut RsPlayer,
    out: *mut c_uchar,
    out_len: usize,
    err: *mut c_char,
    err_len: usize,
) -> c_int {
    const SIZE: usize = 1 + 2 + 0x100 + 0x20;
    if player.is_null() || out.is_null() || out_len < SIZE {
        write_err(err, err_len, "observation buffer is null or too small");
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let game = &mut *player;
        let observation = game.emu.observation();
        *out = if observation.large_sprites { 1 } else { 0 };
        *out.add(1) = observation.sprite_pattern_addr as u8;
        *out.add(2) = (observation.sprite_pattern_addr >> 8) as u8;
        ptr::copy_nonoverlapping(observation.oam.as_ptr(), out.add(3), 0x100);
        ptr::copy_nonoverlapping(observation.palette.as_ptr(), out.add(3 + 0x100), 0x20);
        SIZE as c_int
    }));
    result.unwrap_or(-1)
}

/// Optional pose observation protocol: fixed 25-byte original 8x8 sprite cells.
#[no_mangle]
pub extern "C" fn rs_pose_version() -> c_uint { 2 }

#[no_mangle]
pub unsafe extern "C" fn rs_pose_arm(
    handle: *mut RsPlayer, active: c_uchar, err: *mut c_char, err_len: usize,
) -> c_int {
    with_player(handle, err, err_len, |emu| { emu.pose_arm(active != 0); 0 })
}

#[no_mangle]
pub unsafe extern "C" fn rs_pose_frame(
    handle: *mut RsPlayer, out: *mut c_uchar, out_len: usize,
    err: *mut c_char, err_len: usize,
) -> c_int {
    if out.is_null() || out_len < 4 + 128 * 25 {
        write_err(err, err_len, "pose output buffer is too small");
        return -1;
    }
    with_player(handle, err, err_len, |emu| {
        let (x, y) = emu.pose_camera().unwrap_or((65535, 65535));
        ptr::copy_nonoverlapping(x.to_le_bytes().as_ptr(), out, 2);
        ptr::copy_nonoverlapping(y.to_le_bytes().as_ptr(), out.add(2), 2);
        let mut count = 0;
        for cell in emu.pose_cells().take(128) {
            let target = out.add(4 + count * 25);
            *target = cell.x; *target.add(1) = cell.y; *target.add(2) = cell.flags;
            *target.add(3) = 0;
            ptr::copy_nonoverlapping(cell.index.to_le_bytes().as_ptr(), target.add(4), 2);
            ptr::copy_nonoverlapping(cell.palette.as_ptr(), target.add(6), 3);
            ptr::copy_nonoverlapping(cell.pattern.as_ptr(), target.add(9), 16);
            count += 1;
        }
        count as c_int
    })
}

/// Copy 16 CHR pattern bytes for tile `index` (8 low rows, then 8 high).
#[no_mangle]
pub unsafe extern "C" fn rs_tile_pattern(
    player: *mut RsPlayer,
    index: u32,
    out: *mut c_uchar,
) -> c_int {
    if player.is_null() || out.is_null() || index >= 512 {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let game = &mut *player;
        let base = (index as u16) * 16;
        for row in 0..16u16 {
            *out.add(row as usize) = game.emu.ppu_read_debug(base.wrapping_add(row));
        }
        0 as c_int
    }));
    result.unwrap_or(-1)
}

/// Standard 2C02 palette as 64 RGB triples (192 bytes).
#[no_mangle]
pub unsafe extern "C" fn rs_standard_palette(out: *mut c_uchar) -> c_int {
    if out.is_null() {
        return -1;
    }
    for (index, value) in mesen_core::types::NES_PALETTE.iter().enumerate() {
        *out.add(index * 3) = ((value >> 16) & 0xFF) as u8;
        *out.add(index * 3 + 1) = ((value >> 8) & 0xFF) as u8;
        *out.add(index * 3 + 2) = (value & 0xFF) as u8;
    }
    0
}

/// Mapper ID of the loaded cartridge. -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn rs_mapper_id(handle: *mut RsPlayer, err: *mut c_char, err_len: usize) -> c_int {
    with_player(handle, err, err_len, |emu| emu.mapper_id() as c_int)
}

/// Library ABI version. Python refuses to load a DLL whose ABI it cannot speak.
#[no_mangle]
pub extern "C" fn rs_abi_version() -> c_uint {
    1
}

/// Opaque handle size probe (always succeeds; useful for smoke tests).
#[no_mangle]
pub unsafe extern "C" fn rs_is_alive(handle: *const RsPlayer) -> c_int {
    (!handle.is_null()) as c_int
}
