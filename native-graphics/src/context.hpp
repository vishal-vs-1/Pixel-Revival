#ifndef REVIVAL_GRAPHICS_CONTEXT_HPP
#define REVIVAL_GRAPHICS_CONTEXT_HPP

#include <cstdint>
#include <array>
#include <atomic>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>

#include "command_queue.hpp"
#include "command_consumer.hpp"
#include "completed_frame.hpp"
#include "diagnostics.hpp"
#include "frame_presenter.hpp"
#include "gl_functions.hpp"
#include "scale2x.hpp"
#include "surface_device.hpp"
#include "surface_ops.hpp"
#include "texture_store.hpp"

namespace revival::graphics {

RvResult validate_attach_config(const RvAttachConfig& config) noexcept;

class ContextSession {
public:
	ContextSession(
		RvContextHandle handle, const RvAttachConfig& config,
		std::thread::id owner_thread, void* native_context,
		const GlFunctionTable& gl_functions);

	RvContextHandle handle() const noexcept;
	const RvAttachConfig& config() const noexcept;
	OwnedCommandQueue& queue() noexcept;
	CompletedFrameMailbox& completed_frames() noexcept;
	OverlayMailbox& overlays() noexcept;
	TextureStore& texture_store() noexcept;
	SurfaceOperations& surface_operations() noexcept;
	Scale2xRenderer& scale2x_renderer() noexcept;
	const GlFunctionTable& gl_functions() const noexcept;
	RvResult check_owner(std::thread::id thread, void* current_context) const noexcept;
	RvSurfaceHandle allocate_resource_id() noexcept;
	RvResult submit_completed_frame(const RvFrameDesc* descriptor,
		uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes) noexcept;
	RvResult submit_overlay(uint32_t slot, const RvFrameDesc* descriptor,
		uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes) noexcept;
	RvResult pump(uint64_t budget_ns, RvPumpStats* out) noexcept;
	RvResult present(const RvViewport& viewport) noexcept;
	RvResult clear_frame() noexcept;
	RvResult device_strings(char* vendor, uint32_t vendor_capacity,
		char* renderer, uint32_t renderer_capacity,
		char* version, uint32_t version_capacity) const noexcept;
	RvResult readback(RvSurfaceHandle surface, const RvReadbackRect& rect,
		uint32_t rect_bytes, uint8_t* destination, uint32_t destination_bytes,
		uint32_t destination_stride_bytes) const noexcept;
	uint64_t frames_presented() const noexcept;
	uint64_t bytes_uploaded() const noexcept;
	uint64_t operations_executed() const noexcept;
	void begin_close() noexcept;
	void clear_graphics_resources() noexcept;

private:
	const RvContextHandle handle_;
	const RvAttachConfig config_;
	const std::thread::id owner_thread_;
	void* const native_context_;
	const GlFunctionTable gl_functions_;
	OpenGlTextureDevice texture_device_;
	TextureStore texture_store_;
	OpenGlSurfaceDevice surface_renderer_;
	SurfaceOperations surface_operations_;
	Scale2xRenderer scale2x_renderer_;
	FramePresenter frame_presenter_;
	CommandConsumer command_consumer_;
	OpenGlFrameOutputRenderer frame_output_renderer_;
	GraphicsDiagnostics diagnostics_;
	OwnedCommandQueue queue_;
	CompletedFrameMailbox completed_frames_;
	OverlayMailbox overlays_;
	std::array<RvSurfaceHandle, 2> overlay_surfaces_{};
	std::mutex frame_submission_mutex_;
	uint64_t next_resource_id_ = UINT64_C(1) << 63;
	std::atomic<uint64_t> frames_presented_{0};
	std::atomic<uint64_t> bytes_uploaded_{0};
	std::atomic<uint64_t> operations_executed_{0};

	RvResult upload_surface(const RvFrameDesc& descriptor, const uint8_t* pixels,
		uint32_t pixel_bytes, RvSurfaceHandle* out_surface) noexcept;
	RvResult process_frame(CompletedFrame& frame, ScaleReport* scale_report) noexcept;
	RvResult process_overlay(OverlayUpdate& update) noexcept;
	RvResult fill_pump_queue_stats(RvPumpStats* out) const noexcept;
};

class ContextRegistry {
public:
	RvResult create(
		const RvAttachConfig& config, std::thread::id owner_thread,
		void* native_context, const GlFunctionTable& gl_functions,
		RvContextHandle* out_handle);
	std::shared_ptr<ContextSession> find(RvContextHandle handle) const;
	RvResult begin_close(RvContextHandle handle) noexcept;
	RvResult destroy_on_owner(
		RvContextHandle handle, std::thread::id thread,
		void* current_context) noexcept;

private:
	mutable std::mutex mutex_;
	std::unordered_map<RvContextHandle, std::shared_ptr<ContextSession>> contexts_;
	RvContextHandle next_handle_ = 1;
};

ContextRegistry& global_context_registry();

} // namespace revival::graphics

#endif
