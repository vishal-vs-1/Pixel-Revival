#include "command_protocol.hpp"

#include <cstring>
#include <new>
#include <utility>

#include "frame_desc.hpp"

namespace revival::graphics {
namespace {

constexpr uint32_t kCreateSurfaceBytes = 20;
constexpr uint32_t kBeginUploadBytes = 68;
constexpr uint32_t kUploadRowsHeaderBytes = 24;
constexpr uint32_t kEndUploadBytes = 8;
constexpr uint32_t kCommitFrameBytes = 36;
constexpr uint32_t kScaleSurfaceBytes = 24;

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
	std::memcpy(&value, &bits, sizeof(value));
	return value;
}

SurfaceRect read_rect(const uint8_t* bytes) noexcept
{
	return SurfaceRect{read_i32(bytes), read_i32(bytes + 4),
		read_i32(bytes + 8), read_i32(bytes + 12)};
}

RvFrameDesc read_frame_desc(const uint8_t* bytes) noexcept
{
	RvFrameDesc desc{};
	desc.struct_bytes = read_u32(bytes);
	desc.abi_version = read_u32(bytes + 4);
	desc.frame_sequence = read_u64(bytes + 8);
	desc.physical_width = read_u32(bytes + 16);
	desc.physical_height = read_u32(bytes + 20);
	desc.physical_stride_bytes = read_u32(bytes + 24);
	desc.logical_width = read_u32(bytes + 28);
	desc.logical_height = read_u32(bytes + 32);
	desc.source_scale = read_u32(bytes + 36);
	desc.desired_scale = read_u32(bytes + 40);
	desc.pixel_format = read_u32(bytes + 44);
	desc.alpha_mode = read_u32(bytes + 48);
	desc.origin = read_u32(bytes + 52);
	return desc;
}

bool positive_rect(const SurfaceRect& rect) noexcept
{
	return rect.width > 0 && rect.height > 0;
}

RvResult validate_commit(const uint8_t* payload) noexcept
{
	const uint32_t desired_scale = read_u32(payload + 16);
	const SurfaceRect content = read_rect(payload + 20);
	if(read_u64(payload) == 0 || read_u64(payload + 8) == 0 ||
		(desired_scale != RV_SCALE_ORIGINAL && desired_scale != RV_SCALE_2X &&
			desired_scale != RV_SCALE_4X) || !positive_rect(content)) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	return RV_RESULT_OK;
}

} // namespace

