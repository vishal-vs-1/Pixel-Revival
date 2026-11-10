#ifndef REVIVAL_GRAPHICS_DIAGNOSTICS_HPP
#define REVIVAL_GRAPHICS_DIAGNOSTICS_HPP

#include <cstdint>

#include "gl_functions.hpp"
#include "revival_graphics.h"
#include "texture_store.hpp"

namespace revival::graphics {

class GraphicsDiagnostics {
public:
	explicit GraphicsDiagnostics(const GlFunctionTable& functions) noexcept;
	RvResult device_strings(char* vendor, uint32_t vendor_capacity,
		char* renderer, uint32_t renderer_capacity,
		char* version, uint32_t version_capacity) const noexcept;
	RvResult readback(const TextureStore& store, RvSurfaceHandle surface,
		const RvReadbackRect& rect, uint32_t rect_bytes, uint8_t* destination,
		uint32_t destination_bytes, uint32_t destination_stride_bytes) const noexcept;

private:
	const GlFunctionTable& functions_;
};

} // namespace revival::graphics

#endif
