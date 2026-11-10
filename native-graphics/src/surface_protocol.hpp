#ifndef REVIVAL_GRAPHICS_SURFACE_PROTOCOL_HPP
#define REVIVAL_GRAPHICS_SURFACE_PROTOCOL_HPP

#include <cstdint>
#include <vector>

#include "packet.hpp"

namespace revival::graphics {

struct SurfaceRect {
	int32_t x = 0;
	int32_t y = 0;
	int32_t width = 0;
	int32_t height = 0;
};

enum class SurfaceBlend : uint32_t {
	Replace = 1,
	SourceOver = 2
};

struct SurfaceCommand {
	uint16_t opcode = 0;
	RvSurfaceHandle source = 0;
	RvSurfaceHandle destination = 0;
	SurfaceRect source_rect{};
	SurfaceRect destination_rect{};
	SurfaceRect clip{};
	uint32_t argb = 0;
	uint32_t transform = 0;
	SurfaceBlend blend = SurfaceBlend::SourceOver;
};

RvResult validate_surface_payload(
	uint16_t opcode, const uint8_t* payload, uint32_t payload_bytes) noexcept;
RvResult validate_surface_command(const SurfaceCommand& command) noexcept;
RvResult decode_surface_commands(
	const uint8_t* packet_bytes, uint32_t packet_size,
	const ValidatedPacket& packet, std::vector<SurfaceCommand>* out) noexcept;

} // namespace revival::graphics

#endif
