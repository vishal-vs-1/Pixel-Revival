#include "command_queue.hpp"

#include <cstring>
#include <new>
#include <utility>

#include "frame_desc.hpp"

namespace revival::graphics {

OwnedCommandQueue::OwnedCommandQueue(QueueLimits limits, uint64_t session_epoch)
	: limits_(limits), session_epoch_(session_epoch)
{
}

RvResult OwnedCommandQueue::submit_packet(const uint8_t* bytes, uint32_t byte_count) noexcept
{
	if(bytes == nullptr) {
		reject_packet();
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(byte_count > RV_MAX_PACKET_BYTES) {
		reject_packet();
		return RV_RESULT_RESOURCE_LIMIT;
	}

	try {
		QueuedItem item;
		item.bytes.assign(bytes, bytes + byte_count);
		const RvResult valid = validate_packet(
			item.bytes.data(), byte_count, session_epoch_, &item.packet);
		if(valid != RV_RESULT_OK) {
			reject_packet();
			return valid;
		}
		item.operation_count = static_cast<uint32_t>(item.packet.commands.size());
		item.frame_count = item.packet.frame_commits;
		return enqueue(std::move(item), byte_count);
	} catch(const std::bad_alloc&) {
		reject_packet();
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		reject_packet();
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OwnedCommandQueue::submit_completed_frame(
	const RvFrameDesc* desc, uint32_t desc_bytes,
	const uint8_t* pixels, uint32_t pixel_bytes) noexcept
{
	if(desc == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(desc_bytes != RV_FRAME_DESC_BYTES) {
		return RV_RESULT_UNSUPPORTED_ABI;
	}
	if(pixels == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(pixel_bytes > limits_.bytes || pixel_bytes > RV_MAX_QUEUED_BYTES) {
		return RV_RESULT_RESOURCE_LIMIT;
	}

	try {
		QueuedItem item;
		item.is_completed_frame = true;
		std::memcpy(&item.frame_desc, desc, sizeof(item.frame_desc));
		const RvResult valid = validate_frame_desc(item.frame_desc, pixel_bytes);
		if(valid != RV_RESULT_OK) {
			return valid;
		}
		item.bytes.assign(pixels, pixels + pixel_bytes);
		item.frame_count = 1;
		return enqueue(std::move(item), pixel_bytes);
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OwnedCommandQueue::enqueue(QueuedItem&& item, uint32_t owned_bytes) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		if(closing_) {
			if(!item.is_completed_frame) {
				++snapshot_.packets_rejected;
			}
			return RV_RESULT_CLOSING;
		}
		if(snapshot_.queued_bytes > limits_.bytes ||
			snapshot_.queued_operations > limits_.operations ||
			snapshot_.pending_frames > limits_.frames ||
			snapshot_.inflight_bytes > limits_.bytes - snapshot_.queued_bytes ||
			snapshot_.inflight_operations > limits_.operations - snapshot_.queued_operations ||
			snapshot_.inflight_frames > limits_.frames - snapshot_.pending_frames) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		if(!item.is_completed_frame && item.packet.packet_sequence <= last_packet_sequence_) {
			++snapshot_.packets_rejected;
			return RV_RESULT_BAD_PACKET;
		}
		const uint32_t owned_total = snapshot_.queued_bytes + snapshot_.inflight_bytes;
		const uint32_t operation_total = snapshot_.queued_operations + snapshot_.inflight_operations;
		const uint32_t frame_total = snapshot_.pending_frames + snapshot_.inflight_frames;
		if(owned_bytes > limits_.bytes - owned_total ||
			item.operation_count > limits_.operations - operation_total ||
			item.frame_count > limits_.frames - frame_total) {
			if(!item.is_completed_frame) {
				++snapshot_.packets_rejected;
			}
			return RV_RESULT_QUEUE_FULL;
		}

		queue_.push_back(std::move(item));
		snapshot_.queued_bytes += owned_bytes;
		snapshot_.queued_operations += queue_.back().operation_count;
		snapshot_.pending_frames += queue_.back().frame_count;
		if(!queue_.back().is_completed_frame) {
			++snapshot_.packets_submitted;
			last_packet_sequence_ = queue_.back().packet.packet_sequence;
		}
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		if(!item.is_completed_frame) {
			reject_packet();
		}
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		if(!item.is_completed_frame) {
			reject_packet();
		}
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OwnedCommandQueue::try_pop(QueuedItem* out, bool* found) noexcept
{
	if(out == nullptr || found == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*found = false;
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		if(queue_.empty()) {
			return closing_ ? RV_RESULT_CLOSING : RV_RESULT_OK;
		}
		QueuedItem item = std::move(queue_.front());
		queue_.pop_front();
		snapshot_.queued_bytes -= static_cast<uint32_t>(item.bytes.size());
		snapshot_.queued_operations -= item.operation_count;
		snapshot_.pending_frames -= item.frame_count;
		snapshot_.inflight_bytes += static_cast<uint32_t>(item.bytes.size());
		snapshot_.inflight_operations += item.operation_count;
		snapshot_.inflight_frames += item.frame_count;
		*out = std::move(item);
		*found = true;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult OwnedCommandQueue::release_inflight(
	uint32_t owned_bytes, uint32_t operations, uint32_t frames) noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		if(owned_bytes > snapshot_.inflight_bytes || operations > snapshot_.inflight_operations ||
			frames > snapshot_.inflight_frames) {
			return RV_RESULT_INTERNAL_ERROR;
		}
		snapshot_.inflight_bytes -= owned_bytes;
		snapshot_.inflight_operations -= operations;
		snapshot_.inflight_frames -= frames;
		return RV_RESULT_OK;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

void OwnedCommandQueue::begin_close() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		closing_ = true;
		queue_.clear();
		snapshot_.queued_bytes = 0;
		snapshot_.queued_operations = 0;
		snapshot_.pending_frames = 0;
	} catch(...) {
		// A mutex failure cannot be recovered at the C boundary; never throw across it.
	}
}

bool OwnedCommandQueue::is_closing() const noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		return closing_;
	} catch(...) {
		return true;
	}
}

QueueSnapshot OwnedCommandQueue::snapshot() const noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		return snapshot_;
	} catch(...) {
		QueueSnapshot failed{};
		failed.queued_bytes = UINT32_MAX;
		failed.queued_operations = UINT32_MAX;
		failed.pending_frames = UINT32_MAX;
		failed.inflight_bytes = UINT32_MAX;
		failed.inflight_operations = UINT32_MAX;
		failed.inflight_frames = UINT32_MAX;
		return failed;
	}
}

void OwnedCommandQueue::reject_packet() noexcept
{
	try {
		std::lock_guard<std::mutex> lock(mutex_);
		++snapshot_.packets_rejected;
	} catch(...) {
	}
}

} // namespace revival::graphics
