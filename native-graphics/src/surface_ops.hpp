#ifndef REVIVAL_GRAPHICS_SURFACE_OPS_HPP
#define REVIVAL_GRAPHICS_SURFACE_OPS_HPP

#include <vector>

#include "surface_protocol.hpp"
#include "texture_store.hpp"

namespace revival::graphics {

class SurfaceRenderer {
public:
	virtual ~SurfaceRenderer() = default;
	virtual RvResult clear_transparent(const TextureSurfaceSnapshot& destination) noexcept = 0;
	virtual RvResult copy_surface(
		const TextureAllocation& source, const TextureAllocation& destination) noexcept = 0;
	virtual RvResult fill(
		const TextureSurfaceSnapshot& destination, const SurfaceRect& rectangle,
		const SurfaceRect& clip, uint32_t argb) noexcept = 0;
	virtual RvResult blit(
		const TextureAllocation& source, uint32_t source_logical_width,
		uint32_t source_logical_height, uint32_t source_scale,
		const TextureSurfaceSnapshot& destination, const SurfaceRect& source_rectangle,
		const SurfaceRect& destination_rectangle, const SurfaceRect& clip,
		uint32_t transform, SurfaceBlend blend) noexcept = 0;
};

class SurfaceOperations {
public:
	SurfaceOperations(TextureStore& store, SurfaceRenderer& renderer) noexcept;
	RvResult execute(const std::vector<SurfaceCommand>& commands) noexcept;

private:
	RvResult preflight(const std::vector<SurfaceCommand>& commands) const noexcept;
	RvResult execute_one(const SurfaceCommand& command) noexcept;

	TextureStore& store_;
	SurfaceRenderer& renderer_;
};

} // namespace revival::graphics

#endif
