#ifndef REVIVAL_GRAPHICS_TEXTURE_DEVICE_HPP
#define REVIVAL_GRAPHICS_TEXTURE_DEVICE_HPP

#include <cstdint>
#include <thread>

#include "gl_functions.hpp"
#include "revival_graphics.h"

namespace revival::graphics {

struct TextureAllocation {
	uint32_t texture = 0;
	uint32_t framebuffer = 0;
	uint32_t width = 0;
	uint32_t height = 0;
	uint64_t storage_bytes = 0;
};

class TextureDevice {
public:
	virtual ~TextureDevice() = default;
	virtual RvResult check_owner() const noexcept = 0;
	virtual RvResult max_texture_dimension(uint32_t* out_dimension) noexcept = 0;
	virtual RvResult create_rgba8(
		uint32_t width, uint32_t height, TextureAllocation* out) noexcept = 0;
	virtual RvResult upload_rgba8_rows(
		const TextureAllocation& allocation, uint32_t first_gl_row,
		uint32_t row_count, const uint8_t* bottom_up_rgba,
		uint32_t byte_count) noexcept = 0;
	virtual void destroy(const TextureAllocation& allocation) noexcept = 0;
};

class OpenGlTextureDevice final : public TextureDevice {
public:
	explicit OpenGlTextureDevice(const GlFunctionTable& functions) noexcept;

	RvResult check_owner() const noexcept override;
	RvResult max_texture_dimension(uint32_t* out_dimension) noexcept override;
	RvResult create_rgba8(
		uint32_t width, uint32_t height, TextureAllocation* out) noexcept override;
	RvResult upload_rgba8_rows(
		const TextureAllocation& allocation, uint32_t first_gl_row,
		uint32_t row_count, const uint8_t* bottom_up_rgba,
		uint32_t byte_count) noexcept override;
	void destroy(const TextureAllocation& allocation) noexcept override;

private:
	const GlFunctionTable& functions_;
	const std::thread::id owner_thread_;
	void* const native_context_;
};

} // namespace revival::graphics

#endif
