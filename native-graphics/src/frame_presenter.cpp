#include "frame_presenter.hpp"

#include <algorithm>
#include <cstdint>

namespace revival::graphics {

RvResult validate_viewport(const RvViewport& viewport) noexcept
{
	if(viewport.struct_bytes != RV_VIEWPORT_BYTES || viewport.abi_version != RV_ABI_VERSION) {
		return RV_RESULT_UNSUPPORTED_ABI;
	}
	if(viewport.framebuffer_width == 0 || viewport.framebuffer_height == 0 ||
		viewport.framebuffer_width > INT32_MAX || viewport.framebuffer_height > INT32_MAX ||
		viewport.aspect_num == 0 || viewport.aspect_den == 0 ||
		viewport.aspect_num > RV_MAX_ASPECT_COMPONENT ||
		viewport.aspect_den > RV_MAX_ASPECT_COMPONENT ||
		(viewport.filter != RV_FILTER_NEAREST && viewport.filter != RV_FILTER_LINEAR) ||
		viewport.fit != RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	return RV_RESULT_OK;
}

RvResult calculate_viewport_rect(const RvViewport& viewport,
	FrameViewportRect* out) noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = FrameViewportRect{};
	const RvResult valid = validate_viewport(viewport);
	if(valid != RV_RESULT_OK) return valid;
	uint64_t width = static_cast<uint64_t>(viewport.framebuffer_height) * viewport.aspect_num /
		viewport.aspect_den;
	uint64_t height = viewport.framebuffer_height;
	if(width == 0) width = 1;
	if(width > viewport.framebuffer_width) {
		width = viewport.framebuffer_width;
		height = static_cast<uint64_t>(width) * viewport.aspect_den / viewport.aspect_num;
		if(height == 0) height = 1;
	}
	if(width > UINT32_MAX || height > UINT32_MAX) return RV_RESULT_RESOURCE_LIMIT;
	out->width = static_cast<uint32_t>(width);
	out->height = static_cast<uint32_t>(height);
	out->x = (viewport.framebuffer_width - out->width) / 2;
	out->y = (viewport.framebuffer_height - out->height) / 2;
	return RV_RESULT_OK;
}

FramePresenter::FramePresenter(TextureStore& store, SurfaceScaler& scaler) noexcept
	: store_(store), scaler_(scaler)
{
}

RvResult FramePresenter::commit(RvSurfaceHandle source_handle, const SurfaceRect& content,
	uint64_t frame_sequence, uint32_t desired_scale, RvSurfaceHandle candidate_surface,
	ScaleReport* scale_report) noexcept
{
	if(scale_report == nullptr || source_handle == 0 || frame_sequence == 0 ||
		candidate_surface == 0 ||
		(desired_scale != RV_SCALE_ORIGINAL && desired_scale != RV_SCALE_2X &&
			desired_scale != RV_SCALE_4X)) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	TextureSurfaceSnapshot source{};
	RvResult status = store_.inspect(source_handle, &source);
	if(status != RV_RESULT_OK) return status;
	if(!source.initialized || content.x < 0 || content.y < 0 || content.width <= 0 ||
		content.height <= 0 || static_cast<int64_t>(content.x) + content.width > source.logical_width ||
		static_cast<int64_t>(content.y) + content.height > source.logical_height) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(frame_sequence <= last_frame_sequence_) return RV_RESULT_BAD_PACKET;
	const uint32_t target_scale = std::max(source.storage_scale, desired_scale);
	if(source_handle == back_.surface) return RV_RESULT_INVALID_ARGUMENT;

	if(back_.surface != 0 && (back_.logical_width != source.logical_width ||
		back_.logical_height != source.logical_height || back_.storage_scale != target_scale)) {
		status = store_.destroy_surface(back_.surface);
		if(status != RV_RESULT_OK) return status;
		back_ = CommittedFrameSnapshot{};
	}
	if(back_.surface == 0) {
		status = store_.create_surface(candidate_surface, source.logical_width,
			source.logical_height, target_scale);
		if(status != RV_RESULT_OK) return status;
		back_.surface = candidate_surface;
		back_.logical_width = source.logical_width;
		back_.logical_height = source.logical_height;
		back_.storage_scale = target_scale;
	}

	status = scaler_.apply(source_handle, back_.surface, scale_report);
	if(status != RV_RESULT_OK) return status;
	TextureSurfaceSnapshot completed{};
	status = store_.inspect(back_.surface, &completed);
	if(status != RV_RESULT_OK || !completed.initialized) {
		return status == RV_RESULT_OK ? RV_RESULT_INTERNAL_ERROR : status;
	}
	back_.valid = true;
	back_.source_generation = source.generation;
	back_.frame_sequence = frame_sequence;
	back_.content = content;
	std::swap(front_, back_);
	last_frame_sequence_ = frame_sequence;
	return RV_RESULT_OK;
}

RvResult FramePresenter::clear() noexcept
{
	front_.valid = false;
	return RV_RESULT_OK;
}

RvResult FramePresenter::current(CommittedFrameSnapshot* out) const noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = front_;
	return front_.valid ? RV_RESULT_OK : RV_RESULT_STALE_HANDLE;
}

uint64_t FramePresenter::last_frame_sequence() const noexcept
{
	return last_frame_sequence_;
}

void FramePresenter::destroy() noexcept
{
	if(front_.surface != 0) store_.destroy_surface(front_.surface);
	if(back_.surface != 0 && back_.surface != front_.surface) store_.destroy_surface(back_.surface);
	front_ = CommittedFrameSnapshot{};
	back_ = CommittedFrameSnapshot{};
}

} // namespace revival::graphics
