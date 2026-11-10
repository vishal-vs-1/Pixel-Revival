#include "RevivalRenderingDevice.hpp"

#ifdef _WIN32

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>

#include "Core/Shared/Video/VideoRenderer.h"
#include "Core/Shared/Emulator.h"
#include "Core/Shared/EmuSettings.h"
#include "NesRendererProbe.hpp"
#include "revival_graphics.h"

namespace revival::nes {
RevivalRenderingDevice::RevivalRenderingDevice(Emulator* emulator, HWND viewer, uint32_t detail_scale) noexcept
	: video_renderer_(emulator == nullptr ? nullptr : emulator->GetVideoRenderer()),
	  emulator_(emulator),
	  viewer_(viewer),
	  detail_scale_(detail_scale == RV_SCALE_4X ? RV_SCALE_4X : RV_SCALE_2X)
{
}

RevivalRenderingDevice::~RevivalRenderingDevice()
{
	// Unregister first so the renderer thread finishes callbacks before this
	// object and its owner-thread WGL state can be destroyed.
	if(video_renderer_ != nullptr) {
		video_renderer_->UnregisterRenderingDevice(this);
		video_renderer_ = nullptr;
	}
	// Mesen normally calls OnRendererThreadStopped before destruction.  Do not
	// destroy a WGL context from an arbitrary decoder/destructor thread.
	if(renderer_thread_started_) {
		OutputDebugStringA("RevivalRenderingDevice destroyed before renderer-thread stop; resources remain fail-closed.\n");
	}
}

void RevivalRenderingDevice::report_failure(const char* operation, RvResult result) noexcept
{
	last_result_ = result;
	active_ = false;
	char message[160]{};
	_snprintf_s(message, sizeof(message), _TRUNCATE,
		"RevivalRenderingDevice %s failed (%u); common presentation disabled.\n",
		operation == nullptr ? "operation" : operation, static_cast<unsigned>(result));
	OutputDebugStringA(message);
}

void RevivalRenderingDevice::OnRendererThreadStarted()
{
	renderer_thread_started_ = true;
	active_.store(false, std::memory_order_release);
	last_result_ = RV_RESULT_OK;
	if(viewer_ == nullptr || !host_.attach(viewer_) || !host_.make_current()) {
		report_failure("WGL attach", RV_RESULT_NO_CONTEXT);
		return;
	}
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = reinterpret_cast<uintptr_t>(this) == 0 ? 1u :
		static_cast<uint64_t>(reinterpret_cast<uintptr_t>(this));
	if(config.session_epoch == 0) config.session_epoch = 1;
	config.texture_budget_bytes = RV_MAX_TEXTURE_BUDGET_BYTES;
	config.max_queued_bytes = RV_MAX_QUEUED_BYTES;
	config.max_queued_ops = RV_MAX_QUEUED_OPS;
	config.max_pending_frames = RV_MAX_PENDING_FRAMES;
	config.detail_scale = detail_scale_;
	config.input_mode = RV_INPUT_MODE_COMPLETED_FRAMES;

	const RvResult attached = rv_attach_current_context(&config, RV_ATTACH_CONFIG_BYTES, &context_);
	if(attached != RV_RESULT_OK || context_ == 0) {
		report_failure("common attach", attached);
		host_.detach();
		return;
	}
	// Identity is diagnostic only; an unavailable driver string must not stop gameplay.
	capture_graphics_identity();
	active_.store(true, std::memory_order_release);
}

void RevivalRenderingDevice::capture_graphics_identity() noexcept
{
	GraphicsIdentity captured{};
	RvBuildInfo build{};
	build.struct_bytes = RV_BUILD_INFO_BYTES;
	build.abi_version = RV_ABI_VERSION;
	if(rv_get_build_info(&build, RV_BUILD_INFO_BYTES) != RV_RESULT_OK ||
		build.struct_bytes != RV_BUILD_INFO_BYTES || build.abi_version != RV_ABI_VERSION) {
		return;
	}
	for(uint32_t index = 0; index < 64; index++) {
		captured.BuildId[index] = static_cast<char>(build.build_id[index]);
	}
	captured.BuildId[64] = '\0';
	captured.AbiVersion = build.abi_version;
	if(rv_get_device_info(context_, captured.Vendor, sizeof(captured.Vendor),
		captured.Renderer, sizeof(captured.Renderer),
		captured.Version, sizeof(captured.Version)) != RV_RESULT_OK ||
		captured.Vendor[0] == '\0' || captured.Renderer[0] == '\0') {
		return;
	}
	graphics_identity_ = captured;
	graphics_identity_ready_.store(true, std::memory_order_release);
}

void RevivalRenderingDevice::UpdateFrame(RenderedFrame& frame)
{
	if(!active() || frame.FrameBuffer == nullptr) return;
	FrameSource source{};
	source.pixels = frame.FrameBuffer;
	source.physical_width = frame.Width;
	source.physical_height = frame.Height;
	source.scale = frame.Scale;
	source.sequence = next_sequence_++;
	RvFrameDesc descriptor{};
	uint32_t pixel_bytes = 0;
	const RvResult built = build_frame_desc(source, &descriptor, &pixel_bytes);
	if(built != RV_RESULT_OK) {
		report_failure("frame descriptor", built);
		return;
	}
	const RvResult submitted = rv_submit_completed_frame(context_, &descriptor,
		RV_FRAME_DESC_BYTES, static_cast<const uint8_t*>(frame.FrameBuffer), pixel_bytes);
	if(submitted != RV_RESULT_OK) {
		report_failure("frame submit", submitted);
		return;
	}
	logical_width_.store(descriptor.logical_width, std::memory_order_release);
	logical_height_.store(descriptor.logical_height, std::memory_order_release);
	last_frame_width_.store(frame.Width, std::memory_order_release);
	last_frame_height_.store(frame.Height, std::memory_order_release);
	submitted_frames_.fetch_add(1, std::memory_order_acq_rel);
}

void RevivalRenderingDevice::GetFrameStatus(uint64_t& submitted, uint64_t& presented,
	uint32_t& width, uint32_t& height) const noexcept
{
	submitted = submitted_frames_.load(std::memory_order_acquire);
	presented = presented_frames_.load(std::memory_order_acquire);
	width = last_frame_width_.load(std::memory_order_acquire);
	height = last_frame_height_.load(std::memory_order_acquire);
}

bool RevivalRenderingDevice::GetGraphicsIdentity(GraphicsIdentity& identity) const noexcept
{
	if(!graphics_identity_ready_.load(std::memory_order_acquire)) return false;
	identity = graphics_identity_;
	return true;
}

bool RevivalRenderingDevice::submit_overlay(uint32_t slot, RenderSurfaceInfo& surface) noexcept
{
	RvFrameDesc descriptor{};
	if(surface.Buffer == nullptr || surface.Width == 0 || surface.Height == 0) {
		if(build_clear_overlay_desc(&descriptor) != RV_RESULT_OK) return false;
		last_result_ = rv_submit_overlay(context_, slot, &descriptor,
			RV_FRAME_DESC_BYTES, nullptr, 0);
		if(last_result_ == RV_RESULT_OK) surface.IsDirty = false;
		return last_result_ == RV_RESULT_OK;
	}
	OverlaySource source{surface.Buffer, surface.Width, surface.Height};
	uint32_t pixel_bytes = 0;
	const RvResult built = build_overlay_desc(source, &descriptor, &pixel_bytes);
	if(built != RV_RESULT_OK) {
		last_result_ = built;
		return false;
	}
	last_result_ = rv_submit_overlay(context_, slot, &descriptor, RV_FRAME_DESC_BYTES,
		reinterpret_cast<const uint8_t*>(surface.Buffer), pixel_bytes);
	if(last_result_ == RV_RESULT_OK) surface.IsDirty = false;
	return last_result_ == RV_RESULT_OK;
}

void RevivalRenderingDevice::Render(RenderSurfaceInfo& emuHud, RenderSurfaceInfo& scriptHud)
{
	if(!active() || !host_.make_current()) return;
	host_.set_vertical_sync(emulator_ != nullptr && emulator_->GetSettings()->GetVideoConfig().VerticalSync);
	if(clear_pending_.exchange(false, std::memory_order_acq_rel) ||
		reset_pending_.exchange(false, std::memory_order_acq_rel)) {
		last_result_ = rv_clear_frame(context_);
		if(last_result_ != RV_RESULT_OK) {
			report_failure("clear", last_result_);
			return;
		}
	}
	if(emuHud.IsDirty && !submit_overlay(0, emuHud)) {
		report_failure("emulator HUD", last_result_);
		return;
	}
	if(scriptHud.IsDirty && !submit_overlay(1, scriptHud)) {
		report_failure("script HUD", last_result_);
		return;
	}
	RvPumpStats pump{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	auto& probe = renderer_probe();
	auto started = probe.tick();
	last_result_ = rv_pump(context_, 4000000u, &pump);
	if(probe.enabled()) probe.pump(started, pump.uploaded_bytes, pump.frames_completed);
	if(last_result_ != RV_RESULT_OK) {
		report_failure("pump", last_result_);
		return;
	}
	RvViewport viewport{};
	viewport.struct_bytes = RV_VIEWPORT_BYTES;
	viewport.abi_version = RV_ABI_VERSION;
	// Mesen can resize the native child after attach or when leaving fullscreen.
	// Use the drawable size at presentation time and retry empty clients next frame.
	if(!host_.client_size(viewport.framebuffer_width, viewport.framebuffer_height)) return;
	viewport.aspect_num = std::max(1u, logical_width_.load(std::memory_order_acquire));
	viewport.aspect_den = std::max(1u, logical_height_.load(std::memory_order_acquire));
	viewport.filter = RV_FILTER_NEAREST;
	viewport.fit = RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT;
	started = probe.tick();
	last_result_ = rv_present(context_, &viewport, RV_VIEWPORT_BYTES);
	if(probe.enabled()) probe.draw(started);
	if(last_result_ != RV_RESULT_OK) {
		report_failure("present", last_result_);
		return;
	}
	started = probe.tick();
	const bool swapped = host_.swap_buffers();
	if(probe.enabled()) probe.swap(started);
	if(!swapped) {
		report_failure("SwapBuffers", RV_RESULT_GL_FAILURE);
	} else {
		presented_frames_.fetch_add(1, std::memory_order_acq_rel);
	}
}

void RevivalRenderingDevice::ClearFrame()
{
	clear_pending_.store(true, std::memory_order_release);
}

void RevivalRenderingDevice::Reset()
{
	// Reset is a presentation reset, not a game reset.  Keep the context alive
	// so Mesen's decoder, input and audio lifetimes remain untouched.
	reset_pending_.store(true, std::memory_order_release);
}

void RevivalRenderingDevice::SetFullscreenMode(FullscreenSettings)
{
	// The frontend resizes the drawable. The requested display mode may differ from its client size.
}

void RevivalRenderingDevice::close_owner_context() noexcept
{
	if(context_ != 0 && host_.make_current()) {
		rv_begin_close(context_);
		rv_destroy_on_owner(context_);
		context_ = 0;
	}
	host_.detach();
	active_.store(false, std::memory_order_release);
}

void RevivalRenderingDevice::OnRendererThreadStopped()
{
	close_owner_context();
	renderer_thread_started_ = false;
}

} // namespace revival::nes

#endif
