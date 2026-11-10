#include "completed_frame.hpp"

#include <cstring>
#include <new>
#include <utility>

#include "frame_desc.hpp"

namespace revival::graphics {
namespace {

bool valid_overlay_descriptor(const RvFrameDesc& descriptor) noexcept
{
	return descriptor.struct_bytes == RV_FRAME_DESC_BYTES &&
		descriptor.abi_version == RV_ABI_VERSION &&
		(descriptor.source_scale == RV_SCALE_ORIGINAL || descriptor.source_scale == RV_SCALE_2X ||
			descriptor.source_scale == RV_SCALE_4X) &&
		(descriptor.desired_scale == RV_SCALE_ORIGINAL || descriptor.desired_scale == RV_SCALE_2X ||
			descriptor.desired_scale == RV_SCALE_4X) &&
		(descriptor.pixel_format == RV_PIXEL_FORMAT_RGBA8 || descriptor.pixel_format == RV_PIXEL_FORMAT_BGRA8) &&
		(descriptor.alpha_mode == RV_ALPHA_OPAQUE || descriptor.alpha_mode == RV_ALPHA_STRAIGHT ||
			descriptor.alpha_mode == RV_ALPHA_PREMULTIPLIED) && descriptor.origin == RV_ORIGIN_TOP_LEFT;
}

} // namespace

CompletedFrameMailbox::CompletedFrameMailbox(uint32_t max_bytes, uint32_t max_frames) noexcept
	: max_bytes_(max_bytes), max_frames_(max_frames)
{
}

RvResult CompletedFrameMailbox::submit(const RvFrameDesc* descriptor,
	uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes,
	uint32_t reserved_bytes) noexcept
{
	if(descriptor == nullptr || pixels == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	if(descriptor_bytes != RV_FRAME_DESC_BYTES) return RV_RESULT_UNSUPPORTED_ABI;
	if(descriptor->frame_sequence == 0) return RV_RESULT_INVALID_ARGUMENT;
	const uint64_t frame_sequence = descriptor->frame_sequence;
	RvResult valid = validate_frame_desc(*descriptor, pixel_bytes);
	if(valid != RV_RESULT_OK) {
		try {
			std::lock_guard<std::mutex> lock(mutex_);
			++snapshot_.frames_rejected;
		} catch(...) {
		}
		return valid;
	}
	if(max_frames_ == 0 || reserved_bytes > max_bytes_ ||
		pixel_bytes > max_bytes_ - reserved_bytes) return RV_RESULT_RESOURCE_LIMIT;

	try {
		CompletedFrame frame{};
		std::memcpy(&frame.descriptor, descriptor, sizeof(frame.descriptor));
		std::lock_guard<std::mutex> lock(mutex_);
		if(closing_) return RV_RESULT_CLOSING;
		if(frame_sequence <= last_frame_sequence_) {
			++snapshot_.frames_rejected;
			return RV_RESULT_BAD_PACKET;
		}
		const uint32_t available_bytes = max_bytes_ - reserved_bytes;
		if(snapshot_.inflight_bytes > available_bytes || pixel_bytes > available_bytes - snapshot_.inflight_bytes) {
			++snapshot_.frames_rejected;
			return RV_RESULT_QUEUE_FULL;
		}
		while(frames_.size() + snapshot_.inflight_frames >= max_frames_ ||
			static_cast<uint64_t>(snapshot_.queued_bytes) + snapshot_.inflight_bytes + pixel_bytes >
			available_bytes) {
			if(frames_.empty()) {
				++snapshot_.frames_rejected;
				return RV_RESULT_QUEUE_FULL;
			}
			snapshot_.queued_bytes -= static_cast<uint32_t>(frames_.front().pixels.size());
			frames_.pop_front();
			++snapshot_.frames_replaced;
		}
		// Return the last retired allocation to the producer instead of allocating
		// across two threads every frame. Discard it before a different-size copy.
		if(spare_.size() == pixel_bytes) {
			frame.pixels.swap(spare_);
			std::memcpy(frame.pixels.data(), pixels, pixel_bytes);
		} else {
			std::vector<uint8_t>().swap(spare_);
			frame.pixels.assign(pixels, pixels + pixel_bytes);
		}
		frames_.push_back(std::move(frame));
		snapshot_.queued_bytes += pixel_bytes;
		snapshot_.pending_frames = static_cast<uint32_t>(frames_.size());
		last_frame_sequence_ = frame_sequence;
		++snapshot_.frames_submitted;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult CompletedFrameMailbox::try_pop(CompletedFrame* out, bool* found) noexcept
{
	if(out == nullptr || found == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*found = false;
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		if(frames_.empty()) return closing_ ? RV_RESULT_CLOSING : RV_RESULT_OK;
		CompletedFrame frame = std::move(frames_.front());
		frames_.pop_front();
		snapshot_.queued_bytes -= static_cast<uint32_t>(frame.pixels.size());
		snapshot_.pending_frames = static_cast<uint32_t>(frames_.size());
		snapshot_.inflight_bytes += static_cast<uint32_t>(frame.pixels.size());
		++snapshot_.inflight_frames;
		*out = std::move(frame);
		*found = true;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult CompletedFrameMailbox::release_inflight(CompletedFrame&& frame) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		const auto pixel_bytes = static_cast<uint32_t>(frame.pixels.size());
		if(snapshot_.inflight_frames == 0 || pixel_bytes > snapshot_.inflight_bytes) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		if(!closing_ && spare_.empty() && frame.pixels.capacity() == pixel_bytes) {
			spare_.swap(frame.pixels);
		} else {
			std::vector<uint8_t>().swap(frame.pixels);
		}
		snapshot_.inflight_bytes -= pixel_bytes;
		--snapshot_.inflight_frames;
		return RV_RESULT_OK;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

void CompletedFrameMailbox::begin_close() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		closing_ = true;
		frames_.clear();
		std::vector<uint8_t>().swap(spare_);
		snapshot_.queued_bytes = 0;
		snapshot_.pending_frames = 0;
	} catch(...) {
	}
}

FrameMailboxSnapshot CompletedFrameMailbox::snapshot() const noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		return snapshot_;
	} catch(...) {
		FrameMailboxSnapshot failed{};
		failed.queued_bytes = UINT32_MAX;
		failed.inflight_bytes = UINT32_MAX;
		failed.pending_frames = UINT32_MAX;
		failed.inflight_frames = UINT32_MAX;
		return failed;
	}
}

FrameMailboxSnapshot CompletedFrameMailbox::reclaim_for_overlay() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		// Overlays share the frame budget. Return an idle allocation before
		// the overlay computes its headroom under the submission lock.
		std::vector<uint8_t>().swap(spare_);
		return snapshot_;
	} catch(...) {
		FrameMailboxSnapshot failed{};
		failed.queued_bytes = UINT32_MAX;
		return failed;
	}
}

