#ifndef REVIVAL_GRAPHICS_PACKET_HPP
#define REVIVAL_GRAPHICS_PACKET_HPP

#include <cstdint>
#include <vector>

#include "revival_graphics.h"

namespace revival::graphics {

struct CommandSpan {
	uint16_t opcode = 0;
	uint16_t flags = 0;
	uint32_t command_bytes = 0;
	uint32_t offset = 0;
};

struct ValidatedPacket {
	uint64_t session_epoch = 0;
	uint64_t packet_sequence = 0;
	uint32_t frame_commits = 0;
	std::vector<CommandSpan> commands;
};

RvResult validate_packet(
	const uint8_t* bytes, uint32_t byte_count, uint64_t expected_epoch,
	ValidatedPacket* out) noexcept;

} // namespace revival::graphics

#endif
