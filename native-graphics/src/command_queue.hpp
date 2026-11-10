#ifndef REVIVAL_GRAPHICS_COMMAND_QUEUE_HPP
#define REVIVAL_GRAPHICS_COMMAND_QUEUE_HPP

#include <cstdint>
#include <deque>
#include <mutex>
#include <vector>

#include "packet.hpp"

namespace revival::graphics {

struct QueueLimits {
	uint32_t bytes = 0;
	uint32_t operations = 0;
	uint32_t frames = 0;
};

struct QueueSnapshot {
	uint32_t queued_bytes = 0;
	uint32_t queued_operations = 0;
	uint32_t pending_frames = 0;
	// A popped packet remains charged until its full owner-thread work unit finishes.
	uint32_t inflight_bytes = 0;
	uint32_t inflight_operations = 0;
	uint32_t inflight_frames = 0;
	uint64_t packets_submitted = 0;
	uint64_t packets_rejected = 0;
	uint64_t operations_executed = 0;
	uint64_t bytes_uploaded = 0;
	uint64_t frames_presented = 0;
};

struct QueuedItem {
	bool is_completed_frame = false;
	std::vector<uint8_t> bytes;
	RvFrameDesc frame_desc{};
	ValidatedPacket packet;
	uint32_t operation_count = 0;
	uint32_t frame_count = 0;
};

class OwnedCommandQueue {
public:
	OwnedCommandQueue(QueueLimits limits, uint64_t session_epoch);

	RvResult submit_packet(const uint8_t* bytes, uint32_t byte_count) noexcept;
	RvResult submit_completed_frame(
		const RvFrameDesc* desc, uint32_t desc_bytes,
		const uint8_t* pixels, uint32_t pixel_bytes) noexcept;
	RvResult try_pop(QueuedItem* out, bool* found) noexcept;
	RvResult release_inflight(uint32_t owned_bytes, uint32_t operations, uint32_t frames) noexcept;
	void begin_close() noexcept;
	bool is_closing() const noexcept;
	QueueSnapshot snapshot() const noexcept;

private:
	RvResult enqueue(QueuedItem&& item, uint32_t owned_bytes) noexcept;
	void reject_packet() noexcept;

	const QueueLimits limits_;
	const uint64_t session_epoch_;
	mutable std::mutex mutex_;
	std::deque<QueuedItem> queue_;
	QueueSnapshot snapshot_{};
	bool closing_ = false;
	uint64_t last_packet_sequence_ = 0;
};

} // namespace revival::graphics

#endif
