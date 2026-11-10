#include <cstring>
#include <new>
#include <thread>

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>

#include "context.hpp"
#include "revival_graphics.h"

#ifndef RV_BUILD_ID_HEX
#error CMake must provide a build-input SHA-256 identifier.
#endif

static_assert(sizeof(RV_BUILD_ID_HEX) == 65, "build ID must be 64 hexadecimal bytes");

namespace {

template<typename Action>
RvResult guarded(Action&& action) noexcept
{
	try {
		return action();
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

void* current_gl_context() noexcept
{
	const HGLRC context = ::wglGetCurrentContext();
	return context != nullptr && ::wglGetCurrentDC() != nullptr ?
		reinterpret_cast<void*>(context) : nullptr;
}

std::shared_ptr<revival::graphics::ContextSession> find_context(RvContextHandle handle)
{
	return revival::graphics::global_context_registry().find(handle);
}

RvResult check_owner(const revival::graphics::ContextSession& context)
{
	return context.check_owner(std::this_thread::get_id(), current_gl_context());
}

} // namespace

extern "C" RV_API uint32_t RV_CALL rv_get_abi_version(void)
{
	return RV_ABI_VERSION;
}

extern "C" RV_API RvResult RV_CALL rv_get_build_info(RvBuildInfo* out, uint32_t out_bytes)
{
	return guarded([&]() -> RvResult {
		if(out == nullptr) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		if(out_bytes != RV_BUILD_INFO_BYTES) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}

		RvBuildInfo result{};
		result.struct_bytes = RV_BUILD_INFO_BYTES;
		result.abi_version = RV_ABI_VERSION;
		std::memcpy(result.build_id, RV_BUILD_ID_HEX, sizeof(result.build_id));
		result.feature_bits = RV_FEATURE_SURFACE_COMMANDS | RV_FEATURE_COMPLETED_FRAMES | RV_FEATURE_SCALE2X |
			RV_FEATURE_ACTUAL_DEVICE | RV_FEATURE_SURFACE_SCALE;
		*out = result;
		return RV_RESULT_OK;
	});
}

extern "C" RV_API RvResult RV_CALL rv_attach_current_context(
	const RvAttachConfig* config, uint32_t config_bytes, RvContextHandle* out)
{
	return guarded([&]() -> RvResult {
		if(config == nullptr || out == nullptr) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		*out = 0;
		if(config_bytes != RV_ATTACH_CONFIG_BYTES) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}
		const RvResult valid = revival::graphics::validate_attach_config(*config);
		if(valid != RV_RESULT_OK) {
			return valid;
		}
		const HGLRC native_context = ::wglGetCurrentContext();
		if(native_context == nullptr || ::wglGetCurrentDC() == nullptr) {
			return RV_RESULT_NO_CONTEXT;
		}

		revival::graphics::GlFunctionTable functions{};
		const char* missing_name = nullptr;
		if(!revival::graphics::load_required_gl_functions(
			revival::graphics::current_windows_gl_resolver(), &functions, &missing_name)) {
			return RV_RESULT_GL_FAILURE;
		}
		return revival::graphics::global_context_registry().create(
			*config, std::this_thread::get_id(), reinterpret_cast<void*>(native_context),
			functions, out);
	});
}

extern "C" RV_API RvResult RV_CALL rv_submit(
	RvContextHandle handle, const uint8_t* packet, uint32_t bytes)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(context->config().input_mode != RV_INPUT_MODE_COMMANDS) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return context->queue().submit_packet(packet, bytes);
	});
}

extern "C" RV_API RvResult RV_CALL rv_submit_completed_frame(
	RvContextHandle handle, const RvFrameDesc* desc, uint32_t desc_bytes,
	const uint8_t* pixels, uint32_t pixel_bytes)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(context->config().input_mode != RV_INPUT_MODE_COMPLETED_FRAMES) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return context->submit_completed_frame(desc, desc_bytes, pixels, pixel_bytes);
	});
}

extern "C" RV_API RvResult RV_CALL rv_submit_overlay(
	RvContextHandle handle, uint32_t slot, const RvFrameDesc* desc, uint32_t desc_bytes,
	const uint8_t* pixels, uint32_t pixel_bytes)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(context->config().input_mode != RV_INPUT_MODE_COMPLETED_FRAMES) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return context->submit_overlay(slot, desc, desc_bytes, pixels, pixel_bytes);
	});
}

extern "C" RV_API RvResult RV_CALL rv_pump(
	RvContextHandle handle, uint64_t budget_ns, RvPumpStats* out)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(out == nullptr) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		if(out->struct_bytes != RV_PUMP_STATS_BYTES || out->abi_version != RV_ABI_VERSION) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}
		if(context->queue().is_closing()) {
			return RV_RESULT_CLOSING;
		}
		const RvResult owner = check_owner(*context);
		if(owner != RV_RESULT_OK) {
			return owner;
		}
		return context->pump(budget_ns, out);
	});
}

