#include "texture_device.hpp"

#include <climits>
#include <thread>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>
#endif

namespace revival::graphics {
namespace {

#if defined(_WIN32)
#define RV_GL_CALL __stdcall
#else
#define RV_GL_CALL
#endif

using GlEnum = uint32_t;
using GlInt = int32_t;
using GlSizei = int32_t;
using GlUint = uint32_t;

using GetIntegervFn = void (RV_GL_CALL*)(GlEnum, GlInt*);
using GetErrorFn = GlEnum (RV_GL_CALL*)();
using GenTexturesFn = void (RV_GL_CALL*)(GlSizei, GlUint*);
using BindTextureFn = void (RV_GL_CALL*)(GlEnum, GlUint);
using DeleteTexturesFn = void (RV_GL_CALL*)(GlSizei, const GlUint*);
using TexImage2DFn = void (RV_GL_CALL*)(GlEnum, GlInt, GlInt, GlSizei, GlSizei, GlInt, GlEnum, GlEnum, const void*);
using TexSubImage2DFn = void (RV_GL_CALL*)(GlEnum, GlInt, GlInt, GlInt, GlSizei, GlSizei, GlEnum, GlEnum, const void*);
using TexParameteriFn = void (RV_GL_CALL*)(GlEnum, GlEnum, GlInt);
using PixelStoreiFn = void (RV_GL_CALL*)(GlEnum, GlInt);
using GenFramebuffersFn = void (RV_GL_CALL*)(GlSizei, GlUint*);
using BindFramebufferFn = void (RV_GL_CALL*)(GlEnum, GlUint);
using FramebufferTexture2DFn = void (RV_GL_CALL*)(GlEnum, GlEnum, GlEnum, GlUint, GlInt);
using CheckFramebufferStatusFn = GlEnum (RV_GL_CALL*)(GlEnum);
using DeleteFramebuffersFn = void (RV_GL_CALL*)(GlSizei, const GlUint*);

constexpr GlEnum kTexture2D = 0x0DE1;
constexpr GlEnum kTextureMinFilter = 0x2801;
constexpr GlEnum kTextureMagFilter = 0x2800;
constexpr GlEnum kTextureWrapS = 0x2802;
constexpr GlEnum kTextureWrapT = 0x2803;
constexpr GlInt kNearest = 0x2600;
constexpr GlInt kClampToEdge = 0x812F;
constexpr GlEnum kRgba8 = 0x8058;
constexpr GlEnum kRgba = 0x1908;
constexpr GlEnum kUnsignedByte = 0x1401;
constexpr GlEnum kFramebuffer = 0x8D40;
constexpr GlEnum kColorAttachment0 = 0x8CE0;
constexpr GlEnum kFramebufferComplete = 0x8CD5;
constexpr GlEnum kFramebufferBinding = 0x8CA6;
constexpr GlEnum kTextureBinding2D = 0x8069;
constexpr GlEnum kUnpackAlignment = 0x0CF5;
constexpr GlEnum kMaxTextureSize = 0x0D33;
constexpr GlEnum kNoError = 0;

template<typename Function>
Function procedure(const GlFunctionTable& functions, GlProcedure name) noexcept
{
	return reinterpret_cast<Function>(functions.get(name));
}

class GlBindingSnapshot {
public:
	explicit GlBindingSnapshot(const GlFunctionTable& functions) noexcept
		: get_integer_(procedure<GetIntegervFn>(functions, GlProcedure::GetIntegerv)),
		  bind_texture_(procedure<BindTextureFn>(functions, GlProcedure::BindTexture)),
		  bind_framebuffer_(procedure<BindFramebufferFn>(functions, GlProcedure::BindFramebuffer)),
		  pixel_store_(procedure<PixelStoreiFn>(functions, GlProcedure::PixelStorei))
	{
		get_integer_(kTextureBinding2D, &texture_);
		get_integer_(kFramebufferBinding, &framebuffer_);
		get_integer_(kUnpackAlignment, &unpack_alignment_);
	}