RvResult validate_graphics_command_payload(
	uint16_t opcode, const uint8_t* payload, uint32_t payload_bytes) noexcept
{
	if(payload == nullptr && payload_bytes != 0) return RV_RESULT_INVALID_ARGUMENT;
	switch(opcode) {
	case RV_OP_CREATE_SURFACE: {
		if(payload_bytes != kCreateSurfaceBytes) return RV_RESULT_BAD_PACKET;
		const uint64_t surface = read_u64(payload);
		const uint32_t width = read_u32(payload + 8);
		const uint32_t height = read_u32(payload + 12);
		const uint32_t scale = read_u32(payload + 16);
		if(surface == 0 || width == 0 || height == 0 ||
			static_cast<uint64_t>(width) * height > RV_MAX_LOGICAL_SOURCE_PIXELS ||
			(scale != RV_SCALE_ORIGINAL && scale != RV_SCALE_2X && scale != RV_SCALE_4X)) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return RV_RESULT_OK;
	}
	case RV_OP_BEGIN_UPLOAD: {
		if(payload_bytes != kBeginUploadBytes) return RV_RESULT_BAD_PACKET;
		if(read_u64(payload) == 0) return RV_RESULT_INVALID_ARGUMENT;
		const RvFrameDesc desc = read_frame_desc(payload + 8);
		if(desc.frame_sequence != 0) return RV_RESULT_INVALID_ARGUMENT;
		return validate_frame_desc(desc, read_u32(payload + 64));
	}
	case RV_OP_UPLOAD_ROWS: {
		if(payload_bytes < kUploadRowsHeaderBytes) return RV_RESULT_BAD_PACKET;
		const uint64_t expected_bytes = static_cast<uint64_t>(read_u32(payload + 12)) *
			read_u32(payload + 16);
		if(read_u64(payload) == 0 || read_u32(payload + 12) == 0 ||
			read_u32(payload + 16) == 0 || expected_bytes > UINT32_MAX ||
			read_u32(payload + 20) != expected_bytes ||
			payload_bytes - kUploadRowsHeaderBytes != expected_bytes) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		return RV_RESULT_OK;
	}
	case RV_OP_END_UPLOAD:
		if(payload_bytes != kEndUploadBytes) return RV_RESULT_BAD_PACKET;
		return read_u64(payload) == 0 ? RV_RESULT_INVALID_ARGUMENT : RV_RESULT_OK;
	case RV_OP_FILL:
	case RV_OP_BLIT:
	case RV_OP_COPY:
	case RV_OP_DESTROY_SURFACE:
		return validate_surface_payload(opcode, payload, payload_bytes);
	case RV_OP_COMMIT_FRAME:
		if(payload_bytes != kCommitFrameBytes) return RV_RESULT_BAD_PACKET;
		return validate_commit(payload);
	case RV_OP_CLEAR_FRAME:
		return payload_bytes == 0 ? RV_RESULT_OK : RV_RESULT_BAD_PACKET;
	case RV_OP_SCALE_SURFACE:
		if(payload_bytes != kScaleSurfaceBytes) return RV_RESULT_BAD_PACKET;
		if(read_u64(payload) == 0 || read_u64(payload + 8) == 0 ||
			(read_u32(payload + 16) != RV_SCALE_2X && read_u32(payload + 16) != RV_SCALE_4X) ||
			read_u32(payload + 20) != 0) return RV_RESULT_INVALID_ARGUMENT;
		return RV_RESULT_OK;
	default:
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
}

RvResult decode_graphics_commands(const uint8_t* packet_bytes, uint32_t packet_size,
	const ValidatedPacket& packet, std::vector<GraphicsCommand>* out) noexcept
{
	if(packet_bytes == nullptr || out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = std::vector<GraphicsCommand>{};
	try {
		std::vector<GraphicsCommand> commands;
		commands.reserve(packet.commands.size());
		for(const CommandSpan& span : packet.commands) {
			if(span.offset > packet_size || span.command_bytes > packet_size - span.offset ||
				span.command_bytes < RV_COMMAND_HEADER_BYTES) return RV_RESULT_BAD_PACKET;
			const uint8_t* payload = packet_bytes + span.offset + RV_COMMAND_HEADER_BYTES;
			const uint32_t payload_bytes = span.command_bytes - RV_COMMAND_HEADER_BYTES;
			const RvResult valid = validate_graphics_command_payload(span.opcode, payload, payload_bytes);
			if(valid != RV_RESULT_OK) return valid;
			GraphicsCommand command{};
			command.opcode = span.opcode;
			switch(span.opcode) {
			case RV_OP_CREATE_SURFACE:
				command.surface = read_u64(payload);
				command.logical_width = read_u32(payload + 8);
				command.logical_height = read_u32(payload + 12);
				command.storage_scale = read_u32(payload + 16);
				break;
			case RV_OP_BEGIN_UPLOAD:
				command.surface = read_u64(payload);
				command.frame_desc = read_frame_desc(payload + 8);
				command.total_pixel_bytes = read_u32(payload + 64);
				break;
			case RV_OP_UPLOAD_ROWS:
				command.surface = read_u64(payload);
				command.first_row = read_u32(payload + 8);
				command.row_count = read_u32(payload + 12);
				command.source_stride_bytes = read_u32(payload + 16);
				command.data_bytes = read_u32(payload + 20);
				command.data_offset = static_cast<uint32_t>(
					span.offset + RV_COMMAND_HEADER_BYTES + kUploadRowsHeaderBytes);
				break;
			case RV_OP_END_UPLOAD:
				command.surface = read_u64(payload);
				break;
			case RV_OP_FILL:
			case RV_OP_BLIT:
			case RV_OP_COPY:
			case RV_OP_DESTROY_SURFACE: {
				std::vector<SurfaceCommand> decoded;
				ValidatedPacket one{};
				one.commands.push_back(span);
				const RvResult surface_status = decode_surface_commands(
					packet_bytes, packet_size, one, &decoded);
				if(surface_status != RV_RESULT_OK || decoded.size() != 1) return surface_status;
				command.surface_command = decoded[0];
				break;
			}
			case RV_OP_COMMIT_FRAME:
				command.surface = read_u64(payload);
				command.frame_sequence = read_u64(payload + 8);
				command.desired_scale = read_u32(payload + 16);
				command.content = read_rect(payload + 20);
				break;
			case RV_OP_CLEAR_FRAME:
				break;
			case RV_OP_SCALE_SURFACE:
				command.scale_source = read_u64(payload);
				command.scale_destination = read_u64(payload + 8);
				command.scale_target = read_u32(payload + 16);
				break;
			default:
				return RV_RESULT_UNSUPPORTED_OPERATION;
			}
			commands.push_back(command);
		}
		*out = std::move(commands);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

} // namespace revival::graphics
