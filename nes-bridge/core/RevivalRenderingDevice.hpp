#pragma once

#ifdef _WIN32

#include <atomic>
#include <cstdint>

#include "Core/Shared/Interfaces/IRenderingDevice.h"
#include "Core/Shared/RenderedFrame.h"
#include "RevivalFrameContract.hpp"
#include "RevivalWglHost.hpp"

class VideoRenderer;
class Emulator;

namespace revival::nes {

// Mesen calls UpdateFrame from its decoder thread and Render from its
// renderer thread.  The common DLL owns the copied frame mailbox; this class
// only translates Mesen's lifecycle and never retains a Mesen-owned pointer.
class RevivalRenderingDevice final : public IRenderingDevice {
public:
	struct GraphicsIdentity {
		uint32_t AbiVersion = 0;
		char BuildId[65]{};
		char Vendor[129]{};
		char Renderer[257]{};
		char Version[129]{};
	};

	RevivalRenderingDevice(Emulator* emulator, HWND viewer, uint32_t detail_scale = RV_SCALE_2X) noexcept;
	~RevivalRenderingDevice() override;

	RevivalRenderingDevice(const RevivalRenderingDevice&) = delete;
	RevivalRenderingDevice& operator=(const RevivalRenderingDevice&) = delete;

	void UpdateFrame(RenderedFrame& frame) override;
	void ClearFrame() override;
	void Render(RenderSurfaceInfo& emuHud, RenderSurfaceInfo& scriptHud) override;
	void Reset() override;
	void OnRendererThreadStarted() override;
	void OnRendererThreadStopped() override;
	void SetFullscreenMode(FullscreenSettings settings) override;

	bool active() const noexcept { return active_.load(std::memory_order_acquire); }
	RvResult last_result() const noexcept { return last_result_; }
	void GetFrameStatus(uint64_t& submitted, uint64_t& presented, uint32_t& width, uint32_t& height) const noexcept;
	bool GetGraphicsIdentity(GraphicsIdentity& identity) const noexcept;

private:
	void capture_graphics_identity() noexcept;
	bool submit_overlay(uint32_t slot, RenderSurfaceInfo& surface) noexcept;
	void close_owner_context() noexcept;
	void report_failure(const char* operation, RvResult result) noexcept;

	VideoRenderer* video_renderer_ = nullptr;
	Emulator* emulator_ = nullptr;
	HWND viewer_ = nullptr;
	uint32_t detail_scale_ = RV_SCALE_2X;
	std::atomic<uint32_t> logical_width_{256};
	std::atomic<uint32_t> logical_height_{240};
	std::atomic<uint64_t> submitted_frames_{0};
	std::atomic<uint64_t> presented_frames_{0};
	std::atomic<uint32_t> last_frame_width_{0};
	std::atomic<uint32_t> last_frame_height_{0};
	GraphicsIdentity graphics_identity_{};
	std::atomic<bool> graphics_identity_ready_{false};
	uint64_t next_sequence_ = 1;
	RvContextHandle context_ = 0;
	RvResult last_result_ = RV_RESULT_OK;
	bool renderer_thread_started_ = false;
	std::atomic<bool> active_{false};
	std::atomic<bool> clear_pending_{false};
	std::atomic<bool> reset_pending_{false};
	RevivalWglHost host_;
};

} // namespace revival::nes

#endif
