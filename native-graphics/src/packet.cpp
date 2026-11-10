#include "packet.hpp"

#include <new>
#include <utility>

#include "command_protocol.hpp"

namespace revival::graphics {
namespace {

uint16_t read_u16_le(const uint8_t* bytes) noexcept
{
	return static_cast<uint16_t>(bytes[0]) |
		static_cast<uint16_t>(static_cast<uint16_t>(bytes[1]) << 8);
}

uint32_t read_u32_le(const uint8_t* bytes) noexcept
{
	return static_cast<uint32_t>(bytes[0]) |
		(static_cast<uint32_t>(bytes[1]) << 8) |
		(static_cast<uint32_t>(bytes[2]) << 16) |
		(static_cast<uint32_t>(bytes[3]) << 24);
}

uint64_t read_u64_le(const uint8_t* bytes) noexcept
{
	return static_cast<uint64_t>(read_u32_le(bytes)) |
		(static_cast<uint64_t>(read_u32_le(bytes + 4)) << 32);
}

bool known_opcode(uint16_t opcode) noexcept
{
	return opcode >= RV_OP_CREATE_SURFACE && opcode <= RV_OP_SCALE_SURFACE;
}

} // namespace

RvResult validate_packet(
	const uint8_t* bytes, uint32_t byte_count, uint64_t expected_epoch,
	ValidatedPacket* out) noexcept
{
	if(out == nullptr || bytes == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*out = ValidatedPacket{};
	if(byte_count < RV_PACKET_HEADER_BYTES) {
		return RV_RESULT_BAD_PACKET;
	}
	if(byte_count > RV_MAX_PACKET_BYTES) {
		return RV_RESULT_RESOURCE_LIMIT;
	}

	const uint32_t magic = read_u32_le(bytes);
	const uint16_t abi_version = read_u16_le(bytes + 4);
	const uint16_t header_bytes = read_u16_le(bytes + 6);
	const uint32_t total_bytes = read_u32_le(bytes + 8);
	const uint32_t command_count = read_u32_le(bytes + 12);
	const uint64_t session_epoch = read_u64_le(bytes + 16);
	const uint64_t packet_sequence = read_u64_le(bytes + 24);

	if(magic != RV_PACKET_MAGIC || abi_version != RV_ABI_VERSION ||
		header_bytes != RV_PACKET_HEADER_BYTES || total_bytes != byte_count) {
		return RV_RESULT_BAD_PACKET;
	}
	if(command_count > RV_MAX_QUEUED_OPS) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	if(session_epoch != expected_epoch) {
		return RV_RESULT_STALE_HANDLE;
	}
	if(packet_sequence == 0) return RV_RESULT_BAD_PACKET;

	try {
		ValidatedPacket parsed;
		parsed.session_epoch = session_epoch;
		parsed.packet_sequence = packet_sequence;
		parsed.commands.reserve(command_count);

		size_t offset = RV_PACKET_HEADER_BYTES;
		for(uint32_t index = 0; index < command_count; ++index) {
			const size_t remaining = static_cast<size_t>(total_bytes) - offset;
			if(remaining < RV_COMMAND_HEADER_BYTES) {
				return RV_RESULT_BAD_PACKET;
			}

			const uint16_t opcode = read_u16_le(bytes + offset);
			const uint16_t flags = read_u16_le(bytes + offset + 2);
			const uint32_t command_bytes = read_u32_le(bytes + offset + 4);
			if(!known_opcode(opcode) || flags != 0 || command_bytes < RV_COMMAND_HEADER_BYTES) {
				return RV_RESULT_BAD_PACKET;
			}
			// Check the untrusted length against remaining bytes before advancing.
			if(command_bytes > remaining) {
				return RV_RESULT_BAD_PACKET;
			}
			const RvResult payload_result = validate_graphics_command_payload(
				opcode, bytes + offset + RV_COMMAND_HEADER_BYTES,
				command_bytes - RV_COMMAND_HEADER_BYTES);
			if(payload_result != RV_RESULT_OK) return payload_result;

			parsed.commands.push_back(CommandSpan{
				opcode, flags, command_bytes, static_cast<uint32_t>(offset)});
			if(opcode == RV_OP_COMMIT_FRAME) {
				++parsed.frame_commits;
			}
			offset += command_bytes;
		}

		if(offset != total_bytes) {
			return RV_RESULT_BAD_PACKET;
		}
		*out = std::move(parsed);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

} // namespace revival::graphics
