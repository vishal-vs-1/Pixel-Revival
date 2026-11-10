#include "texture_store.hpp"

#include <limits>
#include <new>
#include <utility>

#include "frame_desc.hpp"
#include "pixel_transfer.hpp"

namespace revival::graphics {
namespace {

bool valid_storage_scale(uint32_t scale) noexcept
{
	return scale == RV_SCALE_ORIGINAL || scale == RV_SCALE_2X || scale == RV_SCALE_4X;
}

uint64_t surface_storage_bytes(uint32_t width, uint32_t height) noexcept
{
	return static_cast<uint64_t>(width) * height * 4;
}

} // namespace

TextureStore::TextureStore(TextureDevice& device, uint64_t texture_budget_bytes) noexcept
	: device_(device), texture_budget_bytes_(texture_budget_bytes)
{
}

RvResult TextureStore::create_surface(
	RvSurfaceHandle handle, uint32_t logical_width,
	uint32_t logical_height, uint32_t storage_scale) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(handle == 0 || logical_width == 0 || logical_height == 0 ||
		!valid_storage_scale(storage_scale)) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(static_cast<uint64_t>(logical_width) * logical_height > RV_MAX_LOGICAL_SOURCE_PIXELS) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	const uint64_t physical_width = static_cast<uint64_t>(logical_width) * storage_scale;
	const uint64_t physical_height = static_cast<uint64_t>(logical_height) * storage_scale;
	if(physical_width > UINT32_MAX || physical_height > UINT32_MAX) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	uint32_t max_dimension = 0;
	const RvResult device_result = device_.max_texture_dimension(&max_dimension);
	if(device_result != RV_RESULT_OK) {
		return device_result;
	}
	if(physical_width > max_dimension || physical_height > max_dimension) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	const uint64_t bytes = surface_storage_bytes(
		static_cast<uint32_t>(physical_width), static_cast<uint32_t>(physical_height));
	if(resident_bytes_ > texture_budget_bytes_ || bytes > texture_budget_bytes_ - resident_bytes_) {
		return RV_RESULT_RESOURCE_LIMIT;
	}

