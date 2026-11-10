#ifndef REVIVAL_GRAPHICS_SCALE2X_HPP
#define REVIVAL_GRAPHICS_SCALE2X_HPP

#include <cstddef>
#include <cstdint>
#include <vector>

#include "gl_functions.hpp"
#include "surface_ops.hpp"
#include "texture_store.hpp"

namespace revival::graphics {

enum class ScalePath : uint32_t {
	GpuScale2x = 1,
	CompatibilitySampling = 2,
	AlreadyDetailed = 3
};

enum class ScaleFallbackReason : uint32_t {
	None = 0,
	ShaderUnavailable = 1,
	ShaderCompileFailed = 2,
	ProgramLinkFailed = 3,
	TemporaryResourceLimit = 4,
	GpuExecutionFailed = 5
};

struct ScaleReport {
	ScalePath path = ScalePath::CompatibilitySampling;
	ScaleFallbackReason fallback_reason = ScaleFallbackReason::None;
	uint32_t source_scale = 0;
	uint32_t requested_scale = 0;
	uint32_t passes = 0;
};

struct Scale2xStatistics {
	uint64_t completed_gpu_passes = 0;
	uint64_t gpu_scaled_surfaces = 0;
	uint64_t compatibility_surfaces = 0;
	uint64_t shader_failures = 0;
};

class SurfaceScaler {
public:
	virtual ~SurfaceScaler() = default;
	virtual RvResult apply(RvSurfaceHandle source, RvSurfaceHandle destination,
		ScaleReport* report) noexcept = 0;
};

RvResult scale2x_reference_rgba(
	const uint8_t* source, size_t source_bytes, uint32_t width, uint32_t height,
	std::vector<uint8_t>* output) noexcept;

class Scale2xRenderer final : public SurfaceScaler {
public:
	Scale2xRenderer(TextureStore& store, SurfaceOperations& operations,
		const GlFunctionTable& functions) noexcept;

	RvResult apply(RvSurfaceHandle source, RvSurfaceHandle destination,
		ScaleReport* report) noexcept override;
	void destroy() noexcept;
	Scale2xStatistics statistics() const noexcept;

private:
	RvResult ensure_program() noexcept;
	RvResult draw_pass(const TextureAllocation& source, const TextureAllocation& destination,
		uint32_t source_width, uint32_t source_height) noexcept;
	RvResult compatibility_copy(RvSurfaceHandle source, RvSurfaceHandle destination) noexcept;

	TextureStore& store_;
	SurfaceOperations& operations_;
	const GlFunctionTable& functions_;
	uint32_t program_ = 0;
	int32_t source_texture_uniform_ = -1;
	int32_t source_size_uniform_ = -1;
	int32_t output_size_uniform_ = -1;
	bool shader_failed_ = false;
	ScaleFallbackReason shader_failure_reason_ = ScaleFallbackReason::None;
	Scale2xStatistics statistics_{};
};

} // namespace revival::graphics

#endif
