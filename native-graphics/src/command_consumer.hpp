#ifndef REVIVAL_GRAPHICS_COMMAND_CONSUMER_HPP
#define REVIVAL_GRAPHICS_COMMAND_CONSUMER_HPP

#include <functional>
#include <vector>

#include "command_protocol.hpp"
#include "frame_presenter.hpp"
#include "surface_ops.hpp"
#include "scale2x.hpp"

namespace revival::graphics {

struct CommandExecutionStats {
	uint32_t operations_completed = 0;
	uint32_t frames_completed = 0;
	uint64_t uploaded_bytes = 0;
};

class CommandConsumer {
public:
	struct VirtualSurface;
	using InternalIdAllocator = std::function<RvSurfaceHandle()>;

	CommandConsumer(TextureStore& textures, SurfaceOperations& surfaces,
		FramePresenter& presenter, Scale2xRenderer& scaler,
		InternalIdAllocator allocate_internal_id);
	RvResult process(const std::vector<uint8_t>& packet_bytes,
		const ValidatedPacket& packet, CommandExecutionStats* out) noexcept;

private:
	RvResult preflight(const std::vector<GraphicsCommand>& commands,
		uint64_t* out_last_surface, uint64_t* out_last_frame) const noexcept;
	RvResult execute(const GraphicsCommand& command,
		const std::vector<uint8_t>& packet_bytes, CommandExecutionStats* stats) noexcept;

	TextureStore& textures_;
	SurfaceOperations& surfaces_;
	FramePresenter& presenter_;
	Scale2xRenderer& scaler_;
	InternalIdAllocator allocate_internal_id_;
	uint64_t last_surface_handle_ = 0;
	uint64_t last_frame_sequence_ = 0;
};

} // namespace revival::graphics

#endif