	try {
		auto inserted = surfaces_.try_emplace(handle);
		if(!inserted.second) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		TextureAllocation allocation{};
		const RvResult create_result = device_.create_rgba8(
			static_cast<uint32_t>(physical_width), static_cast<uint32_t>(physical_height), &allocation);
		if(create_result != RV_RESULT_OK) {
			surfaces_.erase(inserted.first);
			return create_result;
		}
		if(allocation.texture == 0 || allocation.framebuffer == 0 ||
			allocation.width != physical_width || allocation.height != physical_height ||
			allocation.storage_bytes != bytes) {
			device_.destroy(allocation);
			surfaces_.erase(inserted.first);
			return RV_RESULT_GL_FAILURE;
		}
		Surface& surface = inserted.first->second;
		surface.logical_width = logical_width;
		surface.logical_height = logical_height;
		surface.storage_scale = storage_scale;
		surface.active = allocation;
		resident_bytes_ += bytes;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult TextureStore::create_upload_surface(RvSurfaceHandle handle,
	const RvFrameDesc& desc, uint32_t total_source_bytes) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(handle == 0) return RV_RESULT_INVALID_ARGUMENT;
	const RvResult valid = validate_frame_desc(desc, total_source_bytes);
	if(valid != RV_RESULT_OK) return valid;
	try {
		auto inserted = surfaces_.try_emplace(handle);
		if(!inserted.second) return RV_RESULT_INVALID_ARGUMENT;
		Surface& surface = inserted.first->second;
		surface.logical_width = desc.logical_width;
		surface.logical_height = desc.logical_height;
		surface.storage_scale = desc.source_scale;
		const RvResult upload = begin_upload(handle, desc, total_source_bytes);
		if(upload != RV_RESULT_OK) {
			surfaces_.erase(inserted.first);
			return upload;
		}
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult TextureStore::begin_upload(
	RvSurfaceHandle handle, const RvFrameDesc& desc,
	uint32_t total_source_bytes) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	Surface& surface = found->second;
	if(surface.pending.allocation.texture != 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const RvResult valid = validate_frame_desc(desc, total_source_bytes);
	if(valid != RV_RESULT_OK) {
		return valid;
	}
	if(desc.logical_width != surface.logical_width || desc.logical_height != surface.logical_height ||
		desc.source_scale != surface.storage_scale ||
		(surface.active.texture != 0 && (desc.physical_width != surface.active.width ||
			desc.physical_height != surface.active.height)) ||
		(surface.active.texture == 0 && (surface.initialized || surface.generation != 0))) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint64_t bytes = surface_storage_bytes(desc.physical_width, desc.physical_height);
	if(resident_bytes_ > texture_budget_bytes_ || bytes > texture_budget_bytes_ - resident_bytes_) {
		return RV_RESULT_RESOURCE_LIMIT;
	}

	try {
		PendingUpload pending{};
		pending.desc = desc;
		pending.row_coverage.resize(desc.physical_height, 0);
		const RvResult create_result = device_.create_rgba8(
			desc.physical_width, desc.physical_height, &pending.allocation);
		if(create_result != RV_RESULT_OK) {
			return create_result;
		}
		if(pending.allocation.texture == 0 || pending.allocation.framebuffer == 0 ||
			pending.allocation.width != desc.physical_width ||
			pending.allocation.height != desc.physical_height ||
			pending.allocation.storage_bytes != bytes) {
			device_.destroy(pending.allocation);
			return RV_RESULT_GL_FAILURE;
		}
		surface.pending = std::move(pending);
		resident_bytes_ += bytes;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

RvResult TextureStore::upload_rows(
	RvSurfaceHandle handle, uint32_t first_row, uint32_t row_count,
	uint32_t source_stride_bytes, const uint8_t* source,
	uint32_t source_bytes) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	Surface& surface = found->second;
	PendingUpload& pending = surface.pending;
	if(pending.allocation.texture == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(row_count == 0 || first_row > pending.desc.physical_height ||
		row_count > pending.desc.physical_height - first_row ||
		source_stride_bytes != pending.desc.physical_stride_bytes) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	for(uint32_t row = first_row; row < first_row + row_count; ++row) {
		if(pending.row_coverage[row] != 0) {
			return RV_RESULT_BAD_PACKET;
		}
	}

	std::vector<uint8_t> converted;
	const RvResult conversion = convert_rows_to_gl_rgba(
		pending.desc, first_row, row_count, source_stride_bytes, source,
		source_bytes, &converted);
	if(conversion != RV_RESULT_OK) {
		return conversion;
	}
	const uint32_t gl_first_row = pending.desc.physical_height - first_row - row_count;
	const RvResult upload_result = device_.upload_rgba8_rows(
		pending.allocation, gl_first_row, row_count, converted.data(),
		static_cast<uint32_t>(converted.size()));
	if(upload_result != RV_RESULT_OK) {
		if(upload_result == RV_RESULT_GL_FAILURE) {
			if(pending.allocation.storage_bytes > resident_bytes_) {
				return RV_RESULT_INTERNAL_ERROR;
			}
			device_.destroy(pending.allocation);
			resident_bytes_ -= pending.allocation.storage_bytes;
			pending = PendingUpload{};
		}
		return upload_result;
	}
	for(uint32_t row = first_row; row < first_row + row_count; ++row) {
		pending.row_coverage[row] = 1;
	}
	pending.covered_rows += row_count;
	return RV_RESULT_OK;
}

RvResult TextureStore::end_upload(RvSurfaceHandle handle) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	Surface& surface = found->second;
	PendingUpload& pending = surface.pending;
	if(pending.allocation.texture == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(pending.covered_rows != pending.desc.physical_height) {
		return RV_RESULT_BAD_PACKET;
	}
	if(surface.generation == UINT64_MAX) return RV_RESULT_RESOURCE_LIMIT;
	const TextureAllocation old = surface.active;
	if(old.storage_bytes > resident_bytes_) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	surface.active = pending.allocation;
	pending = PendingUpload{};
	surface.initialized = true;
	++surface.generation;
	resident_bytes_ -= old.storage_bytes;
	device_.destroy(old);
	return RV_RESULT_OK;
}

RvResult TextureStore::abort_upload(RvSurfaceHandle handle) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	PendingUpload& pending = found->second.pending;
	if(pending.allocation.texture == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const TextureAllocation allocation = pending.allocation;
	if(allocation.storage_bytes > resident_bytes_) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	device_.destroy(allocation);
	resident_bytes_ -= allocation.storage_bytes;
	pending = PendingUpload{};
	return RV_RESULT_OK;
}

RvResult TextureStore::destroy_surface(RvSurfaceHandle handle) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	Surface& surface = found->second;
	const uint64_t bytes = surface.active.storage_bytes + surface.pending.allocation.storage_bytes;
	if(bytes > resident_bytes_) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	if(surface.pending.allocation.texture != 0) {
		device_.destroy(surface.pending.allocation);
	}
	device_.destroy(surface.active);
	resident_bytes_ -= bytes;
	surfaces_.erase(found);
	return RV_RESULT_OK;
}

RvResult TextureStore::check_owner() const noexcept
{
	return device_.check_owner();
}

RvResult TextureStore::mark_initialized(RvSurfaceHandle handle) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) return RV_RESULT_STALE_HANDLE;
	found->second.initialized = true;
	return RV_RESULT_OK;
}

RvResult TextureStore::mark_modified(RvSurfaceHandle handle) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) return RV_RESULT_STALE_HANDLE;
	if(found->second.generation == UINT64_MAX) return RV_RESULT_RESOURCE_LIMIT;
	found->second.initialized = true;
	++found->second.generation;
	return RV_RESULT_OK;
}

