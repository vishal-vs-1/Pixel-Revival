#ifndef REVIVAL_GRAPHICS_FRAME_DESC_HPP
#define REVIVAL_GRAPHICS_FRAME_DESC_HPP

#include "revival_graphics.h"

namespace revival::graphics {

RvResult validate_frame_desc(const RvFrameDesc& desc, uint32_t pixel_bytes) noexcept;

} // namespace revival::graphics

#endif
