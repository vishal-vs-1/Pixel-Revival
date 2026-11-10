#ifndef REVIVAL_GRAPHICS_H
#define REVIVAL_GRAPHICS_H

#include <stdint.h>

#if defined(_WIN32)
#if defined(REVIVAL_GRAPHICS_BUILD)
#define RV_API __declspec(dllexport)
#else
#define RV_API __declspec(dllimport)
#endif
#define RV_CALL __cdecl
#else
#define RV_API
#define RV_CALL
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define RV_ABI_VERSION 1u

#define RV_PACKET_MAGIC UINT32_C(0x31475652)
#define RV_PACKET_HEADER_BYTES 32u
#define RV_COMMAND_HEADER_BYTES 8u
#define RV_MAX_PACKET_BYTES 8388608u
#define RV_MAX_QUEUED_BYTES 67108864u
#define RV_MAX_QUEUED_OPS 8192u
#define RV_MAX_PENDING_FRAMES 2u
#define RV_MAX_LOGICAL_SOURCE_PIXELS 4194304u
#define RV_MAX_ASPECT_COMPONENT 16384u
#define RV_MAX_TEXTURE_BUDGET_BYTES UINT64_C(536870912)

#define RV_BUILD_INFO_BYTES 76u
#define RV_ATTACH_CONFIG_BYTES 48u
#define RV_FRAME_DESC_BYTES 56u
#define RV_VIEWPORT_BYTES 32u
#define RV_READBACK_RECT_BYTES 32u
#define RV_PUMP_STATS_BYTES 40u
#define RV_STATS_BYTES 64u

#define RV_INPUT_MODE_COMMANDS 1u
#define RV_INPUT_MODE_COMPLETED_FRAMES 2u

#define RV_SCALE_ORIGINAL 1u
#define RV_SCALE_2X 2u
#define RV_SCALE_4X 4u

#define RV_PIXEL_FORMAT_RGBA8 1u
#define RV_PIXEL_FORMAT_BGRA8 2u

#define RV_ALPHA_OPAQUE 1u
#define RV_ALPHA_STRAIGHT 2u
#define RV_ALPHA_PREMULTIPLIED 3u

#define RV_ORIGIN_TOP_LEFT 1u

#define RV_FILTER_NEAREST 1u
#define RV_FILTER_LINEAR 2u

#define RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT 1u

#define RV_FEATURE_SURFACE_COMMANDS UINT32_C(0x00000001)
#define RV_FEATURE_COMPLETED_FRAMES UINT32_C(0x00000002)
#define RV_FEATURE_SCALE2X UINT32_C(0x00000004)
#define RV_FEATURE_ACTUAL_DEVICE UINT32_C(0x00000008)
#define RV_FEATURE_SURFACE_SCALE UINT32_C(0x00000010)

#define RV_RESULT_OK 0u
#define RV_RESULT_INVALID_ARGUMENT 1u
#define RV_RESULT_UNSUPPORTED_ABI 2u
#define RV_RESULT_BAD_PACKET 3u
#define RV_RESULT_QUEUE_FULL 4u
#define RV_RESULT_WRONG_THREAD 5u
#define RV_RESULT_STALE_HANDLE 6u
#define RV_RESULT_RESOURCE_LIMIT 7u
#define RV_RESULT_NO_CONTEXT 8u
#define RV_RESULT_GL_FAILURE 9u
#define RV_RESULT_CLOSING 10u
#define RV_RESULT_UNSUPPORTED_OPERATION 11u
#define RV_RESULT_INTERNAL_ERROR 12u

#define RV_OP_CREATE_SURFACE 1u
#define RV_OP_BEGIN_UPLOAD 2u
#define RV_OP_UPLOAD_ROWS 3u
#define RV_OP_END_UPLOAD 4u
#define RV_OP_FILL 5u
#define RV_OP_BLIT 6u
#define RV_OP_COPY 7u
#define RV_OP_DESTROY_SURFACE 8u
#define RV_OP_COMMIT_FRAME 9u
#define RV_OP_CLEAR_FRAME 10u
#define RV_OP_SCALE_SURFACE 11u

typedef uint32_t RvResult;
typedef uint64_t RvContextHandle;
typedef uint64_t RvSurfaceHandle;

