#pragma once

#include <cstdint>

#include "revival_graphics.h"

namespace revival::nes {

// This is the only conversion boundary between Mesen's uint32_t frame and the
// common renderer.  The adapter treats the Windows little-endian word as BGRA8
// and rejects dimensions that cannot be represented without guessing.
struct FrameSource {
	const void* pixels = nullptr;
	uint32_t physical_width = 0;
	uint32_t physical_height = 0;
	double scale = 1.0;
	uint64_t sequence = 0;
};

struct OverlaySource {
	const void* pixels = nullptr;
	uint32_t physical_width = 0;
	uint32_t physical_height = 0;
};

RvResult build_frame_desc(const FrameSource& source, RvFrameDesc* out,
	uint32_t* out_bytes) noexcept;
RvResult build_overlay_desc(const OverlaySource& source, RvFrameDesc* out,
	uint32_t* out_bytes) noexcept;
RvResult build_clear_overlay_desc(RvFrameDesc* out) noexcept;

} // namespace revival::nes
