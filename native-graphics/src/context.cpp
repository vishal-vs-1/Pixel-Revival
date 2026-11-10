#include "context.hpp"

#include <algorithm>
#include <chrono>
#include <cstring>
#include <new>
#include <utility>

namespace revival::graphics {
namespace {

bool valid_detail_scale(uint32_t scale) noexcept
{
	return scale == RV_SCALE_2X || scale == RV_SCALE_4X;
}

} // namespace

RvResult validate_attach_config(const RvAttachConfig& config) noexcept
{
	if(config.struct_bytes != RV_ATTACH_CONFIG_BYTES || config.abi_version != RV_ABI_VERSION) {
		return RV_RESULT_UNSUPPORTED_ABI;
	}
	if(config.session_epoch == 0 || config.texture_budget_bytes == 0 ||
		config.texture_budget_bytes > RV_MAX_TEXTURE_BUDGET_BYTES ||
		config.max_queued_bytes == 0 || config.max_queued_bytes > RV_MAX_QUEUED_BYTES ||
		config.max_queued_ops == 0 || config.max_queued_ops > RV_MAX_QUEUED_OPS ||
		config.max_pending_frames == 0 || config.max_pending_frames > RV_MAX_PENDING_FRAMES ||
		!valid_detail_scale(config.detail_scale) ||
		(config.input_mode != RV_INPUT_MODE_COMMANDS &&
			config.input_mode != RV_INPUT_MODE_COMPLETED_FRAMES) ||
		config.reserved_zero != 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	return RV_RESULT_OK;
}

ContextSession::ContextSession(
	RvContextHandle handle, const RvAttachConfig& config,
	std::thread::id owner_thread, void* native_context,
	const GlFunctionTable& gl_functions)
	: handle_(handle),
	  config_(config),
	  owner_thread_(owner_thread),
	  native_context_(native_context),
	  gl_functions_(gl_functions),
	  texture_device_(gl_functions_),
	  texture_store_(texture_device_, config.texture_budget_bytes),
	  surface_renderer_(texture_device_, gl_functions_),
	  surface_operations_(texture_store_, surface_renderer_),
	  scale2x_renderer_(texture_store_, surface_operations_, gl_functions_),
	  frame_presenter_(texture_store_, scale2x_renderer_),
	  command_consumer_(texture_store_, surface_operations_, frame_presenter_, scale2x_renderer_,
		[this]() noexcept { return allocate_resource_id(); }),
	  frame_output_renderer_(gl_functions_),
	  diagnostics_(gl_functions_),
	  queue_(QueueLimits{
		  config.max_queued_bytes, config.max_queued_ops, config.max_pending_frames},
		  config.session_epoch),
	  completed_frames_(config.max_queued_bytes, config.max_pending_frames),
	  overlays_(config.max_queued_bytes)
{
}

RvContextHandle ContextSession::handle() const noexcept
{
	return handle_;
}

const RvAttachConfig& ContextSession::config() const noexcept
{
	return config_;
}

OwnedCommandQueue& ContextSession::queue() noexcept
{
	return queue_;
}

CompletedFrameMailbox& ContextSession::completed_frames() noexcept
{
	return completed_frames_;
}

OverlayMailbox& ContextSession::overlays() noexcept
{
	return overlays_;
}

TextureStore& ContextSession::texture_store() noexcept
{
	return texture_store_;
}

SurfaceOperations& ContextSession::surface_operations() noexcept
{
	return surface_operations_;
}

Scale2xRenderer& ContextSession::scale2x_renderer() noexcept
{
	return scale2x_renderer_;
}

const GlFunctionTable& ContextSession::gl_functions() const noexcept
{
	return gl_functions_;
}

RvResult ContextSession::check_owner(std::thread::id thread, void* current_context) const noexcept
{
	if(thread != owner_thread_) {
		return RV_RESULT_WRONG_THREAD;
	}
	if(native_context_ == nullptr || current_context == nullptr ||
		current_context != native_context_) {
		return RV_RESULT_NO_CONTEXT;
	}
	return RV_RESULT_OK;
}

RvSurfaceHandle ContextSession::allocate_resource_id() noexcept
{
	if(next_resource_id_ == 0 || next_resource_id_ == UINT64_MAX) {
		return 0;
	}
	return next_resource_id_++;
}

RvResult ContextSession::submit_completed_frame(const RvFrameDesc* descriptor,
	uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(frame_submission_mutex_);
		const OverlayMailboxSnapshot overlay_snapshot = overlays_.snapshot();
		if(overlay_snapshot.queued_bytes == UINT32_MAX || overlay_snapshot.inflight_bytes == UINT32_MAX ||
			static_cast<uint64_t>(overlay_snapshot.queued_bytes) + overlay_snapshot.inflight_bytes > UINT32_MAX) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		return completed_frames_.submit(descriptor, descriptor_bytes, pixels, pixel_bytes,
			overlay_snapshot.queued_bytes + overlay_snapshot.inflight_bytes);
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult ContextSession::submit_overlay(uint32_t slot, const RvFrameDesc* descriptor,
	uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(frame_submission_mutex_);
		const FrameMailboxSnapshot frame_snapshot = completed_frames_.reclaim_for_overlay();
		if(frame_snapshot.queued_bytes == UINT32_MAX || frame_snapshot.inflight_bytes == UINT32_MAX ||
			static_cast<uint64_t>(frame_snapshot.queued_bytes) + frame_snapshot.inflight_bytes > UINT32_MAX) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		return overlays_.submit(slot, descriptor, descriptor_bytes, pixels, pixel_bytes,
			frame_snapshot.queued_bytes + frame_snapshot.inflight_bytes);
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult ContextSession::upload_surface(const RvFrameDesc& descriptor, const uint8_t* pixels,
	uint32_t pixel_bytes, RvSurfaceHandle* out_surface) noexcept
{
	if(out_surface == nullptr || pixels == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out_surface = 0;
	const RvSurfaceHandle surface = allocate_resource_id();
	if(surface == 0) return RV_RESULT_RESOURCE_LIMIT;
	RvResult status = texture_store_.create_upload_surface(surface, descriptor, pixel_bytes);
	if(status != RV_RESULT_OK) return status;
	status = texture_store_.upload_rows(surface, 0, descriptor.physical_height,
		descriptor.physical_stride_bytes, pixels, pixel_bytes);
	if(status == RV_RESULT_OK) status = texture_store_.end_upload(surface);
	if(status != RV_RESULT_OK) {
		texture_store_.destroy_surface(surface);
		return status;
	}
	bytes_uploaded_.fetch_add(pixel_bytes, std::memory_order_relaxed);
	*out_surface = surface;
	return RV_RESULT_OK;
}

RvResult ContextSession::process_frame(CompletedFrame& frame, ScaleReport* scale_report) noexcept
{
	if(scale_report == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	RvSurfaceHandle source = 0;
	RvResult status = upload_surface(frame.descriptor, frame.pixels.data(),
		static_cast<uint32_t>(frame.pixels.size()), &source);
	if(status == RV_RESULT_OK) {
		const RvSurfaceHandle candidate = allocate_resource_id();
		if(candidate == 0) status = RV_RESULT_RESOURCE_LIMIT;
		else {
			const SurfaceRect content{0, 0,
				static_cast<int32_t>(frame.descriptor.logical_width),
				static_cast<int32_t>(frame.descriptor.logical_height)};
			const uint32_t requested_scale = std::max(config_.detail_scale,
				frame.descriptor.desired_scale);
			status = frame_presenter_.commit(source, content,
				frame.descriptor.frame_sequence, requested_scale, candidate, scale_report);
		}
	}
	if(source != 0) {
		const RvResult destroyed = texture_store_.destroy_surface(source);
		if(status == RV_RESULT_OK && destroyed != RV_RESULT_OK) status = destroyed;
	}
	return status;
}

RvResult ContextSession::process_overlay(OverlayUpdate& update) noexcept
{
	if(!update.pending || update.slot >= overlay_surfaces_.size()) return RV_RESULT_INVALID_ARGUMENT;
	RvSurfaceHandle& current = overlay_surfaces_[update.slot];
	if(update.clear) {
		if(current == 0) return RV_RESULT_OK;
		const RvResult destroyed = texture_store_.destroy_surface(current);
		if(destroyed != RV_RESULT_OK) return destroyed;
		current = 0;
		return RV_RESULT_OK;
	}
	RvSurfaceHandle replacement = 0;
	RvResult status = upload_surface(update.descriptor, update.pixels.data(),
		static_cast<uint32_t>(update.pixels.size()), &replacement);
	if(status != RV_RESULT_OK) return status;
	if(current != 0) {
		status = texture_store_.destroy_surface(current);
		if(status != RV_RESULT_OK) {
			texture_store_.destroy_surface(replacement);
			return status;
		}
	}
	current = replacement;
	return RV_RESULT_OK;
}

RvResult ContextSession::fill_pump_queue_stats(RvPumpStats* out) const noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	const QueueSnapshot queue = queue_.snapshot();
	const FrameMailboxSnapshot frames = completed_frames_.snapshot();
	const OverlayMailboxSnapshot overlays = overlays_.snapshot();
	if(queue.queued_bytes == UINT32_MAX || queue.queued_operations == UINT32_MAX ||
		queue.inflight_bytes == UINT32_MAX || queue.inflight_operations == UINT32_MAX ||
		frames.queued_bytes == UINT32_MAX || frames.inflight_bytes == UINT32_MAX ||
		overlays.queued_bytes == UINT32_MAX || overlays.inflight_bytes == UINT32_MAX) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	const uint64_t queued_bytes = static_cast<uint64_t>(queue.queued_bytes) + queue.inflight_bytes +
		frames.queued_bytes + frames.inflight_bytes + overlays.queued_bytes + overlays.inflight_bytes;
	if(queued_bytes > UINT32_MAX) return RV_RESULT_RESOURCE_LIMIT;
	out->queued_bytes = static_cast<uint32_t>(queued_bytes);
	out->queued_ops = queue.queued_operations + queue.inflight_operations;
	return RV_RESULT_OK;
}

RvResult ContextSession::pump(uint64_t budget_ns, RvPumpStats* out) noexcept
{
	if(out == nullptr || out->struct_bytes != RV_PUMP_STATS_BYTES ||
		out->abi_version != RV_ABI_VERSION) return RV_RESULT_UNSUPPORTED_ABI;
	RvPumpStats result{};
	result.struct_bytes = RV_PUMP_STATS_BYTES;
	result.abi_version = RV_ABI_VERSION;
	const auto started = std::chrono::steady_clock::now();
	const auto elapsed = [&]() -> uint64_t {
		const auto duration = std::chrono::steady_clock::now() - started;
		return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(duration).count());
	};
	const auto finish = [&](RvResult status) {
		result.elapsed_ns = elapsed();
		const RvResult stats = fill_pump_queue_stats(&result);
		*out = result;
		return status == RV_RESULT_OK ? stats : status;
	};
	if(budget_ns == 0) return finish(RV_RESULT_OK);
	if(config_.input_mode == RV_INPUT_MODE_COMMANDS) {
		while(elapsed() < budget_ns) {
			QueuedItem item{};
			bool found = false;
			RvResult status = queue_.try_pop(&item, &found);
			if(status == RV_RESULT_CLOSING && !found) break;
			if(status != RV_RESULT_OK) return finish(status);
			if(!found) break;
			CommandExecutionStats executed{};
			status = command_consumer_.process(item.bytes, item.packet, &executed);
			const uint32_t owned_bytes = static_cast<uint32_t>(item.bytes.size());
			const uint32_t operations = item.operation_count;
			const uint32_t frames = item.frame_count;
			std::vector<uint8_t>().swap(item.bytes);
			std::vector<CommandSpan>().swap(item.packet.commands);
			const RvResult released = queue_.release_inflight(owned_bytes, operations, frames);
			result.operations_processed += executed.operations_completed;
			result.frames_completed += executed.frames_completed;
			result.uploaded_bytes += executed.uploaded_bytes;
			operations_executed_.fetch_add(executed.operations_completed, std::memory_order_relaxed);
			bytes_uploaded_.fetch_add(executed.uploaded_bytes, std::memory_order_relaxed);
			if(released != RV_RESULT_OK) return finish(released);
			if(status != RV_RESULT_OK) return finish(status);
		}
		return finish(RV_RESULT_OK);
	}

	// The budget is soft because each overlay upload is atomic and can finish after it expires.
	while(elapsed() < budget_ns) {
		OverlayUpdate update{};
		bool found = false;
		RvResult status = overlays_.try_pop(&update, &found);
		if(status != RV_RESULT_OK) return finish(status);
		if(!found) break;
		const uint32_t bytes = static_cast<uint32_t>(update.pixels.size());
		status = process_overlay(update);
		std::vector<uint8_t>().swap(update.pixels);
		const RvResult released = overlays_.release_inflight(bytes, 1);
		++result.operations_processed;
		if(released != RV_RESULT_OK) return finish(released);
		if(status != RV_RESULT_OK) return finish(status);
		result.uploaded_bytes += bytes;
		operations_executed_.fetch_add(1, std::memory_order_relaxed);
	}

	// Frames are also atomic work units; check the deadline between frames, never mid-commit.
	while(elapsed() < budget_ns) {
		CompletedFrame frame{};
		bool found = false;
		RvResult status = completed_frames_.try_pop(&frame, &found);
		if(status == RV_RESULT_CLOSING && !found) break;
		if(status != RV_RESULT_OK) return finish(status);
		if(!found) break;
		const uint32_t bytes = static_cast<uint32_t>(frame.pixels.size());
		ScaleReport scale_report{};
		status = process_frame(frame, &scale_report);
		const RvResult released = completed_frames_.release_inflight(std::move(frame));
		++result.operations_processed;
		if(released != RV_RESULT_OK) return finish(released);
		if(status != RV_RESULT_OK) return finish(status);
		result.uploaded_bytes += bytes;
		operations_executed_.fetch_add(1, std::memory_order_relaxed);
		++result.frames_completed;
	}
	return finish(RV_RESULT_OK);
}

RvResult ContextSession::present(const RvViewport& viewport) noexcept
{
	const RvResult valid = validate_viewport(viewport);
	if(valid != RV_RESULT_OK) return valid;
	CommittedFrameSnapshot frame{};
	const RvResult current = frame_presenter_.current(&frame);
	if(current != RV_RESULT_OK && current != RV_RESULT_STALE_HANDLE) return current;
	if(frame.valid && static_cast<uint64_t>(frame.content.width) * viewport.aspect_den !=
		static_cast<uint64_t>(frame.content.height) * viewport.aspect_num) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const RvResult rendered = frame_output_renderer_.render(frame, overlay_surfaces_,
		texture_store_, viewport);
	if(rendered == RV_RESULT_OK) frames_presented_.fetch_add(1, std::memory_order_relaxed);
	return rendered;
}

RvResult ContextSession::clear_frame() noexcept
{
	return frame_presenter_.clear();
}

RvResult ContextSession::device_strings(char* vendor, uint32_t vendor_capacity,
	char* renderer, uint32_t renderer_capacity,
	char* version, uint32_t version_capacity) const noexcept
{
	const RvResult owner = texture_store_.check_owner();
	return owner == RV_RESULT_OK ? diagnostics_.device_strings(vendor, vendor_capacity,
		renderer, renderer_capacity, version, version_capacity) : owner;
}

RvResult ContextSession::readback(RvSurfaceHandle surface, const RvReadbackRect& rect,
	uint32_t rect_bytes, uint8_t* destination, uint32_t destination_bytes,
	uint32_t destination_stride_bytes) const noexcept
{
	const RvResult owner = texture_store_.check_owner();
	return owner == RV_RESULT_OK ? diagnostics_.readback(texture_store_, surface, rect,
		rect_bytes, destination, destination_bytes, destination_stride_bytes) : owner;
}

uint64_t ContextSession::frames_presented() const noexcept
{
	return frames_presented_.load(std::memory_order_relaxed);
}

uint64_t ContextSession::bytes_uploaded() const noexcept
{
	return bytes_uploaded_.load(std::memory_order_relaxed);
}

uint64_t ContextSession::operations_executed() const noexcept
{
	return operations_executed_.load(std::memory_order_relaxed);
}

void ContextSession::begin_close() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(frame_submission_mutex_);
		queue_.begin_close();
		completed_frames_.begin_close();
		overlays_.begin_close();
	} catch(...) {
	}
}

void ContextSession::clear_graphics_resources() noexcept
{
	for(RvSurfaceHandle& overlay : overlay_surfaces_) {
		if(overlay != 0) texture_store_.destroy_surface(overlay);
		overlay = 0;
	}
	frame_presenter_.destroy();
	scale2x_renderer_.destroy();
	texture_store_.clear();
}

RvResult ContextRegistry::create(
	const RvAttachConfig& config, std::thread::id owner_thread,
	void* native_context, const GlFunctionTable& gl_functions,
	RvContextHandle* out_handle)
{
	if(out_handle == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*out_handle = 0;
	const RvResult config_result = validate_attach_config(config);
	if(config_result != RV_RESULT_OK) {
		return config_result;
	}
	if(native_context == nullptr) {
		return RV_RESULT_NO_CONTEXT;
	}
	for(const void* procedure : gl_functions.procedures) {
		if(procedure == nullptr) {
			return RV_RESULT_GL_FAILURE;
		}
	}

	std::lock_guard<std::mutex> lock(mutex_);
	if(next_handle_ == 0 || next_handle_ == UINT64_MAX) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	const RvContextHandle handle = next_handle_;
	auto context = std::make_shared<ContextSession>(
		handle, config, owner_thread, native_context, gl_functions);
	const auto inserted = contexts_.emplace(handle, std::move(context));
	if(!inserted.second) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	++next_handle_;
	*out_handle = handle;
	return RV_RESULT_OK;
}

std::shared_ptr<ContextSession> ContextRegistry::find(RvContextHandle handle) const
{
	if(handle == 0) {
		return {};
	}
	std::lock_guard<std::mutex> lock(mutex_);
	const auto found = contexts_.find(handle);
	return found == contexts_.end() ? std::shared_ptr<ContextSession>{} : found->second;
}

RvResult ContextRegistry::begin_close(RvContextHandle handle) noexcept
{
	try {
		auto context = find(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		context->begin_close();
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult ContextRegistry::destroy_on_owner(
	RvContextHandle handle, std::thread::id thread, void* current_context) noexcept
{
	try {
		auto context = find(handle);
		if(!context) {
			return RV_RESULT_STALE_HANDLE;
		}
		const RvResult owner_result = context->check_owner(thread, current_context);
		if(owner_result != RV_RESULT_OK) {
			return owner_result;
		}
		context->begin_close();
		context->clear_graphics_resources();

		std::lock_guard<std::mutex> lock(mutex_);
		const auto found = contexts_.find(handle);
		if(found == contexts_.end() || found->second != context) {
			return RV_RESULT_STALE_HANDLE;
		}
		contexts_.erase(found);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

ContextRegistry& global_context_registry()
{
	static ContextRegistry registry;
	return registry;
}

} // namespace revival::graphics