typedef struct RvBuildInfo {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint8_t build_id[64];
	uint32_t feature_bits;
} RvBuildInfo;

typedef struct RvAttachConfig {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint64_t session_epoch;
	uint64_t texture_budget_bytes;
	uint32_t max_queued_bytes;
	uint32_t max_queued_ops;
	uint32_t max_pending_frames;
	uint32_t detail_scale;
	uint32_t input_mode;
	uint32_t reserved_zero;
} RvAttachConfig;

typedef struct RvFrameDesc {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint64_t frame_sequence;
	uint32_t physical_width;
	uint32_t physical_height;
	uint32_t physical_stride_bytes;
	uint32_t logical_width;
	uint32_t logical_height;
	uint32_t source_scale;
	uint32_t desired_scale;
	uint32_t pixel_format;
	uint32_t alpha_mode;
	uint32_t origin;
} RvFrameDesc;

typedef struct RvViewport {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint32_t framebuffer_width;
	uint32_t framebuffer_height;
	uint32_t aspect_num;
	uint32_t aspect_den;
	uint32_t filter;
	uint32_t fit;
} RvViewport;

typedef struct RvReadbackRect {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint32_t x;
	uint32_t y;
	uint32_t width;
	uint32_t height;
	uint32_t output_format;
	uint32_t output_alpha_mode;
} RvReadbackRect;

typedef struct RvPumpStats {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint32_t operations_processed;
	uint32_t frames_completed;
	uint32_t queued_bytes;
	uint32_t queued_ops;
	uint64_t uploaded_bytes;
	uint64_t elapsed_ns;
} RvPumpStats;

typedef struct RvStats {
	uint32_t struct_bytes;
	uint32_t abi_version;
	uint64_t packets_submitted;
	uint64_t packets_rejected;
	uint64_t operations_executed;
	uint64_t bytes_uploaded;
	uint64_t frames_presented;
	uint32_t queued_bytes;
	uint32_t queued_ops;
	uint32_t pending_frames;
	uint32_t reserved_zero;
} RvStats;

RV_API uint32_t RV_CALL rv_get_abi_version(void);
RV_API RvResult RV_CALL rv_get_build_info(RvBuildInfo* out, uint32_t out_bytes);
RV_API RvResult RV_CALL rv_attach_current_context(
	const RvAttachConfig* config, uint32_t config_bytes, RvContextHandle* out);
RV_API RvResult RV_CALL rv_submit(RvContextHandle context, const uint8_t* packet, uint32_t bytes);
RV_API RvResult RV_CALL rv_submit_completed_frame(
	RvContextHandle context, const RvFrameDesc* desc, uint32_t desc_bytes,
	const uint8_t* pixels, uint32_t pixel_bytes);
RV_API RvResult RV_CALL rv_submit_overlay(
	RvContextHandle context, uint32_t slot, const RvFrameDesc* desc, uint32_t desc_bytes,
	const uint8_t* pixels, uint32_t pixel_bytes);
RV_API RvResult RV_CALL rv_pump(RvContextHandle context, uint64_t budget_ns, RvPumpStats* out);
RV_API RvResult RV_CALL rv_present(
	RvContextHandle context, const RvViewport* viewport, uint32_t viewport_bytes);
RV_API RvResult RV_CALL rv_clear_frame(RvContextHandle context);
RV_API RvResult RV_CALL rv_get_device_info(RvContextHandle context,
	char* vendor, uint32_t vendor_capacity, char* renderer, uint32_t renderer_capacity,
	char* version, uint32_t version_capacity);
RV_API RvResult RV_CALL rv_get_stats(RvContextHandle context, RvStats* out, uint32_t stats_bytes);
RV_API RvResult RV_CALL rv_readback_for_check(
	RvContextHandle context, RvSurfaceHandle surface, const RvReadbackRect* rect,
	uint32_t rect_bytes, uint8_t* destination, uint32_t destination_bytes,
	uint32_t destination_stride_bytes);
RV_API RvResult RV_CALL rv_begin_close(RvContextHandle context);
RV_API RvResult RV_CALL rv_destroy_on_owner(RvContextHandle context);

#ifdef __cplusplus
}
#endif

#endif