extern "C" RV_API RvResult RV_CALL rv_present(
	RvContextHandle handle, const RvViewport* viewport, uint32_t viewport_bytes)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(viewport == nullptr) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		if(viewport_bytes != RV_VIEWPORT_BYTES || viewport->struct_bytes != RV_VIEWPORT_BYTES ||
			viewport->abi_version != RV_ABI_VERSION) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}
		if(context->queue().is_closing()) {
			return RV_RESULT_CLOSING;
		}
		const RvResult owner = check_owner(*context);
		if(owner != RV_RESULT_OK) {
			return owner;
		}
		return context->present(*viewport);
	});
}

extern "C" RV_API RvResult RV_CALL rv_clear_frame(RvContextHandle handle)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) return RV_RESULT_STALE_HANDLE;
		if(context->queue().is_closing()) return RV_RESULT_CLOSING;
		const RvResult owner = check_owner(*context);
		if(owner != RV_RESULT_OK) return owner;
		return context->clear_frame();
	});
}

extern "C" RV_API RvResult RV_CALL rv_get_device_info(RvContextHandle handle,
	char* vendor, uint32_t vendor_capacity, char* renderer, uint32_t renderer_capacity,
	char* version, uint32_t version_capacity)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) return RV_RESULT_STALE_HANDLE;
		if(context->queue().is_closing()) return RV_RESULT_CLOSING;
		const RvResult owner = check_owner(*context);
		if(owner != RV_RESULT_OK) return owner;
		return context->device_strings(vendor, vendor_capacity, renderer, renderer_capacity,
			version, version_capacity);
	});
}

extern "C" RV_API RvResult RV_CALL rv_get_stats(
	RvContextHandle handle, RvStats* out, uint32_t stats_bytes)
{
	return guarded([&]() -> RvResult {
		if(out == nullptr) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		if(stats_bytes != RV_STATS_BYTES) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		const revival::graphics::QueueSnapshot snapshot = context->queue().snapshot();
		const revival::graphics::FrameMailboxSnapshot frame_snapshot = context->completed_frames().snapshot();
		const revival::graphics::OverlayMailboxSnapshot overlay_snapshot = context->overlays().snapshot();
		if(snapshot.queued_bytes == UINT32_MAX || snapshot.queued_operations == UINT32_MAX ||
			snapshot.pending_frames == UINT32_MAX || snapshot.inflight_bytes == UINT32_MAX ||
			snapshot.inflight_operations == UINT32_MAX || snapshot.inflight_frames == UINT32_MAX ||
			frame_snapshot.queued_bytes == UINT32_MAX ||
			frame_snapshot.inflight_bytes == UINT32_MAX || frame_snapshot.pending_frames == UINT32_MAX ||
			frame_snapshot.inflight_frames == UINT32_MAX || overlay_snapshot.queued_bytes == UINT32_MAX ||
			overlay_snapshot.inflight_bytes == UINT32_MAX) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		RvStats result{};
		result.struct_bytes = RV_STATS_BYTES;
		result.abi_version = RV_ABI_VERSION;
		result.packets_submitted = snapshot.packets_submitted;
		result.packets_rejected = snapshot.packets_rejected;
		result.operations_executed = snapshot.operations_executed + context->operations_executed();
		result.bytes_uploaded = snapshot.bytes_uploaded + context->bytes_uploaded();
		result.frames_presented = snapshot.frames_presented + context->frames_presented();
		result.queued_bytes = snapshot.queued_bytes + snapshot.inflight_bytes + frame_snapshot.queued_bytes +
			frame_snapshot.inflight_bytes + overlay_snapshot.queued_bytes + overlay_snapshot.inflight_bytes;
		result.queued_ops = snapshot.queued_operations + snapshot.inflight_operations;
		result.pending_frames = snapshot.pending_frames + snapshot.inflight_frames + frame_snapshot.pending_frames +
			frame_snapshot.inflight_frames;
		*out = result;
		return RV_RESULT_OK;
	});
}

extern "C" RV_API RvResult RV_CALL rv_readback_for_check(
	RvContextHandle handle, RvSurfaceHandle surface, const RvReadbackRect* rect,
	uint32_t rect_bytes, uint8_t* destination, uint32_t destination_bytes,
	uint32_t destination_stride_bytes)
{
	return guarded([&]() -> RvResult {
		auto context = find_context(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		if(rect == nullptr || (destination == nullptr && destination_bytes != 0)) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		if(rect_bytes != RV_READBACK_RECT_BYTES || rect->struct_bytes != RV_READBACK_RECT_BYTES ||
			rect->abi_version != RV_ABI_VERSION) {
			return RV_RESULT_UNSUPPORTED_ABI;
		}
		if(context->queue().is_closing()) {
			return RV_RESULT_CLOSING;
		}
		const RvResult owner = check_owner(*context);
		if(owner != RV_RESULT_OK) {
			return owner;
		}
		return context->readback(surface, *rect, rect_bytes, destination,
			destination_bytes, destination_stride_bytes);
	});
}

extern "C" RV_API RvResult RV_CALL rv_begin_close(RvContextHandle handle)
{
	return guarded([&]() -> RvResult {
		return revival::graphics::global_context_registry().begin_close(handle);
	});
}

extern "C" RV_API RvResult RV_CALL rv_destroy_on_owner(RvContextHandle handle)
{
	return guarded([&]() -> RvResult {
		return revival::graphics::global_context_registry().destroy_on_owner(
			handle, std::this_thread::get_id(), current_gl_context());
	});
}
