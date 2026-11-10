#include "command_consumer.hpp"

#include <algorithm>
#include <new>
#include <unordered_map>
#include <utility>

namespace revival::graphics {

struct CommandConsumer::VirtualSurface {
	bool exists = false;
	bool initialized = false;
	bool upload_pending = false;
	uint32_t width = 0;
	uint32_t height = 0;
	uint32_t storage_scale = 0;
	RvFrameDesc pending_desc{};
	std::vector<uint8_t> row_coverage;
};

namespace {

struct VirtualState {
	std::unordered_map<RvSurfaceHandle, CommandConsumer::VirtualSurface> surfaces;
	uint64_t last_surface_handle = 0;
	uint64_t last_frame_sequence = 0;
};

bool visible(const SurfaceRect& destination, const SurfaceRect& clip,
	uint32_t width, uint32_t height) noexcept
{
	const int64_t left = std::max<int64_t>({0, destination.x, clip.x});
	const int64_t top = std::max<int64_t>({0, destination.y, clip.y});
	const int64_t right = std::min<int64_t>({static_cast<int64_t>(width),
		static_cast<int64_t>(destination.x) + destination.width,
		static_cast<int64_t>(clip.x) + clip.width});
	const int64_t bottom = std::min<int64_t>({static_cast<int64_t>(height),
		static_cast<int64_t>(destination.y) + destination.height,
		static_cast<int64_t>(clip.y) + clip.height});
	return right > left && bottom > top;
}

bool source_rect_inside(const SurfaceRect& rectangle, uint32_t width, uint32_t height) noexcept
{
	return rectangle.x >= 0 && rectangle.y >= 0 && rectangle.width > 0 && rectangle.height > 0 &&
		static_cast<int64_t>(rectangle.x) + rectangle.width <= width &&
		static_cast<int64_t>(rectangle.y) + rectangle.height <= height;
}

RvResult get_surface(TextureStore& textures,
	std::unordered_map<RvSurfaceHandle, CommandConsumer::VirtualSurface>& states,
	RvSurfaceHandle handle, CommandConsumer::VirtualSurface** out)
{
	auto found = states.find(handle);
	if(found != states.end()) {
		if(!found->second.exists) return RV_RESULT_STALE_HANDLE;
		*out = &found->second;
		return RV_RESULT_OK;
	}
	TextureSurfaceSnapshot snapshot{};
	const RvResult inspected = textures.inspect(handle, &snapshot);
	if(inspected != RV_RESULT_OK) return inspected;
	CommandConsumer::VirtualSurface state{};
	state.exists = true;
	state.initialized = snapshot.initialized;
	state.width = snapshot.logical_width;
	state.height = snapshot.logical_height;
	state.storage_scale = snapshot.storage_scale;
	state.upload_pending = snapshot.upload_pending;
	if(snapshot.upload_pending) {
		const RvResult pending = textures.inspect_pending_upload(handle,
			&state.pending_desc, &state.row_coverage);
		if(pending != RV_RESULT_OK) return pending;
	}
	auto inserted = states.emplace(handle, std::move(state));
	*out = &inserted.first->second;
	return RV_RESULT_OK;
}

} // namespace

CommandConsumer::CommandConsumer(TextureStore& textures, SurfaceOperations& surfaces,
	FramePresenter& presenter, Scale2xRenderer& scaler,
	InternalIdAllocator allocate_internal_id)
	: textures_(textures), surfaces_(surfaces), presenter_(presenter),
	  scaler_(scaler),
	  allocate_internal_id_(std::move(allocate_internal_id)),
	  last_frame_sequence_(presenter.last_frame_sequence())
{
}

RvResult CommandConsumer::preflight(const std::vector<GraphicsCommand>& commands,
	uint64_t* out_last_surface, uint64_t* out_last_frame) const noexcept
{
	if(out_last_surface == nullptr || out_last_frame == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	try {
		VirtualState state{};
		state.surfaces.reserve(commands.size() * 2);
		state.last_surface_handle = last_surface_handle_;
		state.last_frame_sequence = last_frame_sequence_;
		for(const GraphicsCommand& command : commands) {
			if(command.opcode == RV_OP_CREATE_SURFACE) {
				constexpr RvSurfaceHandle internal_handle_bit = UINT64_C(1) << 63;
				if(command.surface >= internal_handle_bit ||
					command.surface <= state.last_surface_handle) return RV_RESULT_INVALID_ARGUMENT;
				auto found = state.surfaces.find(command.surface);
				if(found != state.surfaces.end() && found->second.exists) return RV_RESULT_INVALID_ARGUMENT;
				TextureSurfaceSnapshot existing{};
				const RvResult inspected = textures_.inspect(command.surface, &existing);
				if(inspected == RV_RESULT_OK) return RV_RESULT_INVALID_ARGUMENT;
				if(inspected != RV_RESULT_STALE_HANDLE) return inspected;
				VirtualSurface created{};
				created.exists = true;
				created.width = command.logical_width;
				created.height = command.logical_height;
				created.storage_scale = command.storage_scale;
				state.surfaces[command.surface] = std::move(created);
				state.last_surface_handle = command.surface;
				continue;
			}
			if(command.opcode == RV_OP_BEGIN_UPLOAD) {
				VirtualSurface* surface = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					command.surface, &surface);
				if(found != RV_RESULT_OK) return found;
				if(surface->upload_pending || surface->width != command.frame_desc.logical_width ||
					surface->height != command.frame_desc.logical_height ||
					surface->storage_scale != command.frame_desc.source_scale) return RV_RESULT_INVALID_ARGUMENT;
				surface->upload_pending = true;
				surface->pending_desc = command.frame_desc;
				surface->row_coverage.assign(command.frame_desc.physical_height, 0);
				continue;
			}
			if(command.opcode == RV_OP_UPLOAD_ROWS) {
				VirtualSurface* surface = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					command.surface, &surface);
				if(found != RV_RESULT_OK) return found;
				const RvFrameDesc& desc = surface->pending_desc;
				if(!surface->upload_pending || command.source_stride_bytes != desc.physical_stride_bytes ||
					command.first_row > desc.physical_height ||
					command.row_count > desc.physical_height - command.first_row) return RV_RESULT_INVALID_ARGUMENT;
				for(uint32_t row = command.first_row; row < command.first_row + command.row_count; ++row) {
					if(surface->row_coverage[row] != 0) return RV_RESULT_BAD_PACKET;
					surface->row_coverage[row] = 1;
				}
				continue;
			}
			if(command.opcode == RV_OP_END_UPLOAD) {
				VirtualSurface* surface = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					command.surface, &surface);
				if(found != RV_RESULT_OK) return found;
				if(!surface->upload_pending || std::find(surface->row_coverage.begin(),
					surface->row_coverage.end(), 0) != surface->row_coverage.end()) return RV_RESULT_BAD_PACKET;
				surface->upload_pending = false;
				surface->initialized = true;
				surface->row_coverage.clear();
				continue;
			}
			if(command.opcode == RV_OP_FILL) {
				const SurfaceCommand& draw = command.surface_command;
				VirtualSurface* destination = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					draw.destination, &destination);
				if(found != RV_RESULT_OK) return found;
				if(visible(draw.destination_rect, draw.clip, destination->width, destination->height)) {
					destination->initialized = true;
				}
				continue;
			}
			if(command.opcode == RV_OP_BLIT || command.opcode == RV_OP_COPY) {
				const SurfaceCommand& draw = command.surface_command;
				VirtualSurface* source = nullptr;
				VirtualSurface* destination = nullptr;
				RvResult found = get_surface(textures_, state.surfaces, draw.source, &source);
				if(found != RV_RESULT_OK) return found;
				found = get_surface(textures_, state.surfaces, draw.destination, &destination);
				if(found != RV_RESULT_OK) return found;
				if(!source_rect_inside(draw.source_rect, source->width, source->height)) {
					return RV_RESULT_INVALID_ARGUMENT;
				}
				if(visible(draw.destination_rect, draw.clip, destination->width, destination->height)) {
					if(draw.source != draw.destination && !source->initialized) return RV_RESULT_INVALID_ARGUMENT;
					destination->initialized = true;
				}
				continue;
			}
			if(command.opcode == RV_OP_DESTROY_SURFACE) {
				VirtualSurface* surface = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					command.surface_command.destination, &surface);
				if(found != RV_RESULT_OK) return found;
				surface->exists = false;
				continue;
			}
			if(command.opcode == RV_OP_COMMIT_FRAME) {
				VirtualSurface* surface = nullptr;
				const RvResult found = get_surface(textures_, state.surfaces,
					command.surface, &surface);
				if(found != RV_RESULT_OK) return found;
				const SurfaceRect& content = command.content;
				if(!surface->initialized || content.x < 0 || content.y < 0 || content.width <= 0 ||
					content.height <= 0 || static_cast<int64_t>(content.x) + content.width > surface->width ||
					static_cast<int64_t>(content.y) + content.height > surface->height ||
					command.frame_sequence <= state.last_frame_sequence) return RV_RESULT_INVALID_ARGUMENT;
				state.last_frame_sequence = command.frame_sequence;
				continue;
			}
			if(command.opcode == RV_OP_SCALE_SURFACE) {
				VirtualSurface* source = nullptr;
				VirtualSurface* destination = nullptr;
				RvResult found = get_surface(textures_, state.surfaces, command.scale_source, &source);
				if(found != RV_RESULT_OK) return found;
				found = get_surface(textures_, state.surfaces, command.scale_destination, &destination);
				if(found != RV_RESULT_OK) return found;
				if(!source->initialized || source->upload_pending || destination->upload_pending ||
					source->width != destination->width || source->height != destination->height ||
					destination->storage_scale != command.scale_target ||
					source->storage_scale >= destination->storage_scale) {
					return RV_RESULT_INVALID_ARGUMENT;
				}
				destination->initialized = true;
				continue;
			}
			if(command.opcode != RV_OP_CLEAR_FRAME) return RV_RESULT_UNSUPPORTED_OPERATION;
		}
		*out_last_surface = state.last_surface_handle;
		*out_last_frame = state.last_frame_sequence;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult CommandConsumer::process(const std::vector<uint8_t>& packet_bytes,
	const ValidatedPacket& packet, CommandExecutionStats* out) noexcept
{
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = CommandExecutionStats{};
	std::vector<GraphicsCommand> commands;
	const RvResult decoded = decode_graphics_commands(packet_bytes.data(),
		static_cast<uint32_t>(packet_bytes.size()), packet, &commands);
	if(decoded != RV_RESULT_OK) return decoded;
	uint64_t next_surface = last_surface_handle_;
	uint64_t next_frame = last_frame_sequence_;
	const RvResult valid = preflight(commands, &next_surface, &next_frame);
	if(valid != RV_RESULT_OK) return valid;
	last_surface_handle_ = next_surface;
	last_frame_sequence_ = next_frame;
	for(const GraphicsCommand& command : commands) {
		const RvResult executed = execute(command, packet_bytes, out);
		if(executed != RV_RESULT_OK) return executed;
		++out->operations_completed;
	}
	return RV_RESULT_OK;
}