RvResult TextureStore::reserve_scratch(RvSurfaceHandle source, TextureAllocation* out) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*out = TextureAllocation{};
	auto found = surfaces_.find(source);
	if(found == surfaces_.end()) return RV_RESULT_STALE_HANDLE;
	const TextureAllocation& active = found->second.active;
	if(active.texture == 0 || active.width == 0 || active.height == 0) {
		return RV_RESULT_INTERNAL_ERROR;
	}
	return reserve_temporary(active.width, active.height, out);
}

RvResult TextureStore::reserve_temporary(
	uint32_t width, uint32_t height, TextureAllocation* out) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(out == nullptr || width == 0 || height == 0) return RV_RESULT_INVALID_ARGUMENT;
	*out = TextureAllocation{};
	uint32_t max_dimension = 0;
	const RvResult max_result = device_.max_texture_dimension(&max_dimension);
	if(max_result != RV_RESULT_OK) return max_result;
	if(width > max_dimension || height > max_dimension) return RV_RESULT_RESOURCE_LIMIT;
	if(static_cast<uint64_t>(width) * height >
		std::numeric_limits<uint64_t>::max() / 4) return RV_RESULT_RESOURCE_LIMIT;
	const uint64_t bytes = surface_storage_bytes(width, height);
	if(resident_bytes_ > texture_budget_bytes_ ||
		bytes > texture_budget_bytes_ - resident_bytes_) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	TextureAllocation scratch{};
	const RvResult created = device_.create_rgba8(width, height, &scratch);
	if(created != RV_RESULT_OK) return created;
	if(scratch.texture == 0 || scratch.framebuffer == 0 ||
		scratch.width != width || scratch.height != height ||
		scratch.storage_bytes != bytes) {
		device_.destroy(scratch);
		return RV_RESULT_GL_FAILURE;
	}
	resident_bytes_ += scratch.storage_bytes;
	*out = scratch;
	return RV_RESULT_OK;
}

RvResult TextureStore::release_scratch(const TextureAllocation& allocation) noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(allocation.texture == 0 || allocation.framebuffer == 0) return RV_RESULT_INVALID_ARGUMENT;
	if(allocation.storage_bytes > resident_bytes_) return RV_RESULT_INTERNAL_ERROR;
	device_.destroy(allocation);
	resident_bytes_ -= allocation.storage_bytes;
	return RV_RESULT_OK;
}

void TextureStore::clear() noexcept
{
	if(device_.check_owner() != RV_RESULT_OK) return;
	for(auto& entry : surfaces_) {
		Surface& surface = entry.second;
		if(surface.pending.allocation.texture != 0) {
			device_.destroy(surface.pending.allocation);
		}
		device_.destroy(surface.active);
	}
	surfaces_.clear();
	resident_bytes_ = 0;
}

RvResult TextureStore::inspect(
	RvSurfaceHandle handle, TextureSurfaceSnapshot* out) const noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(out == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) {
		return RV_RESULT_STALE_HANDLE;
	}
	const Surface& surface = found->second;
	TextureSurfaceSnapshot snapshot{};
	snapshot.handle = handle;
	snapshot.logical_width = surface.logical_width;
	snapshot.logical_height = surface.logical_height;
	snapshot.storage_scale = surface.storage_scale;
	snapshot.generation = surface.generation;
	snapshot.active = surface.active;
	snapshot.initialized = surface.initialized;
	snapshot.upload_pending = surface.pending.allocation.texture != 0;
	snapshot.pending_rows = surface.pending.covered_rows;
	*out = snapshot;
	return RV_RESULT_OK;
}

RvResult TextureStore::inspect_pending_upload(RvSurfaceHandle handle,
	RvFrameDesc* descriptor, std::vector<uint8_t>* row_coverage) const noexcept
{
	const RvResult owner = device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(descriptor == nullptr || row_coverage == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	const auto found = surfaces_.find(handle);
	if(found == surfaces_.end()) return RV_RESULT_STALE_HANDLE;
	if(found->second.pending.allocation.texture == 0) return RV_RESULT_INVALID_ARGUMENT;
	try {
		*descriptor = found->second.pending.desc;
		*row_coverage = found->second.pending.row_coverage;
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

uint64_t TextureStore::resident_bytes() const noexcept
{
	return resident_bytes_;
}

} // namespace revival::graphics
