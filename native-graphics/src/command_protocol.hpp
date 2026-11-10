#ifndef REVIVAL_GRAPHICS_COMMAND_PROTOCOL_HPP
#define REVIVAL_GRAPHICS_COMMAND_PROTOCOL_HPP

#include <cstdint>
#include <vector>

#include "surface_protocol.hpp"

namespace revival::graphics {

struct GraphicsCommand {
	uint16_t opcode = 0;
	RvSurfaceHandle surface = 0;
	uint32_t logical_width = 0;
	uint32_t logical_height = 0;
	uint32_t storage_scale = 0;
	RvFrameDesc frame_desc{};
	uint32_t total_pixel_bytes = 0;
	uint32_t first_row = 0;
	uint32_t row_count = 0;
	uint32_t source_stride_bytes = 0;
	uint32_t data_bytes = 0;
	uint32_t data_offset = 0;
	SurfaceCommand surface_command{};
	uint64_t frame_sequence = 0;
	uint32_t desired_scale = 0;
	RvSurfaceHandle scale_source = 0;
	RvSurfaceHandle scale_destination = 0;
	uint32_t scale_target = 0;
	SurfaceRect content{};
};

RvResult validate_graphics_command_payload(
	uint16_t opcode, const uint8_t* payload, uint32_t payload_bytes) noexcept;
RvResult decode_graphics_commands(const uint8_t* packet_bytes, uint32_t packet_size,
	const ValidatedPacket& packet, std::vector<GraphicsCommand>* out) noexcept;

} // namespace revival::graphics

#endif
