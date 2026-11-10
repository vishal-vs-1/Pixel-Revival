#include "frame_desc.hpp"

namespace revival::graphics {
namespace {

bool valid_scale(uint32_t scale) noexcept
{
	return scale == RV_SCALE_ORIGINAL || scale == RV_SCALE_2X || scale == RV_SCALE_4X;
}

} // namespace

RvResult validate_frame_desc(const RvFrameDesc& desc, uint32_t pixel_bytes) noexcept
{
	if(desc.struct_bytes != RV_FRAME_DESC_BYTES || desc.abi_version != RV_ABI_VERSION) {
		return RV_RESULT_UNSUPPORTED_ABI;
	}
	if(!valid_scale(desc.source_scale) || !valid_scale(desc.desired_scale) ||
		(desc.pixel_format != RV_PIXEL_FORMAT_RGBA8 && desc.pixel_format != RV_PIXEL_FORMAT_BGRA8) ||
		(desc.alpha_mode != RV_ALPHA_OPAQUE && desc.alpha_mode != RV_ALPHA_STRAIGHT &&
			desc.alpha_mode != RV_ALPHA_PREMULTIPLIED) || desc.origin != RV_ORIGIN_TOP_LEFT) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(desc.physical_width == 0 || desc.physical_height == 0 ||
		desc.logical_width == 0 || desc.logical_height == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(static_cast<uint64_t>(desc.logical_width) * desc.logical_height >
		RV_MAX_LOGICAL_SOURCE_PIXELS) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	if(static_cast<uint64_t>(desc.logical_width) * desc.source_scale != desc.physical_width ||
		static_cast<uint64_t>(desc.logical_height) * desc.source_scale != desc.physical_height) {
		return RV_RESULT_INVALID_ARGUMENT;
	}

	const uint64_t minimum_stride = static_cast<uint64_t>(desc.physical_width) * 4;
	if(minimum_stride > UINT32_MAX || desc.physical_stride_bytes < minimum_stride) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint64_t required_bytes = static_cast<uint64_t>(desc.physical_stride_bytes) * desc.physical_height;
	if(required_bytes > UINT32_MAX || required_bytes > RV_MAX_QUEUED_BYTES) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	if(pixel_bytes != required_bytes) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	return RV_RESULT_OK;
}

} // namespace revival::graphics