	~GlBindingSnapshot()
	{
		bind_framebuffer_(kFramebuffer, static_cast<GlUint>(framebuffer_));
		bind_texture_(kTexture2D, static_cast<GlUint>(texture_));
		pixel_store_(kUnpackAlignment, unpack_alignment_);
	}

private:
	GetIntegervFn get_integer_;
	BindTextureFn bind_texture_;
	BindFramebufferFn bind_framebuffer_;
	PixelStoreiFn pixel_store_;
	GlInt texture_ = 0;
	GlInt framebuffer_ = 0;
	GlInt unpack_alignment_ = 4;
};

} // namespace

OpenGlTextureDevice::OpenGlTextureDevice(const GlFunctionTable& functions) noexcept
	: functions_(functions),
	  owner_thread_(std::this_thread::get_id()),
#if defined(_WIN32)
	  native_context_(reinterpret_cast<void*>(::wglGetCurrentContext()))
#else
	  native_context_(nullptr)
#endif
{
}

RvResult OpenGlTextureDevice::check_owner() const noexcept
{
	if(std::this_thread::get_id() != owner_thread_) {
		return RV_RESULT_WRONG_THREAD;
	}
#if defined(_WIN32)
	const HGLRC current = ::wglGetCurrentContext();
	if(native_context_ == nullptr || current == nullptr || ::wglGetCurrentDC() == nullptr ||
		reinterpret_cast<void*>(current) != native_context_) {
		return RV_RESULT_NO_CONTEXT;
	}
	return RV_RESULT_OK;
#else
	return RV_RESULT_NO_CONTEXT;
#endif
}

RvResult OpenGlTextureDevice::max_texture_dimension(uint32_t* out_dimension) noexcept
{
	const RvResult owner = check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(out_dimension == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*out_dimension = 0;
	const auto get_integer = procedure<GetIntegervFn>(functions_, GlProcedure::GetIntegerv);
	const auto get_error = procedure<GetErrorFn>(functions_, GlProcedure::GetError);
	if(get_integer == nullptr || get_error == nullptr) {
		return RV_RESULT_GL_FAILURE;
	}
	GlInt dimension = 0;
	get_integer(kMaxTextureSize, &dimension);
	if(get_error() != kNoError || dimension <= 0) {
		return RV_RESULT_GL_FAILURE;
	}
	*out_dimension = static_cast<uint32_t>(dimension);
	return RV_RESULT_OK;
}

RvResult OpenGlTextureDevice::create_rgba8(
	uint32_t width, uint32_t height, TextureAllocation* out) noexcept
{
	const RvResult owner = check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(out == nullptr || width == 0 || height == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*out = TextureAllocation{};
	if(width > static_cast<uint32_t>(INT_MAX) || height > static_cast<uint32_t>(INT_MAX)) {
		return RV_RESULT_RESOURCE_LIMIT;
	}
	uint32_t max_dimension = 0;
	const RvResult dimension_result = max_texture_dimension(&max_dimension);
	if(dimension_result != RV_RESULT_OK) return dimension_result;
	if(width > max_dimension || height > max_dimension) return RV_RESULT_RESOURCE_LIMIT;
	const auto get_error = procedure<GetErrorFn>(functions_, GlProcedure::GetError);
	const auto get_integer = procedure<GetIntegervFn>(functions_, GlProcedure::GetIntegerv);
	const auto gen_textures = procedure<GenTexturesFn>(functions_, GlProcedure::GenTextures);
	const auto bind_texture = procedure<BindTextureFn>(functions_, GlProcedure::BindTexture);
	const auto delete_textures = procedure<DeleteTexturesFn>(functions_, GlProcedure::DeleteTextures);
	const auto tex_image = procedure<TexImage2DFn>(functions_, GlProcedure::TexImage2D);
	const auto tex_parameter = procedure<TexParameteriFn>(functions_, GlProcedure::TexParameteri);
	const auto gen_framebuffers = procedure<GenFramebuffersFn>(functions_, GlProcedure::GenFramebuffers);
	const auto bind_framebuffer = procedure<BindFramebufferFn>(functions_, GlProcedure::BindFramebuffer);
	const auto attach_texture = procedure<FramebufferTexture2DFn>(functions_, GlProcedure::FramebufferTexture2D);
	const auto check_framebuffer = procedure<CheckFramebufferStatusFn>(functions_, GlProcedure::CheckFramebufferStatus);
	const auto delete_framebuffers = procedure<DeleteFramebuffersFn>(functions_, GlProcedure::DeleteFramebuffers);
	const auto pixel_store = procedure<PixelStoreiFn>(functions_, GlProcedure::PixelStorei);
	if(get_error == nullptr || get_integer == nullptr || pixel_store == nullptr ||
		gen_textures == nullptr || bind_texture == nullptr ||
		delete_textures == nullptr || tex_image == nullptr || tex_parameter == nullptr ||
		gen_framebuffers == nullptr || bind_framebuffer == nullptr || attach_texture == nullptr ||
		check_framebuffer == nullptr || delete_framebuffers == nullptr) {
		return RV_RESULT_GL_FAILURE;
	}

	GlBindingSnapshot saved(functions_);
	GlUint texture = 0;
	GlUint framebuffer = 0;
	gen_textures(1, &texture);
	if(texture == 0) {
		return RV_RESULT_GL_FAILURE;
	}
	bind_texture(kTexture2D, texture);
	tex_parameter(kTexture2D, kTextureMinFilter, kNearest);
	tex_parameter(kTexture2D, kTextureMagFilter, kNearest);
	tex_parameter(kTexture2D, kTextureWrapS, kClampToEdge);
	tex_parameter(kTexture2D, kTextureWrapT, kClampToEdge);
	tex_image(kTexture2D, 0, static_cast<GlInt>(kRgba8), static_cast<GlSizei>(width),
		static_cast<GlSizei>(height), 0, kRgba, kUnsignedByte, nullptr);
	gen_framebuffers(1, &framebuffer);
	if(framebuffer != 0) {
		bind_framebuffer(kFramebuffer, framebuffer);
		attach_texture(kFramebuffer, kColorAttachment0, kTexture2D, texture, 0);
	}
	const GlEnum status = framebuffer != 0 ? check_framebuffer(kFramebuffer) : 0;
	const GlEnum error = get_error();
	if(framebuffer == 0 || status != kFramebufferComplete || error != kNoError) {
		if(framebuffer != 0) {
			delete_framebuffers(1, &framebuffer);
		}
		delete_textures(1, &texture);
		return RV_RESULT_GL_FAILURE;
	}

	out->texture = texture;
	out->framebuffer = framebuffer;
	out->width = width;
	out->height = height;
	out->storage_bytes = static_cast<uint64_t>(width) * height * 4;
	return RV_RESULT_OK;
}

RvResult OpenGlTextureDevice::upload_rgba8_rows(
	const TextureAllocation& allocation, uint32_t first_gl_row,
	uint32_t row_count, const uint8_t* bottom_up_rgba,
	uint32_t byte_count) noexcept
{
	const RvResult owner = check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(allocation.texture == 0 || allocation.width == 0 || allocation.height == 0 ||
		row_count == 0 || first_gl_row > allocation.height ||
		row_count > allocation.height - first_gl_row || bottom_up_rgba == nullptr) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint64_t expected = static_cast<uint64_t>(allocation.width) * row_count * 4;
	if(expected > UINT32_MAX || byte_count != expected) {
		return expected > UINT32_MAX ? RV_RESULT_RESOURCE_LIMIT : RV_RESULT_INVALID_ARGUMENT;
	}
	const auto get_error = procedure<GetErrorFn>(functions_, GlProcedure::GetError);
	const auto bind_texture = procedure<BindTextureFn>(functions_, GlProcedure::BindTexture);
	const auto tex_sub_image = procedure<TexSubImage2DFn>(functions_, GlProcedure::TexSubImage2D);
	const auto pixel_store = procedure<PixelStoreiFn>(functions_, GlProcedure::PixelStorei);
	if(get_error == nullptr || bind_texture == nullptr || tex_sub_image == nullptr || pixel_store == nullptr) {
		return RV_RESULT_GL_FAILURE;
	}
	GlBindingSnapshot saved(functions_);
	bind_texture(kTexture2D, allocation.texture);
	pixel_store(kUnpackAlignment, 1);
	tex_sub_image(kTexture2D, 0, 0, static_cast<GlInt>(first_gl_row),
		static_cast<GlSizei>(allocation.width), static_cast<GlSizei>(row_count),
		kRgba, kUnsignedByte, bottom_up_rgba);
	return get_error() == kNoError ? RV_RESULT_OK : RV_RESULT_GL_FAILURE;
}

void OpenGlTextureDevice::destroy(const TextureAllocation& allocation) noexcept
{
	if(check_owner() != RV_RESULT_OK) return;
	const auto get_integer = procedure<GetIntegervFn>(functions_, GlProcedure::GetIntegerv);
	const auto bind_texture = procedure<BindTextureFn>(functions_, GlProcedure::BindTexture);
	const auto bind_framebuffer = procedure<BindFramebufferFn>(functions_, GlProcedure::BindFramebuffer);
	const auto delete_framebuffers = procedure<DeleteFramebuffersFn>(functions_, GlProcedure::DeleteFramebuffers);
	const auto delete_textures = procedure<DeleteTexturesFn>(functions_, GlProcedure::DeleteTextures);
	if(get_integer == nullptr || bind_texture == nullptr || bind_framebuffer == nullptr) return;
	GlInt current_texture = 0;
	GlInt current_framebuffer = 0;
	get_integer(kTextureBinding2D, &current_texture);
	get_integer(kFramebufferBinding, &current_framebuffer);
	// Deleting a bound object must not leave a dead name as the caller's binding.
	if(static_cast<GlUint>(current_framebuffer) == allocation.framebuffer && allocation.framebuffer != 0) {
		bind_framebuffer(kFramebuffer, 0);
	}
	if(static_cast<GlUint>(current_texture) == allocation.texture && allocation.texture != 0) {
		bind_texture(kTexture2D, 0);
	}
	if(allocation.framebuffer != 0 && delete_framebuffers != nullptr) {
		delete_framebuffers(1, &allocation.framebuffer);
	}
	if(allocation.texture != 0 && delete_textures != nullptr) {
		delete_textures(1, &allocation.texture);
	}
}

} // namespace revival::graphics
