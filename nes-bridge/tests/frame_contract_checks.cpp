#include <array>
#include <cassert>
#include <cstdint>

#include "RevivalFrameContract.hpp"

int main()
{
	static std::array<uint32_t, 256 * 240> pixels{};
	revival::nes::FrameSource native{pixels.data(), 256, 240, 1.0, 1};
	RvFrameDesc desc{};
	uint32_t bytes = 0;
	assert(revival::nes::build_frame_desc(native, &desc, &bytes) == RV_RESULT_OK);
	assert(desc.pixel_format == RV_PIXEL_FORMAT_BGRA8);
	assert(desc.source_scale == RV_SCALE_ORIGINAL);
	assert(desc.desired_scale == RV_SCALE_2X);
	assert(desc.logical_width == 256 && desc.logical_height == 240);
	assert(bytes == 256u * 240u * 4u);

	static std::array<uint32_t, 512 * 480> hd_pixels{};
	revival::nes::FrameSource composed{hd_pixels.data(), 512, 480, 2.0, 2};
	assert(revival::nes::build_frame_desc(composed, &desc, &bytes) == RV_RESULT_OK);
	assert(desc.source_scale == RV_SCALE_2X && desc.desired_scale == RV_SCALE_2X);
	assert(desc.logical_width == 256 && desc.logical_height == 240);

	revival::nes::FrameSource bad_scale{pixels.data(), 256, 240, 3.0, 3};
	assert(revival::nes::build_frame_desc(bad_scale, &desc, &bytes) == RV_RESULT_INVALID_ARGUMENT);
	revival::nes::OverlaySource hud{pixels.data(), 256, 240};
	assert(revival::nes::build_overlay_desc(hud, &desc, &bytes) == RV_RESULT_OK);
	assert(desc.frame_sequence == 0 && desc.alpha_mode == RV_ALPHA_PREMULTIPLIED);
	assert(revival::nes::build_clear_overlay_desc(&desc) == RV_RESULT_OK);
	assert(desc.physical_width == 0 && desc.physical_height == 0);
	return 0;
}
