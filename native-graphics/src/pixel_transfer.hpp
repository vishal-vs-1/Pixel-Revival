#ifndef REVIVAL_GRAPHICS_PIXEL_TRANSFER_HPP
#define REVIVAL_GRAPHICS_PIXEL_TRANSFER_HPP

#include <cstdint>
#include <vector>

#include "revival_graphics.h"

namespace revival::graphics {

RvResult convert_rows_to_gl_rgba(
	const RvFrameDesc& desc, uint32_t first_row, uint32_t row_count,
	uint32_t source_stride_bytes, const uint8_t* source, uint32_t source_bytes,
	std::vector<uint8_t>* bottom_up_rgba) noexcept;

} // namespace revival::graphics

#endif
