#include <cstddef>
#include <cstdint>

#include "revival_graphics.h"

static_assert(sizeof(uint32_t) == 4, "v1 requires 32-bit uint32_t");
static_assert(sizeof(uint64_t) == 8, "v1 requires 64-bit uint64_t");
static_assert(sizeof(RvContextHandle) == 8, "context handles are 64-bit");
static_assert(sizeof(RvSurfaceHandle) == 8, "surface handles are 64-bit");

#define RV_ASSERT_SIZE(type, expected) \
	static_assert(sizeof(type) == (expected), #type " size differs from graphics-v1.md")
#define RV_ASSERT_OFFSET(type, field, expected) \
	static_assert(offsetof(type, field) == (expected), #type "." #field " offset differs from graphics-v1.md")

RV_ASSERT_SIZE(RvBuildInfo, RV_BUILD_INFO_BYTES);
RV_ASSERT_OFFSET(RvBuildInfo, struct_bytes, 0);
RV_ASSERT_OFFSET(RvBuildInfo, abi_version, 4);
RV_ASSERT_OFFSET(RvBuildInfo, build_id, 8);
RV_ASSERT_OFFSET(RvBuildInfo, feature_bits, 72);

RV_ASSERT_SIZE(RvAttachConfig, RV_ATTACH_CONFIG_BYTES);
RV_ASSERT_OFFSET(RvAttachConfig, struct_bytes, 0);
RV_ASSERT_OFFSET(RvAttachConfig, abi_version, 4);
RV_ASSERT_OFFSET(RvAttachConfig, session_epoch, 8);
RV_ASSERT_OFFSET(RvAttachConfig, texture_budget_bytes, 16);
RV_ASSERT_OFFSET(RvAttachConfig, max_queued_bytes, 24);
RV_ASSERT_OFFSET(RvAttachConfig, max_queued_ops, 28);
RV_ASSERT_OFFSET(RvAttachConfig, max_pending_frames, 32);
RV_ASSERT_OFFSET(RvAttachConfig, detail_scale, 36);
RV_ASSERT_OFFSET(RvAttachConfig, input_mode, 40);
RV_ASSERT_OFFSET(RvAttachConfig, reserved_zero, 44);

RV_ASSERT_SIZE(RvFrameDesc, RV_FRAME_DESC_BYTES);
RV_ASSERT_OFFSET(RvFrameDesc, struct_bytes, 0);
RV_ASSERT_OFFSET(RvFrameDesc, abi_version, 4);
RV_ASSERT_OFFSET(RvFrameDesc, frame_sequence, 8);
RV_ASSERT_OFFSET(RvFrameDesc, physical_width, 16);
RV_ASSERT_OFFSET(RvFrameDesc, physical_height, 20);
RV_ASSERT_OFFSET(RvFrameDesc, physical_stride_bytes, 24);
RV_ASSERT_OFFSET(RvFrameDesc, logical_width, 28);
RV_ASSERT_OFFSET(RvFrameDesc, logical_height, 32);
RV_ASSERT_OFFSET(RvFrameDesc, source_scale, 36);
RV_ASSERT_OFFSET(RvFrameDesc, desired_scale, 40);
RV_ASSERT_OFFSET(RvFrameDesc, pixel_format, 44);
RV_ASSERT_OFFSET(RvFrameDesc, alpha_mode, 48);
RV_ASSERT_OFFSET(RvFrameDesc, origin, 52);

RV_ASSERT_SIZE(RvViewport, RV_VIEWPORT_BYTES);
RV_ASSERT_OFFSET(RvViewport, struct_bytes, 0);
RV_ASSERT_OFFSET(RvViewport, abi_version, 4);
RV_ASSERT_OFFSET(RvViewport, framebuffer_width, 8);
RV_ASSERT_OFFSET(RvViewport, framebuffer_height, 12);
RV_ASSERT_OFFSET(RvViewport, aspect_num, 16);
RV_ASSERT_OFFSET(RvViewport, aspect_den, 20);
RV_ASSERT_OFFSET(RvViewport, filter, 24);
RV_ASSERT_OFFSET(RvViewport, fit, 28);

RV_ASSERT_SIZE(RvReadbackRect, RV_READBACK_RECT_BYTES);
RV_ASSERT_OFFSET(RvReadbackRect, struct_bytes, 0);
RV_ASSERT_OFFSET(RvReadbackRect, abi_version, 4);
RV_ASSERT_OFFSET(RvReadbackRect, x, 8);
RV_ASSERT_OFFSET(RvReadbackRect, y, 12);
RV_ASSERT_OFFSET(RvReadbackRect, width, 16);
RV_ASSERT_OFFSET(RvReadbackRect, height, 20);
RV_ASSERT_OFFSET(RvReadbackRect, output_format, 24);
RV_ASSERT_OFFSET(RvReadbackRect, output_alpha_mode, 28);

RV_ASSERT_SIZE(RvPumpStats, RV_PUMP_STATS_BYTES);
RV_ASSERT_OFFSET(RvPumpStats, struct_bytes, 0);
RV_ASSERT_OFFSET(RvPumpStats, abi_version, 4);
RV_ASSERT_OFFSET(RvPumpStats, operations_processed, 8);
RV_ASSERT_OFFSET(RvPumpStats, frames_completed, 12);
RV_ASSERT_OFFSET(RvPumpStats, queued_bytes, 16);
RV_ASSERT_OFFSET(RvPumpStats, queued_ops, 20);
RV_ASSERT_OFFSET(RvPumpStats, uploaded_bytes, 24);
RV_ASSERT_OFFSET(RvPumpStats, elapsed_ns, 32);

RV_ASSERT_SIZE(RvStats, RV_STATS_BYTES);
RV_ASSERT_OFFSET(RvStats, struct_bytes, 0);
RV_ASSERT_OFFSET(RvStats, abi_version, 4);
RV_ASSERT_OFFSET(RvStats, packets_submitted, 8);
RV_ASSERT_OFFSET(RvStats, packets_rejected, 16);
RV_ASSERT_OFFSET(RvStats, operations_executed, 24);
RV_ASSERT_OFFSET(RvStats, bytes_uploaded, 32);
RV_ASSERT_OFFSET(RvStats, frames_presented, 40);
RV_ASSERT_OFFSET(RvStats, queued_bytes, 48);
RV_ASSERT_OFFSET(RvStats, queued_ops, 52);
RV_ASSERT_OFFSET(RvStats, pending_frames, 56);
RV_ASSERT_OFFSET(RvStats, reserved_zero, 60);
