#include "surface_protocol.hpp"

#include <cstring>
#include <new>
#include <utility>

namespace revival::graphics {
namespace {

constexpr uint32_t kFillPayloadBytes = 44;
constexpr uint32_t kBlitPayloadBytes = 72;
constexpr uint32_t kDestroyPayloadBytes = 8;

uint32_t read_u32(const uint8_t* bytes) noexcept
{
	return static_cast<uint32_t>(bytes[0]) |
		(static_cast<uint32_t>(bytes[1]) << 8) |
		(static_cast<uint32_t>(bytes[2]) << 16) |
		(static_cast<uint32_t>(bytes[3]) << 24);
}

uint64_t read_u64(const uint8_t* bytes) noexcept
{
	return static_cast<uint64_t>(read_u32(bytes)) |
		(static_cast<uint64_t>(read_u32(bytes + 4)) << 32);
}

int32_t read_i32(const uint8_t* bytes) noexcept
{
	const uint32_t bits = read_u32(bytes);
	int32_t value = 0;
	static_assert(sizeof(value) == sizeof(bits), "i32 wire type must be 32 bits");
	std::memcpy(&value, &bits, sizeof(value));
	return value;
}

SurfaceRect read_rect(const uint8_t* bytes) noexcept
{
	return SurfaceRect{read_i32(bytes), read_i32(bytes + 4),
		read_i32(bytes + 8), read_i32(bytes + 12)};
}

bool positive_area(const SurfaceRect& rect) noexcept
{
	return rect.width > 0 && rect.height > 0;
}

bool valid_clip(const SurfaceRect& rect) noexcept
{
	return rect.width >= 0 && rect.height >= 0;
}

RvResult validate_blit_fields(const uint8_t* payload) noexcept
{
	const RvSurfaceHandle source = read_u64(payload);
	const RvSurfaceHandle destination = read_u64(payload + 8);
	const SurfaceRect source_rect = read_rect(payload + 16);
	const SurfaceRect destination_rect = read_rect(payload + 32);
	const SurfaceRect clip = read_rect(payload + 48);
	const uint32_t transform = read_u32(payload + 64);
	const uint32_t blend = read_u32(payload + 68);
	if(source == 0 || destination == 0 || !positive_area(source_rect) ||
		!positive_area(destination_rect) || !valid_clip(clip) || transform > 7 ||
		(blend != static_cast<uint32_t>(SurfaceBlend::Replace) &&
			blend != static_cast<uint32_t>(SurfaceBlend::SourceOver))) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	return RV_RESULT_OK;
}

} // namespace

RvResult validate_surface_command(const SurfaceCommand& command) noexcept
{
	switch(command.opcode) {
	case RV_OP_FILL:
		return command.destination != 0 && positive_area(command.destination_rect) &&
			valid_clip(command.clip) ? RV_RESULT_OK : RV_RESULT_INVALID_ARGUMENT;
	case RV_OP_BLIT:
	case RV_OP_COPY:
		if(command.source == 0 || command.destination == 0 ||
			!positive_area(command.source_rect) || !positive_area(command.destination_rect) ||
			!valid_clip(command.clip) || command.transform > 7 ||
			(command.blend != SurfaceBlend::Replace && command.blend != SurfaceBlend::SourceOver)) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return RV_RESULT_OK;
	case RV_OP_DESTROY_SURFACE:
		return command.destination != 0 ? RV_RESULT_OK : RV_RESULT_INVALID_ARGUMENT;
	default:
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
}

RvResult validate_surface_payload(
	uint16_t opcode, const uint8_t* payload, uint32_t payload_bytes) noexcept
{
	if(payload == nullptr && payload_bytes != 0) return RV_RESULT_INVALID_ARGUMENT;
	switch(opcode) {
	case RV_OP_FILL:
		if(payload_bytes != kFillPayloadBytes) return RV_RESULT_BAD_PACKET;
		if(read_u64(payload) == 0 || !positive_area(read_rect(payload + 8)) ||
			!valid_clip(read_rect(payload + 28))) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return RV_RESULT_OK;
	case RV_OP_BLIT:
	case RV_OP_COPY:
		if(payload_bytes != kBlitPayloadBytes) return RV_RESULT_BAD_PACKET;
		return validate_blit_fields(payload);
	case RV_OP_DESTROY_SURFACE:
		if(payload_bytes != kDestroyPayloadBytes) return RV_RESULT_BAD_PACKET;
		return read_u64(payload) == 0 ? RV_RESULT_INVALID_ARGUMENT : RV_RESULT_OK;
	default:
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
}

RvResult decode_surface_commands(
	const uint8_t* packet_bytes, uint32_t packet_size,
	const ValidatedPacket& packet, std::vector<SurfaceCommand>* out) noexcept
{
	if(packet_bytes == nullptr || out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = std::vector<SurfaceCommand>{};
	try {
		std::vector<SurfaceCommand> decoded;
		decoded.reserve(packet.commands.size());
		for(const CommandSpan& span : packet.commands) {
			if(span.offset > packet_size || span.command_bytes > packet_size - span.offset ||
				span.command_bytes < RV_COMMAND_HEADER_BYTES) {
				return RV_RESULT_BAD_PACKET;
			}
			const uint8_t* command = packet_bytes + span.offset;
			const uint8_t* payload = command + RV_COMMAND_HEADER_BYTES;
			const uint32_t payload_bytes = span.command_bytes - RV_COMMAND_HEADER_BYTES;
			const RvResult valid = validate_surface_payload(span.opcode, payload, payload_bytes);
			if(valid != RV_RESULT_OK) return valid;
			SurfaceCommand op{};
			op.opcode = span.opcode;
			switch(span.opcode) {
			case RV_OP_FILL:
				op.destination = read_u64(payload);
				op.destination_rect = read_rect(payload + 8);
				op.argb = read_u32(payload + 24);
				op.clip = read_rect(payload + 28);
				break;
			case RV_OP_BLIT:
			case RV_OP_COPY:
				op.source = read_u64(payload);
				op.destination = read_u64(payload + 8);
				op.source_rect = read_rect(payload + 16);
				op.destination_rect = read_rect(payload + 32);
				op.clip = read_rect(payload + 48);
				op.transform = read_u32(payload + 64);
				op.blend = static_cast<SurfaceBlend>(read_u32(payload + 68));
				break;
			case RV_OP_DESTROY_SURFACE:
				op.destination = read_u64(payload);
				break;
			default:
				return RV_RESULT_UNSUPPORTED_OPERATION;
			}
			const RvResult typed_valid = validate_surface_command(op);
			if(typed_valid != RV_RESULT_OK) return typed_valid;
			decoded.push_back(op);
		}
		*out = std::move(decoded);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

} // namespace revival::graphics
