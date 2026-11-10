#ifndef REVIVAL_GRAPHICS_COMPLETED_FRAME_HPP
#define REVIVAL_GRAPHICS_COMPLETED_FRAME_HPP

#include <array>
#include <cstdint>
#include <deque>
#include <mutex>
#include <vector>

#include "revival_graphics.h"

namespace revival::graphics {

struct CompletedFrame {
	RvFrameDesc descriptor{};
	std::vector<uint8_t> pixels;
};

struct FrameMailboxSnapshot {
	uint32_t queued_bytes = 0;
	// Payloads stay charged after pop until the owner-thread upload releases them.
	uint32_t inflight_bytes = 0;
	uint32_t pending_frames = 0;
	uint32_t inflight_frames = 0;
	uint64_t frames_submitted = 0;
	uint64_t frames_replaced = 0;
	uint64_t frames_rejected = 0;
};

class CompletedFrameMailbox {
public:
	CompletedFrameMailbox(uint32_t max_bytes, uint32_t max_frames) noexcept;
	RvResult submit(const RvFrameDesc* descriptor, uint32_t descriptor_bytes,
		const uint8_t* pixels, uint32_t pixel_bytes,
		uint32_t reserved_bytes = 0) noexcept;
	RvResult try_pop(CompletedFrame* out, bool* found) noexcept;
	RvResult release_inflight(CompletedFrame&& frame) noexcept;
	void begin_close() noexcept;
	FrameMailboxSnapshot snapshot() const noexcept;
	FrameMailboxSnapshot reclaim_for_overlay() noexcept;

private:
	const uint32_t max_bytes_;
	const uint32_t max_frames_;
	mutable std::mutex mutex_;
	std::deque<CompletedFrame> frames_;
	std::vector<uint8_t> spare_;
	FrameMailboxSnapshot snapshot_{};
	bool closing_ = false;
	uint64_t last_frame_sequence_ = 0;
};

struct OverlayUpdate {
	bool pending = false;
	bool clear = false;
	uint32_t slot = 0;
	RvFrameDesc descriptor{};
	std::vector<uint8_t> pixels;
};

struct OverlayMailboxSnapshot {
	uint32_t queued_bytes = 0;
	// Consumer-owned overlay copies remain part of the same memory cap.
	uint32_t inflight_bytes = 0;
	uint32_t pending_updates = 0;
	uint32_t inflight_updates = 0;
	uint64_t updates_submitted = 0;
	uint64_t updates_rejected = 0;
};

class OverlayMailbox {
public:
	explicit OverlayMailbox(uint32_t max_bytes) noexcept;
	RvResult submit(uint32_t slot, const RvFrameDesc* descriptor,
		uint32_t descriptor_bytes, const uint8_t* pixels,
		uint32_t pixel_bytes, uint32_t reserved_bytes = 0) noexcept;
	RvResult try_pop(OverlayUpdate* out, bool* found) noexcept;
	RvResult drain(std::array<OverlayUpdate, 2>* out) noexcept;
	RvResult release_inflight(uint32_t pixel_bytes, uint32_t updates) noexcept;
	void begin_close() noexcept;
	OverlayMailboxSnapshot snapshot() const noexcept;

private:
	const uint32_t max_bytes_;
	mutable std::mutex mutex_;
	std::array<OverlayUpdate, 2> updates_{};
	OverlayMailboxSnapshot snapshot_{};
	bool closing_ = false;
};

} // namespace revival::graphics

#endif
