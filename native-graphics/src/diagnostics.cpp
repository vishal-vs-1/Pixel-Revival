#include "diagnostics.hpp"

#include <algorithm>
#include <array>
#include <cstring>
#include <limits>
#include <new>
#include <vector>

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>

namespace revival::graphics {
namespace {

using GlEnum = uint32_t;
using GlInt = int32_t;
using GlUint = uint32_t;
using BindFramebufferFn = void (APIENTRY*)(GlEnum, GlUint);
using PixelStoreFn = void (APIENTRY*)(GlEnum, GlInt);
using GetErrorFn = GlEnum (APIENTRY*)();
using GetStringFn = const uint8_t* (APIENTRY*)(GlEnum);

constexpr GlEnum kFramebuffer = 0x8D40;
constexpr GlEnum kFramebufferBinding = 0x8CA6;
constexpr GlEnum kPackAlignment = 0x0D05;
constexpr GlEnum kPackRowLength = 0x0D02;
constexpr GlEnum kPackSkipRows = 0x0D03;
constexpr GlEnum kPackSkipPixels = 0x0D04;
constexpr GlEnum kVendor = 0x1F00;
constexpr GlEnum kRenderer = 0x1F01;
constexpr GlEnum kVersion = 0x1F02;
constexpr GlEnum kRgba = 0x1908;
constexpr GlEnum kUnsignedByte = 0x1401;
constexpr GlEnum kNoError = 0;
constexpr size_t kMaxDriverStringBytes = 4096;

template<typename Function>
Function get_procedure(const GlFunctionTable& functions, GlProcedure procedure) noexcept
{
	return reinterpret_cast<Function>(functions.get(procedure));
}

class ReadbackState final {
public:
	ReadbackState(BindFramebufferFn bind_framebuffer, PixelStoreFn pixel_store) noexcept
		: bind_framebuffer_(bind_framebuffer), pixel_store_(pixel_store)
	{
		::glGetIntegerv(kFramebufferBinding, &framebuffer_);
		::glGetIntegerv(kPackAlignment, &alignment_);
		::glGetIntegerv(kPackRowLength, &row_length_);
		::glGetIntegerv(kPackSkipRows, &skip_rows_);
		::glGetIntegerv(kPackSkipPixels, &skip_pixels_);
	}

