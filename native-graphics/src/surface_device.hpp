#ifndef REVIVAL_GRAPHICS_SURFACE_DEVICE_HPP
#define REVIVAL_GRAPHICS_SURFACE_DEVICE_HPP

#include "surface_ops.hpp"

namespace revival::graphics {

class OpenGlSurfaceDevice final : public SurfaceRenderer {
public:
	OpenGlSurfaceDevice(TextureDevice& owner_device, const GlFunctionTable& functions) noexcept;

	RvResult clear_transparent(const TextureSurfaceSnapshot& destination) noexcept override;
	RvResult copy_surface(
		const TextureAllocation& source, const TextureAllocation& destination) noexcept override;
	RvResult fill(
		const TextureSurfaceSnapshot& destination, const SurfaceRect& rectangle,
		const SurfaceRect& clip, uint32_t argb) noexcept override;
	RvResult blit(
		const TextureAllocation& source, uint32_t source_logical_width,
		uint32_t source_logical_height, uint32_t source_scale,
		const TextureSurfaceSnapshot& destination, const SurfaceRect& source_rectangle,
		const SurfaceRect& destination_rectangle, const SurfaceRect& clip,
		uint32_t transform, SurfaceBlend blend) noexcept override;

private:
	TextureDevice& owner_device_;
	const GlFunctionTable& functions_;
};

} // namespace revival::graphics

#endif