RvResult CommandConsumer::execute(const GraphicsCommand& command,
	const std::vector<uint8_t>& packet_bytes, CommandExecutionStats* stats) noexcept
{
	switch(command.opcode) {
	case RV_OP_CREATE_SURFACE:
		return textures_.create_surface(command.surface, command.logical_width,
			command.logical_height, command.storage_scale);
	case RV_OP_BEGIN_UPLOAD:
		return textures_.begin_upload(command.surface, command.frame_desc, command.total_pixel_bytes);
	case RV_OP_UPLOAD_ROWS: {
		if(command.data_offset > packet_bytes.size() ||
			command.data_bytes > packet_bytes.size() - command.data_offset) return RV_RESULT_BAD_PACKET;
		const RvResult uploaded = textures_.upload_rows(command.surface, command.first_row,
			command.row_count, command.source_stride_bytes, packet_bytes.data() + command.data_offset,
			command.data_bytes);
		if(uploaded == RV_RESULT_OK) stats->uploaded_bytes += command.data_bytes;
		return uploaded;
	}
	case RV_OP_END_UPLOAD:
		return textures_.end_upload(command.surface);
	case RV_OP_FILL:
	case RV_OP_BLIT:
	case RV_OP_COPY:
	case RV_OP_DESTROY_SURFACE: {
		std::vector<SurfaceCommand> one{command.surface_command};
		return surfaces_.execute(one);
	}
	case RV_OP_COMMIT_FRAME: {
		if(!allocate_internal_id_) return RV_RESULT_INTERNAL_ERROR;
		const RvSurfaceHandle candidate = allocate_internal_id_();
		if(candidate == 0) return RV_RESULT_RESOURCE_LIMIT;
		ScaleReport report{};
		const RvResult committed = presenter_.commit(command.surface, command.content,
			command.frame_sequence, command.desired_scale, candidate, &report);
		if(committed == RV_RESULT_OK) ++stats->frames_completed;
		return committed;
	}
	case RV_OP_CLEAR_FRAME:
		return presenter_.clear();
	case RV_OP_SCALE_SURFACE: {
		ScaleReport report{};
		return scaler_.apply(command.scale_source, command.scale_destination, &report);
	}
	default:
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
}

} // namespace revival::graphics
