#include "surface_ops.hpp"

#include <algorithm>
#include <new>
#include <unordered_map>

namespace revival::graphics {
namespace {

struct IntBounds {
	int64_t left = 0;
	int64_t top = 0;
	int64_t right = 0;
	int64_t bottom = 0;
};

IntBounds intersect(IntBounds first, IntBounds second) noexcept
{
	return IntBounds{std::max(first.left, second.left), std::max(first.top, second.top),
		std::min(first.right, second.right), std::min(first.bottom, second.bottom)};
}

IntBounds bounds(const SurfaceRect& rect) noexcept
{
	return IntBounds{rect.x, rect.y,
		static_cast<int64_t>(rect.x) + rect.width,
		static_cast<int64_t>(rect.y) + rect.height};
}

bool has_visible_pixels(
	const SurfaceRect& destination, const SurfaceRect& clip,
	uint32_t logical_width, uint32_t logical_height) noexcept
{
	const IntBounds surface{0, 0, logical_width, logical_height};
	const IntBounds visible = intersect(intersect(bounds(destination), bounds(clip)), surface);
	return visible.right > visible.left && visible.bottom > visible.top;
}

bool source_rectangle_inside(const SurfaceRect& rect, uint32_t width, uint32_t height) noexcept
{
	return rect.x >= 0 && rect.y >= 0 && rect.width > 0 && rect.height > 0 &&
		static_cast<int64_t>(rect.x) + rect.width <= width &&
		static_cast<int64_t>(rect.y) + rect.height <= height;
}

class ScratchAllocation {
public:
	explicit ScratchAllocation(TextureStore& store) noexcept : store_(store) {}
	~ScratchAllocation()
	{
		if(allocation_.texture != 0) store_.release_scratch(allocation_);
	}
	RvResult reserve(RvSurfaceHandle source) noexcept
	{
		return store_.reserve_scratch(source, &allocation_);
	}
	const TextureAllocation& get() const noexcept { return allocation_; }
	RvResult release() noexcept
	{
		if(allocation_.texture == 0) return RV_RESULT_OK;
		const RvResult result = store_.release_scratch(allocation_);
		if(result == RV_RESULT_OK) allocation_ = TextureAllocation{};
		return result;
	}

private:
	TextureStore& store_;
	TextureAllocation allocation_{};
};

} // namespace

SurfaceOperations::SurfaceOperations(TextureStore& store, SurfaceRenderer& renderer) noexcept
	: store_(store), renderer_(renderer)
{
}

RvResult SurfaceOperations::preflight(const std::vector<SurfaceCommand>& commands) const noexcept
{
	try {
		struct VirtualSurface {
			bool exists = true;
			bool initialized = false;
		};
		std::unordered_map<RvSurfaceHandle, VirtualSurface> states;
		states.reserve(commands.size() * 2);
		auto get_state = [&](RvSurfaceHandle handle, VirtualSurface** state) -> RvResult {
			auto found = states.find(handle);
			if(found != states.end()) {
				if(!found->second.exists) return RV_RESULT_STALE_HANDLE;
				*state = &found->second;
				return RV_RESULT_OK;
			}
			TextureSurfaceSnapshot snapshot{};
			const RvResult inspected = store_.inspect(handle, &snapshot);
			if(inspected != RV_RESULT_OK) return inspected;
			auto inserted = states.emplace(handle, VirtualSurface{true, snapshot.initialized});
			*state = &inserted.first->second;
			return RV_RESULT_OK;
		};

		for(const SurfaceCommand& command : commands) {
			const RvResult schema = validate_surface_command(command);
			if(schema != RV_RESULT_OK) return schema;
			if(command.opcode == RV_OP_DESTROY_SURFACE) {
				VirtualSurface* state = nullptr;
				const RvResult inspected = get_state(command.destination, &state);
				if(inspected != RV_RESULT_OK) return inspected;
				state->exists = false;
				continue;
			}
			if(command.opcode == RV_OP_FILL) {
				VirtualSurface* destination_state = nullptr;
				const RvResult inspected = get_state(command.destination, &destination_state);
				if(inspected != RV_RESULT_OK) return inspected;
				TextureSurfaceSnapshot destination{};
				const RvResult dimensions = store_.inspect(command.destination, &destination);
				if(dimensions != RV_RESULT_OK) return dimensions;
				if(has_visible_pixels(command.destination_rect, command.clip,
				destination.logical_width, destination.logical_height)) {
					destination_state->initialized = true;
				}
				continue;
			}
			if(command.opcode != RV_OP_BLIT && command.opcode != RV_OP_COPY) {
				return RV_RESULT_UNSUPPORTED_OPERATION;
			}
			VirtualSurface* source_state = nullptr;
			VirtualSurface* destination_state = nullptr;
			RvResult inspected = get_state(command.source, &source_state);
			if(inspected != RV_RESULT_OK) return inspected;
			inspected = get_state(command.destination, &destination_state);
			if(inspected != RV_RESULT_OK) return inspected;
			TextureSurfaceSnapshot source{};
			TextureSurfaceSnapshot destination{};
			inspected = store_.inspect(command.source, &source);
			if(inspected != RV_RESULT_OK) return inspected;
			inspected = store_.inspect(command.destination, &destination);
			if(inspected != RV_RESULT_OK) return inspected;
			if(!source_rectangle_inside(command.source_rect,
			source.logical_width, source.logical_height)) {
				return RV_RESULT_INVALID_ARGUMENT;
			}
			if(!has_visible_pixels(command.destination_rect, command.clip,
				destination.logical_width, destination.logical_height)) {
				continue;
			}
			if(command.source != command.destination && !source_state->initialized) {
				return RV_RESULT_INVALID_ARGUMENT;
			}
			destination_state->initialized = true;
		}
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult SurfaceOperations::execute(const std::vector<SurfaceCommand>& commands) noexcept
{
	const RvResult owner = store_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	const RvResult valid = preflight(commands);
	if(valid != RV_RESULT_OK) return valid;
	for(const SurfaceCommand& command : commands) {
		const RvResult executed = execute_one(command);
		if(executed != RV_RESULT_OK) return executed;
	}
	return RV_RESULT_OK;
}

RvResult SurfaceOperations::execute_one(const SurfaceCommand& command) noexcept
{
	if(command.opcode == RV_OP_DESTROY_SURFACE) {
		return store_.destroy_surface(command.destination);
	}
	TextureSurfaceSnapshot destination{};
	RvResult result = store_.inspect(command.destination, &destination);
	if(result != RV_RESULT_OK) return result;
	if(command.opcode == RV_OP_FILL) {
		if(!has_visible_pixels(command.destination_rect, command.clip,
			destination.logical_width, destination.logical_height)) {
			return RV_RESULT_OK;
		}
		if(!destination.initialized) {
			result = renderer_.clear_transparent(destination);
			if(result != RV_RESULT_OK) return result;
			result = store_.mark_initialized(command.destination);
			if(result != RV_RESULT_OK) return result;
		}
		result = renderer_.fill(destination, command.destination_rect, command.clip, command.argb);
		return result == RV_RESULT_OK ? store_.mark_modified(command.destination) : result;
	}
	if(command.opcode != RV_OP_BLIT && command.opcode != RV_OP_COPY) {
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
	TextureSurfaceSnapshot source{};
	result = store_.inspect(command.source, &source);
	if(result != RV_RESULT_OK) return result;
	if(!has_visible_pixels(command.destination_rect, command.clip,
		destination.logical_width, destination.logical_height)) {
		return RV_RESULT_OK;
	}
	bool destination_cleared = false;
	if(command.source == command.destination && !source.initialized) {
		result = renderer_.clear_transparent(destination);
		if(result != RV_RESULT_OK) return result;
		result = store_.mark_initialized(command.destination);
		if(result != RV_RESULT_OK) return result;
		source.initialized = true;
		destination.initialized = true;
		destination_cleared = true;
	}
	if(!destination.initialized && !destination_cleared) {
		result = renderer_.clear_transparent(destination);
		if(result != RV_RESULT_OK) return result;
		result = store_.mark_initialized(command.destination);
		if(result != RV_RESULT_OK) return result;
		destination.initialized = true;
	}
	TextureAllocation source_allocation = source.active;
	ScratchAllocation scratch(store_);
	if(command.source == command.destination) {
		result = scratch.reserve(command.source);
		if(result != RV_RESULT_OK) return result;
		result = renderer_.copy_surface(source.active, scratch.get());
		if(result != RV_RESULT_OK) return result;
		source_allocation = scratch.get();
	}
	result = renderer_.blit(source_allocation, source.logical_width,
		source.logical_height, source.storage_scale, destination,
		command.source_rect, command.destination_rect, command.clip,
		command.transform, command.blend);
	const RvResult release = scratch.release();
	if(result == RV_RESULT_OK) result = store_.mark_modified(command.destination);
	return result != RV_RESULT_OK ? result : release;
}

} // namespace revival::graphics
