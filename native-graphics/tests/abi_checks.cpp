#include <cstdint>
#include <iostream>
#include <array>

#include "revival_graphics.h"

int main()
{
	if(rv_get_abi_version() != RV_ABI_VERSION) {
		std::cerr << "ABI version mismatch.\n";
		return 1;
	}
	RvBuildInfo info{};
	if(rv_get_build_info(&info, RV_BUILD_INFO_BYTES) != RV_RESULT_OK ||
		info.struct_bytes != RV_BUILD_INFO_BYTES || info.abi_version != RV_ABI_VERSION ||
		info.feature_bits != (RV_FEATURE_SURFACE_COMMANDS | RV_FEATURE_COMPLETED_FRAMES | RV_FEATURE_SCALE2X |
			RV_FEATURE_ACTUAL_DEVICE | RV_FEATURE_SURFACE_SCALE)) {
		std::cerr << "Build-info handshake mismatch.\n";
		return 1;
	}
	for(uint8_t byte : info.build_id) {
		if(!((byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f'))) {
			std::cerr << "Build ID is not lowercase hexadecimal.\n";
			return 1;
		}
	}

	RvContextHandle handle = 123;
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = 1;
	config.texture_budget_bytes = 128ull * 1024 * 1024;
	config.max_queued_bytes = 1024;
	config.max_queued_ops = 8;
	config.max_pending_frames = 1;
	config.detail_scale = RV_SCALE_2X;
	config.input_mode = RV_INPUT_MODE_COMMANDS;
	if(rv_attach_current_context(&config, RV_ATTACH_CONFIG_BYTES, &handle) != RV_RESULT_NO_CONTEXT ||
		handle != 0) {
		std::cerr << "Attach without a current WGL context must fail closed.\n";
		return 1;
	}
	config.max_queued_ops = RV_MAX_QUEUED_OPS + 1;
	if(rv_attach_current_context(&config, RV_ATTACH_CONFIG_BYTES, &handle) !=
		RV_RESULT_INVALID_ARGUMENT || handle != 0) {
		std::cerr << "Invalid attach caps must be rejected before context use.\n";
		return 1;
	}
	if(rv_submit(0, nullptr, 0) != RV_RESULT_STALE_HANDLE ||
		rv_begin_close(0) != RV_RESULT_STALE_HANDLE) {
		std::cerr << "Zero handle must be stale for submit and close.\n";
		return 1;
	}
	RvPumpStats pump{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	RvViewport viewport{};
	viewport.struct_bytes = RV_VIEWPORT_BYTES;
	viewport.abi_version = RV_ABI_VERSION;
	RvReadbackRect rect{};
	rect.struct_bytes = RV_READBACK_RECT_BYTES;
	rect.abi_version = RV_ABI_VERSION;
	std::array<char, 8> text{};
	if(rv_pump(0, 0, &pump) != RV_RESULT_STALE_HANDLE ||
		rv_present(0, &viewport, RV_VIEWPORT_BYTES) != RV_RESULT_STALE_HANDLE ||
		rv_clear_frame(0) != RV_RESULT_STALE_HANDLE ||
		rv_get_device_info(0, text.data(), static_cast<uint32_t>(text.size()),
			text.data(), static_cast<uint32_t>(text.size()), text.data(),
			static_cast<uint32_t>(text.size())) != RV_RESULT_STALE_HANDLE ||
		rv_readback_for_check(0, 1, &rect, RV_READBACK_RECT_BYTES, nullptr, 0, 0) !=
			RV_RESULT_STALE_HANDLE) {
		std::cerr << "New frame APIs must reject an unknown context before accessing graphics state.\n";
		return 1;
	}
	std::cout << "Native graphics ABI checks passed without a GL window.\n";
	return 0;
}