	~ReadbackState()
	{
		pixel_store_(kPackAlignment, alignment_);
		pixel_store_(kPackRowLength, row_length_);
		pixel_store_(kPackSkipRows, skip_rows_);
		pixel_store_(kPackSkipPixels, skip_pixels_);
		bind_framebuffer_(kFramebuffer, static_cast<GlUint>(framebuffer_));
	}

private:
	BindFramebufferFn bind_framebuffer_;
	PixelStoreFn pixel_store_;
	GlInt framebuffer_ = 0;
	GlInt alignment_ = 4;
	GlInt row_length_ = 0;
	GlInt skip_rows_ = 0;
	GlInt skip_pixels_ = 0;
};

bool driver_string(const GlFunctionTable& functions, GlEnum name,
	const uint8_t** out, size_t* out_bytes) noexcept
{
	const auto get_string = get_procedure<GetStringFn>(functions, GlProcedure::GetString);
	if(get_string == nullptr || out == nullptr || out_bytes == nullptr) return false;
	const uint8_t* value = get_string(name);
	if(value == nullptr) return false;
	for(size_t length = 0; length < kMaxDriverStringBytes; ++length) {
		if(value[length] == 0) {
			*out = value;
			*out_bytes = length + 1;
			return true;
		}
	}
	return false;
}

uint8_t unpremultiply(uint8_t component, uint8_t alpha) noexcept
{
	if(alpha == 0) return 0;
	return static_cast<uint8_t>(std::min<uint32_t>(255,
		(static_cast<uint32_t>(component) * 255 + alpha / 2) / alpha));
}

} // namespace

GraphicsDiagnostics::GraphicsDiagnostics(const GlFunctionTable& functions) noexcept
	: functions_(functions)
{
}

RvResult GraphicsDiagnostics::device_strings(char* vendor, uint32_t vendor_capacity,
	char* renderer, uint32_t renderer_capacity,
	char* version, uint32_t version_capacity) const noexcept
{
	if(vendor == nullptr || renderer == nullptr || version == nullptr ||
		vendor_capacity == 0 || renderer_capacity == 0 || version_capacity == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	vendor[0] = '\0';
	renderer[0] = '\0';
	version[0] = '\0';
	std::array<const uint8_t*, 3> values{};
	std::array<size_t, 3> sizes{};
	if(!driver_string(functions_, kVendor, &values[0], &sizes[0]) ||
		!driver_string(functions_, kRenderer, &values[1], &sizes[1]) ||
		!driver_string(functions_, kVersion, &values[2], &sizes[2])) {
		return RV_RESULT_GL_FAILURE;
	}
	if(sizes[0] > vendor_capacity || sizes[1] > renderer_capacity || sizes[2] > version_capacity) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	std::memcpy(vendor, values[0], sizes[0]);
	std::memcpy(renderer, values[1], sizes[1]);
	std::memcpy(version, values[2], sizes[2]);
	return RV_RESULT_OK;
}

RvResult GraphicsDiagnostics::readback(const TextureStore& store, RvSurfaceHandle surface,
	const RvReadbackRect& rect, uint32_t rect_bytes, uint8_t* destination,
	uint32_t destination_bytes, uint32_t destination_stride_bytes) const noexcept
{
	if(surface == 0 || destination == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	if(rect_bytes != RV_READBACK_RECT_BYTES || rect.struct_bytes != RV_READBACK_RECT_BYTES ||
		rect.abi_version != RV_ABI_VERSION) return RV_RESULT_UNSUPPORTED_ABI;
	if(rect.width == 0 || rect.height == 0 ||
		(rect.output_format != RV_PIXEL_FORMAT_RGBA8 && rect.output_format != RV_PIXEL_FORMAT_BGRA8) ||
		(rect.output_alpha_mode != RV_ALPHA_OPAQUE && rect.output_alpha_mode != RV_ALPHA_STRAIGHT &&
			rect.output_alpha_mode != RV_ALPHA_PREMULTIPLIED)) return RV_RESULT_INVALID_ARGUMENT;
	const uint64_t row_bytes = static_cast<uint64_t>(rect.width) * 4;
	const uint64_t required_bytes = static_cast<uint64_t>(destination_stride_bytes) * rect.height;
	if(row_bytes > UINT32_MAX || destination_stride_bytes < row_bytes ||
		required_bytes > destination_bytes || required_bytes > RV_MAX_QUEUED_BYTES) {
		return required_bytes > RV_MAX_QUEUED_BYTES ? RV_RESULT_RESOURCE_LIMIT : RV_RESULT_INVALID_ARGUMENT;
	}
	TextureSurfaceSnapshot snapshot{};
	const RvResult inspected = store.inspect(surface, &snapshot);
	if(inspected != RV_RESULT_OK) return inspected;
	if(!snapshot.initialized || snapshot.active.framebuffer == 0 ||
		static_cast<uint64_t>(rect.x) + rect.width > snapshot.active.width ||
		static_cast<uint64_t>(rect.y) + rect.height > snapshot.active.height) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const auto bind_framebuffer = get_procedure<BindFramebufferFn>(functions_, GlProcedure::BindFramebuffer);
	const auto pixel_store = get_procedure<PixelStoreFn>(functions_, GlProcedure::PixelStorei);
	const auto get_error = get_procedure<GetErrorFn>(functions_, GlProcedure::GetError);
	if(bind_framebuffer == nullptr || pixel_store == nullptr || get_error == nullptr) {
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
	try {
		const size_t tight_bytes = static_cast<size_t>(row_bytes) * rect.height;
		if(tight_bytes > RV_MAX_QUEUED_BYTES) return RV_RESULT_RESOURCE_LIMIT;
		std::vector<uint8_t> pixels(tight_bytes);
		ReadbackState saved(bind_framebuffer, pixel_store);
		bind_framebuffer(kFramebuffer, snapshot.active.framebuffer);
		pixel_store(kPackAlignment, 1);
		pixel_store(kPackRowLength, 0);
		pixel_store(kPackSkipRows, 0);
		pixel_store(kPackSkipPixels, 0);
		if(get_error() != kNoError) return RV_RESULT_GL_FAILURE;
		::glReadPixels(static_cast<GLint>(rect.x),
			static_cast<GLint>(snapshot.active.height - rect.y - rect.height),
			static_cast<GLsizei>(rect.width), static_cast<GLsizei>(rect.height),
			kRgba, kUnsignedByte, pixels.data());
		if(get_error() != kNoError) return RV_RESULT_GL_FAILURE;
		const size_t row_size = static_cast<size_t>(row_bytes);
		for(uint32_t y = 0; y < rect.height / 2; ++y) {
			uint8_t* top = pixels.data() + static_cast<size_t>(y) * row_size;
			uint8_t* bottom = pixels.data() + static_cast<size_t>(rect.height - y - 1) * row_size;
			for(size_t x = 0; x < row_size; ++x) std::swap(top[x], bottom[x]);
		}
		for(size_t offset = 0; offset < pixels.size(); offset += 4) {
			uint8_t* pixel = pixels.data() + offset;
			uint8_t alpha = pixel[3];
			uint8_t red = pixel[0];
			uint8_t blue = pixel[2];
			if(rect.output_alpha_mode != RV_ALPHA_PREMULTIPLIED) {
				red = unpremultiply(red, alpha);
				blue = unpremultiply(blue, alpha);
				pixel[1] = unpremultiply(pixel[1], alpha);
				if(rect.output_alpha_mode == RV_ALPHA_OPAQUE) alpha = 255;
			}
			pixel[0] = rect.output_format == RV_PIXEL_FORMAT_RGBA8 ? red : blue;
			pixel[2] = rect.output_format == RV_PIXEL_FORMAT_RGBA8 ? blue : red;
			pixel[3] = alpha;
		}
		for(uint32_t y = 0; y < rect.height; ++y) {
			std::memcpy(destination + static_cast<size_t>(y) * destination_stride_bytes,
				pixels.data() + static_cast<size_t>(y) * row_size, row_size);
		}
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

} // namespace revival::graphics
