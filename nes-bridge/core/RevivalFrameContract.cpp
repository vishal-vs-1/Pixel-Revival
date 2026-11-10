#include "RevivalFrameContract.hpp"

#include <cmath>
#include <limits>

namespace revival::nes {
namespace {

uint32_t scale_from_double(double value) noexcept
{
	if(!std::isfinite(value)) return 0;
	if(std::fabs(value - 1.0) < 0.0001) return RV_SCALE_ORIGINAL;
	if(std::fabs(value - 2.0) < 0.0001) return RV_SCALE_2X;
	if(std::fabs(value - 4.0) < 0.0001) return RV_SCALE_4X;
	return 0;
}

bool safe_bytes(uint32_t width, uint32_t height, uint32_t* out_bytes) noexcept
{
	if(out_bytes == nullptr || width == 0 || height == 0) return false;
	const uint64_t bytes = static_cast<uint64_t>(width) * height * 4u;
	if(bytes > std::numeric_limits<uint32_t>::max() || bytes > RV_MAX_QUEUED_BYTES) {
		return false;
	}
	*out_bytes = static_cast<uint32_t>(bytes);
	return true;
}

void common_fields(RvFrameDesc* out) noexcept
{
	*out = RvFrameDesc{};
	out->struct_bytes = RV_FRAME_DESC_BYTES;
	out->abi_version = RV_ABI_VERSION;
	out->pixel_format = RV_PIXEL_FORMAT_BGRA8;
	out->alpha_mode = RV_ALPHA_OPAQUE;
	out->origin = RV_ORIGIN_TOP_LEFT;
}

} // namespace

RvResult build_frame_desc(const FrameSource& source, RvFrameDesc* out,
	uint32_t* out_bytes) noexcept
{
	if(out == nullptr || out_bytes == nullptr || source.pixels == nullptr ||
		source.physical_width == 0 || source.physical_height == 0 || source.sequence == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint32_t source_scale = scale_from_double(source.scale);
	if(source_scale == 0 || source.physical_width % source_scale != 0 ||
		source.physical_height % source_scale != 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint32_t logical_width = source.physical_width / source_scale;
	const uint32_t logical_height = source.physical_height / source_scale;
	if(!safe_bytes(source.physical_width, source.physical_height, out_bytes) ||
		static_cast<uint64_t>(logical_width) * logical_height > RV_MAX_LOGICAL_SOURCE_PIXELS) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	common_fields(out);
	out->frame_sequence = source.sequence;
	out->physical_width = source.physical_width;
	out->physical_height = source.physical_height;
	out->physical_stride_bytes = source.physical_width * 4u;
	out->logical_width = logical_width;
	out->logical_height = logical_height;
	out->source_scale = source_scale;
	out->desired_scale = source_scale == RV_SCALE_ORIGINAL ? RV_SCALE_2X : source_scale;
	return RV_RESULT_OK;
}

RvResult build_overlay_desc(const OverlaySource& source, RvFrameDesc* out,
	uint32_t* out_bytes) noexcept
{
	if(out == nullptr || out_bytes == nullptr || source.pixels == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(!safe_bytes(source.physical_width, source.physical_height, out_bytes)) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	common_fields(out);
	// Mesen's HUD compositor stores blended colors premultiplied for hardware blending.
	out->alpha_mode = RV_ALPHA_PREMULTIPLIED;
	out->physical_width = source.physical_width;
	out->physical_height = source.physical_height;
	out->physical_stride_bytes = source.physical_width * 4u;
	out->logical_width = source.physical_width;
	out->logical_height = source.physical_height;
	out->source_scale = RV_SCALE_ORIGINAL;
	out->desired_scale = RV_SCALE_ORIGINAL;
	return RV_RESULT_OK;
}

RvResult build_clear_overlay_desc(RvFrameDesc* out) noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	common_fields(out);
	out->source_scale = RV_SCALE_ORIGINAL;
	out->desired_scale = RV_SCALE_ORIGINAL;
	return RV_RESULT_OK;
}

} // namespace revival::nes