OverlayMailbox::OverlayMailbox(uint32_t max_bytes) noexcept : max_bytes_(max_bytes)
{
}

RvResult OverlayMailbox::submit(uint32_t slot, const RvFrameDesc* descriptor,
	uint32_t descriptor_bytes, const uint8_t* pixels, uint32_t pixel_bytes,
	uint32_t reserved_bytes) noexcept
{
	if(slot >= updates_.size() || descriptor == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	if(descriptor_bytes != RV_FRAME_DESC_BYTES) return RV_RESULT_UNSUPPORTED_ABI;
	RvFrameDesc copied{};
	std::memcpy(&copied, descriptor, sizeof(copied));
	if(!valid_overlay_descriptor(copied)) return RV_RESULT_INVALID_ARGUMENT;
	const bool clear = copied.physical_width == 0 && copied.physical_height == 0 &&
		copied.physical_stride_bytes == 0 && copied.logical_width == 0 && copied.logical_height == 0;
	if(clear) {
		if(pixels != nullptr || pixel_bytes != 0) return RV_RESULT_INVALID_ARGUMENT;
	} else {
		if(pixels == nullptr) return RV_RESULT_INVALID_ARGUMENT;
		const RvResult valid = validate_frame_desc(copied, pixel_bytes);
		if(valid != RV_RESULT_OK) return valid;
	}
	if(reserved_bytes > max_bytes_ || pixel_bytes > max_bytes_ - reserved_bytes) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	try {
		OverlayUpdate candidate{};
		candidate.pending = true;
		candidate.clear = clear;
		candidate.slot = slot;
		candidate.descriptor = copied;
		std::lock_guard<std::mutex> lock(mutex_);
		if(closing_) return RV_RESULT_CLOSING;
		const uint32_t old_bytes = updates_[slot].pending ?
			static_cast<uint32_t>(updates_[slot].pixels.size()) : 0;
		const uint64_t owned_bytes = static_cast<uint64_t>(snapshot_.queued_bytes) +
			snapshot_.inflight_bytes;
		if(old_bytes > snapshot_.queued_bytes || owned_bytes < old_bytes) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		const uint64_t other_overlay_bytes = owned_bytes - old_bytes;
		const uint32_t available_bytes = max_bytes_ - reserved_bytes;
		if(other_overlay_bytes > available_bytes ||
			pixel_bytes > available_bytes - other_overlay_bytes) {
			return RV_RESULT_RESOURCE_LIMIT;
		}
		const bool had_update = updates_[slot].pending;
		if(had_update) {
			snapshot_.queued_bytes -= old_bytes;
			--snapshot_.pending_updates;
			updates_[slot] = OverlayUpdate{};
		}
		if(!clear) candidate.pixels.assign(pixels, pixels + pixel_bytes);
		updates_[slot] = std::move(candidate);
		snapshot_.queued_bytes += pixel_bytes;
		++snapshot_.pending_updates;
		++snapshot_.updates_submitted;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OverlayMailbox::drain(std::array<OverlayUpdate, 2>* out) noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		*out = std::move(updates_);
		updates_ = {};
		snapshot_.inflight_bytes += snapshot_.queued_bytes;
		snapshot_.inflight_updates += snapshot_.pending_updates;
		snapshot_.queued_bytes = 0;
		snapshot_.pending_updates = 0;
		return RV_RESULT_OK;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OverlayMailbox::try_pop(OverlayUpdate* out, bool* found) noexcept
{
	if(out == nullptr || found == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = OverlayUpdate{};
	*found = false;
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		for(OverlayUpdate& pending : updates_) {
			if(!pending.pending) continue;
			const uint32_t bytes = static_cast<uint32_t>(pending.pixels.size());
			if(bytes > snapshot_.queued_bytes) return RV_RESULT_INTERNAL_ERROR;
			*out = std::move(pending);
			pending = OverlayUpdate{};
			snapshot_.queued_bytes -= bytes;
			--snapshot_.pending_updates;
			snapshot_.inflight_bytes += bytes;
			++snapshot_.inflight_updates;
			*found = true;
			return RV_RESULT_OK;
		}
		return closing_ ? RV_RESULT_CLOSING : RV_RESULT_OK;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OverlayMailbox::release_inflight(uint32_t pixel_bytes, uint32_t updates) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		if(pixel_bytes > snapshot_.inflight_bytes || updates > snapshot_.inflight_updates) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		snapshot_.inflight_bytes -= pixel_bytes;
		snapshot_.inflight_updates -= updates;
		return RV_RESULT_OK;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

void OverlayMailbox::begin_close() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		closing_ = true;
		updates_ = {};
		snapshot_.queued_bytes = 0;
		snapshot_.pending_updates = 0;
	} catch(...) {
	}
}

OverlayMailboxSnapshot OverlayMailbox::snapshot() const noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		return snapshot_;
	} catch(...) {
		OverlayMailboxSnapshot failed{};
		failed.queued_bytes = UINT32_MAX;
		failed.inflight_bytes = UINT32_MAX;
		failed.pending_updates = UINT32_MAX;
		failed.inflight_updates = UINT32_MAX;
		return failed;
	}
}

} // namespace revival::graphics
