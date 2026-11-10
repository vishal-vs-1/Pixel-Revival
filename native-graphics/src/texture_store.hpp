#ifndef REVIVAL_GRAPHICS_TEXTURE_STORE_HPP
#define REVIVAL_GRAPHICS_TEXTURE_STORE_HPP

#include <cstdint>
#include <unordered_map>
#include <vector>

#include "revival_graphics.h"
#include "texture_device.hpp"

namespace revival::graphics {

struct TextureSurfaceSnapshot {
	RvSurfaceHandle handle = 0;
	uint32_t logical_width = 0;
	uint32_t logical_height = 0;
	uint32_t storage_scale = 0;
	uint64_t generation = 0;
	TextureAllocation active{};
	bool initialized = false;
	bool upload_pending = false;
	uint32_t pending_rows = 0;
};

class TextureStore {
public:
	TextureStore(TextureDevice& device, uint64_t texture_budget_bytes) noexcept;

	// The handle map is context-owned and intentionally has no lock; all calls must use the GL owner thread.
	RvResult create_surface(
		RvSurfaceHandle handle, uint32_t logical_width,
		uint32_t logical_height, uint32_t storage_scale) noexcept;
	RvResult create_upload_surface(RvSurfaceHandle handle, const RvFrameDesc& desc,
		uint32_t total_source_bytes) noexcept;
	RvResult begin_upload(
		RvSurfaceHandle handle, const RvFrameDesc& desc,
		uint32_t total_source_bytes) noexcept;
	RvResult upload_rows(
		RvSurfaceHandle handle, uint32_t first_row, uint32_t row_count,
		uint32_t source_stride_bytes, const uint8_t* source,
		uint32_t source_bytes) noexcept;
	RvResult end_upload(RvSurfaceHandle handle) noexcept;
	RvResult abort_upload(RvSurfaceHandle handle) noexcept;
	RvResult destroy_surface(RvSurfaceHandle handle) noexcept;
	RvResult check_owner() const noexcept;
	RvResult mark_initialized(RvSurfaceHandle handle) noexcept;
	RvResult mark_modified(RvSurfaceHandle handle) noexcept;
	RvResult reserve_scratch(RvSurfaceHandle source, TextureAllocation* out) noexcept;
	RvResult reserve_temporary(uint32_t width, uint32_t height, TextureAllocation* out) noexcept;
	RvResult release_scratch(const TextureAllocation& allocation) noexcept;
	void clear() noexcept;
	RvResult inspect(RvSurfaceHandle handle, TextureSurfaceSnapshot* out) const noexcept;
	RvResult inspect_pending_upload(RvSurfaceHandle handle, RvFrameDesc* descriptor,
		std::vector<uint8_t>* row_coverage) const noexcept;
	uint64_t resident_bytes() const noexcept;

private:
	struct PendingUpload {
		RvFrameDesc desc{};
		TextureAllocation allocation{};
		std::vector<uint8_t> row_coverage;
		uint32_t covered_rows = 0;
	};

	struct Surface {
		uint32_t logical_width = 0;
		uint32_t logical_height = 0;
		uint32_t storage_scale = 0;
		uint64_t generation = 0;
		TextureAllocation active{};
		bool initialized = false;
		PendingUpload pending{};
	};

	TextureDevice& device_;
	const uint64_t texture_budget_bytes_;
	uint64_t resident_bytes_ = 0;
	std::unordered_map<RvSurfaceHandle, Surface> surfaces_;
};

} // namespace revival::graphics

#endif
