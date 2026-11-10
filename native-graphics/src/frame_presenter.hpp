#ifndef REVIVAL_GRAPHICS_FRAME_PRESENTER_HPP
#define REVIVAL_GRAPHICS_FRAME_PRESENTER_HPP

#include <array>
#include <cstdint>

#include "revival_graphics.h"
#include "scale2x.hpp"
#include "surface_protocol.hpp"
#include "texture_store.hpp"

namespace revival::graphics {

struct FrameViewportRect {
	uint32_t x = 0;
	uint32_t y = 0;
	uint32_t width = 0;
	uint32_t height = 0;
};

struct CommittedFrameSnapshot {
	bool valid = false;
	RvSurfaceHandle surface = 0;
	uint32_t logical_width = 0;
	uint32_t logical_height = 0;
	uint32_t storage_scale = 0;
	uint64_t source_generation = 0;
	uint64_t frame_sequence = 0;
	SurfaceRect content{};
};

RvResult validate_viewport(const RvViewport& viewport) noexcept;
RvResult calculate_viewport_rect(const RvViewport& viewport,
	FrameViewportRect* out) noexcept;

class FramePresenter {
public:
	FramePresenter(TextureStore& store, SurfaceScaler& scaler) noexcept;
	RvResult commit(RvSurfaceHandle source, const SurfaceRect& content,
		uint64_t frame_sequence, uint32_t desired_scale,
		RvSurfaceHandle candidate_surface, ScaleReport* scale_report) noexcept;
	RvResult clear() noexcept;
	RvResult current(CommittedFrameSnapshot* out) const noexcept;
	uint64_t last_frame_sequence() const noexcept;
	void destroy() noexcept;

private:
	TextureStore& store_;
	SurfaceScaler& scaler_;
	CommittedFrameSnapshot front_{};
	CommittedFrameSnapshot back_{};
	// Clearing the visible frame must not allow a later frame to reuse an old sequence.
	uint64_t last_frame_sequence_ = 0;
};

class FrameOutputRenderer {
public:
	virtual ~FrameOutputRenderer() = default;
	virtual RvResult render(const CommittedFrameSnapshot& frame,
		const std::array<RvSurfaceHandle, 2>& overlays, const TextureStore& store,
		const RvViewport& viewport) noexcept = 0;
};

class OpenGlFrameOutputRenderer final : public FrameOutputRenderer {
public:
	explicit OpenGlFrameOutputRenderer(const GlFunctionTable& functions) noexcept;
	RvResult render(const CommittedFrameSnapshot& frame,
		const std::array<RvSurfaceHandle, 2>& overlays, const TextureStore& store,
		const RvViewport& viewport) noexcept override;

private:
	const GlFunctionTable& functions_;
};

} // namespace revival::graphics

#endif
