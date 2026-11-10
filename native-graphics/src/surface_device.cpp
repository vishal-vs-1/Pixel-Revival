#include "surface_device.hpp"

#include <algorithm>
#include <array>
#include <cstdint>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>
#endif

namespace revival::graphics {
namespace {

#if defined(_WIN32)
#define RV_SURFACE_GL_CALL APIENTRY
#else
#define RV_SURFACE_GL_CALL
#endif

using GlEnum = uint32_t;
using GlInt = int32_t;
using GlUint = uint32_t;
using GlFloat = float;
using GlBoolean = uint8_t;
using BlendFuncSeparateFn = void (RV_SURFACE_GL_CALL*)(GlEnum, GlEnum, GlEnum, GlEnum);
using BlendEquationSeparateFn = void (RV_SURFACE_GL_CALL*)(GlEnum, GlEnum);

constexpr GlEnum kTexture2D = 0x0DE1;
constexpr GlEnum kTextureBinding2D = 0x8069;
constexpr GlEnum kFramebuffer = 0x8D40;
constexpr GlEnum kFramebufferBinding = 0x8CA6;
constexpr GlEnum kViewport = 0x0BA2;
constexpr GlEnum kMatrixMode = 0x0BA0;
constexpr GlEnum kModelview = 0x1700;
constexpr GlEnum kProjection = 0x1701;
constexpr GlEnum kTexture = 0x1702;
constexpr GlEnum kModelviewStackDepth = 0x0BA3;
constexpr GlEnum kProjectionStackDepth = 0x0BA4;
constexpr GlEnum kTextureStackDepth = 0x0BA5;
constexpr GlEnum kMaxModelviewStackDepth = 0x0D36;
constexpr GlEnum kMaxProjectionStackDepth = 0x0D38;
constexpr GlEnum kMaxTextureStackDepth = 0x0D39;
constexpr GlEnum kScissorBox = 0x0C10;
constexpr GlEnum kScissorTest = 0x0C11;
constexpr GlEnum kBlend = 0x0BE2;
constexpr GlEnum kDepthTest = 0x0B71;
constexpr GlEnum kCullFace = 0x0B44;
constexpr GlEnum kStencilTest = 0x0B90;
constexpr GlEnum kAlphaTest = 0x0BC0;
constexpr GlEnum kColorLogicOp = 0x0BF2;
constexpr GlEnum kDither = 0x0BD0;
constexpr GlEnum kCurrentColor = 0x0B00;
constexpr GlEnum kColorClearValue = 0x0C22;
constexpr GlEnum kColorWriteMask = 0x0C23;
constexpr GlEnum kPolygonMode = 0x0B40;
constexpr GlEnum kFront = 0x0404;
constexpr GlEnum kBack = 0x0405;
constexpr GlEnum kFill = 0x1B02;
constexpr GlEnum kTextureEnv = 0x2300;
constexpr GlEnum kTextureEnvMode = 0x2200;
constexpr GlInt kModulate = 0x2100;
constexpr GlEnum kBlendSrcRgb = 0x80C9;
constexpr GlEnum kBlendDstRgb = 0x80C8;
constexpr GlEnum kBlendSrcAlpha = 0x80CB;
constexpr GlEnum kBlendDstAlpha = 0x80CA;
constexpr GlEnum kBlendEquationRgb = 0x8009;
constexpr GlEnum kBlendEquationAlpha = 0x883D;
constexpr GlEnum kOne = 1;
constexpr GlEnum kOneMinusSrcAlpha = 0x0303;
constexpr GlEnum kFuncAdd = 0x8006;
constexpr GlEnum kColorBufferBit = 0x00004000;
constexpr GlEnum kQuads = 0x0007;
constexpr GlEnum kNoError = 0;

template<typename Function>
Function procedure(const GlFunctionTable& functions, GlProcedure name) noexcept
{
	return reinterpret_cast<Function>(functions.get(name));
}

void set_enabled(GlEnum capability, bool enabled) noexcept
{
	if(enabled) ::glEnable(capability);
	else ::glDisable(capability);
}

struct GlStateSnapshot {
	GlInt framebuffer = 0;
	GlInt texture = 0;
	std::array<GlInt, 4> viewport{};
	std::array<GlInt, 4> scissor{};
	std::array<GlInt, 2> polygon_mode{};
	std::array<GlInt, 4> blend_functions{};
	std::array<GlInt, 2> blend_equations{};
	std::array<GlFloat, 4> color{};
	std::array<GlFloat, 4> clear_color{};
	std::array<GlBoolean, 4> color_mask{};
	std::array<bool, 9> capabilities{};
	GlInt texture_environment = kModulate;
	GlInt matrix_mode = kModelview;
	bool projection_pushed = false;
	bool modelview_pushed = false;
	bool texture_pushed = false;
	bool valid = false;

	void capture() noexcept
	{
		::glGetIntegerv(kFramebufferBinding, &framebuffer);
		::glGetIntegerv(kTextureBinding2D, &texture);
		::glGetIntegerv(kViewport, viewport.data());
		::glGetIntegerv(kScissorBox, scissor.data());
		::glGetIntegerv(kPolygonMode, polygon_mode.data());
		::glGetIntegerv(kMatrixMode, &matrix_mode);
		::glGetIntegerv(kBlendSrcRgb, &blend_functions[0]);
		::glGetIntegerv(kBlendDstRgb, &blend_functions[1]);
		::glGetIntegerv(kBlendSrcAlpha, &blend_functions[2]);
		::glGetIntegerv(kBlendDstAlpha, &blend_functions[3]);
		::glGetIntegerv(kBlendEquationRgb, &blend_equations[0]);
		::glGetIntegerv(kBlendEquationAlpha, &blend_equations[1]);
		::glGetFloatv(kCurrentColor, color.data());
		::glGetFloatv(kColorClearValue, clear_color.data());
		::glGetBooleanv(kColorWriteMask, color_mask.data());
		::glGetTexEnviv(kTextureEnv, kTextureEnvMode, &texture_environment);
		const std::array<GlEnum, 9> names{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kDither, kTexture2D}};
		for(size_t index = 0; index < names.size(); ++index) {
			capabilities[index] = ::glIsEnabled(names[index]) == GL_TRUE;
		}
		valid = true;
	}

	bool prepare_quad_matrices() noexcept
	{
		auto push_identity = [](GlEnum mode, GlEnum depth_name, GlEnum maximum_name,
			bool* pushed) {
			GlInt depth = 0;
			GlInt maximum = 0;
			::glGetIntegerv(depth_name, &depth);
			::glGetIntegerv(maximum_name, &maximum);
			if(depth <= 0 || maximum <= depth) return false;
			::glMatrixMode(mode);
			::glPushMatrix();
			GlInt pushed_depth = 0;
			::glGetIntegerv(depth_name, &pushed_depth);
			if(pushed_depth != depth + 1) return false;
			*pushed = true;
			::glLoadIdentity();
			return ::glGetError() == kNoError;
		};
		// The quad coordinates are normalized device coordinates; isolate them from caller matrices.
		return push_identity(kProjection, kProjectionStackDepth, kMaxProjectionStackDepth,
			&projection_pushed) &&
			push_identity(kModelview, kModelviewStackDepth, kMaxModelviewStackDepth,
				&modelview_pushed) &&
			push_identity(kTexture, kTextureStackDepth, kMaxTextureStackDepth,
				&texture_pushed);
	}

	void restore(const GlFunctionTable& functions) const noexcept
	{
		if(!valid) return;
		if(texture_pushed) {
			::glMatrixMode(kTexture);
			::glPopMatrix();
		}
		if(modelview_pushed) {
			::glMatrixMode(kModelview);
			::glPopMatrix();
		}
		if(projection_pushed) {
			::glMatrixMode(kProjection);
			::glPopMatrix();
		}
		::glMatrixMode(static_cast<GlEnum>(matrix_mode));
		const auto bind_framebuffer = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
			functions, GlProcedure::BindFramebuffer);
		const auto bind_texture = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
			functions, GlProcedure::BindTexture);
		const auto blend_func = procedure<BlendFuncSeparateFn>(functions, GlProcedure::BlendFuncSeparate);
		const auto blend_equation = procedure<BlendEquationSeparateFn>(functions, GlProcedure::BlendEquationSeparate);
		if(bind_framebuffer != nullptr) bind_framebuffer(kFramebuffer, static_cast<GlUint>(framebuffer));
		if(bind_texture != nullptr) bind_texture(kTexture2D, static_cast<GlUint>(texture));
		::glViewport(viewport[0], viewport[1], viewport[2], viewport[3]);
		::glScissor(scissor[0], scissor[1], scissor[2], scissor[3]);
		::glPolygonMode(kFront, static_cast<GlEnum>(polygon_mode[0]));
		::glPolygonMode(kBack, static_cast<GlEnum>(polygon_mode[1]));
		if(blend_func != nullptr) blend_func(
			static_cast<GlEnum>(blend_functions[0]), static_cast<GlEnum>(blend_functions[1]),
			static_cast<GlEnum>(blend_functions[2]), static_cast<GlEnum>(blend_functions[3]));
		if(blend_equation != nullptr) blend_equation(
			static_cast<GlEnum>(blend_equations[0]), static_cast<GlEnum>(blend_equations[1]));
		::glColor4fv(color.data());
		::glClearColor(clear_color[0], clear_color[1], clear_color[2], clear_color[3]);
		::glColorMask(color_mask[0], color_mask[1], color_mask[2], color_mask[3]);
		::glTexEnvi(kTextureEnv, kTextureEnvMode, texture_environment);
		const std::array<GlEnum, 9> names{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kDither, kTexture2D}};
		for(size_t index = 0; index < names.size(); ++index) set_enabled(names[index], capabilities[index]);
	}
};

class ScopedGlState {
public:
	ScopedGlState(const GlFunctionTable& functions, GlStateSnapshot* out) noexcept
		: functions_(functions), snapshot_(out)
	{
		if(snapshot_ != nullptr) snapshot_->capture();
	}
	~ScopedGlState()
	{
		if(snapshot_ != nullptr) snapshot_->restore(functions_);
	}
private:
	const GlFunctionTable& functions_;
	GlStateSnapshot* snapshot_;
};

bool valid_target(const TextureSurfaceSnapshot& surface) noexcept
{
	return surface.active.texture != 0 && surface.active.framebuffer != 0 &&
		surface.logical_width != 0 && surface.logical_height != 0 && surface.storage_scale != 0 &&
		static_cast<uint64_t>(surface.logical_width) * surface.storage_scale == surface.active.width &&
		static_cast<uint64_t>(surface.logical_height) * surface.storage_scale == surface.active.height;
}

struct Bounds {
	int64_t left = 0;
	int64_t top = 0;
	int64_t right = 0;
	int64_t bottom = 0;
};

Bounds intersect(Bounds first, Bounds second) noexcept
{
	return Bounds{std::max(first.left, second.left), std::max(first.top, second.top),
		std::min(first.right, second.right), std::min(first.bottom, second.bottom)};
}

Bounds rect_bounds(const SurfaceRect& rect) noexcept
{
	return Bounds{rect.x, rect.y, static_cast<int64_t>(rect.x) + rect.width,
		static_cast<int64_t>(rect.y) + rect.height};
}

Bounds visible_clip(const TextureSurfaceSnapshot& destination, const SurfaceRect& clip) noexcept
{
	return intersect(rect_bounds(clip), Bounds{0, 0,
		destination.logical_width, destination.logical_height});
}

RvResult bind_target(
	const GlFunctionTable& functions,
	const TextureSurfaceSnapshot& destination, const SurfaceRect& clip) noexcept
{
	const uint32_t scale = destination.storage_scale;
	const uint32_t width = destination.active.width;
	const uint32_t height = destination.active.height;
	auto bind_framebuffer = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
		functions, GlProcedure::BindFramebuffer);
	if(bind_framebuffer == nullptr) return RV_RESULT_GL_FAILURE;
	bind_framebuffer(kFramebuffer, destination.active.framebuffer);
	::glViewport(0, 0, static_cast<GlInt>(width), static_cast<GlInt>(height));
	const Bounds visible = visible_clip(destination, clip);
	const GlInt scissor_x = static_cast<GlInt>(visible.left * scale);
	const GlInt scissor_y = static_cast<GlInt>(
		(static_cast<int64_t>(destination.logical_height) - visible.bottom) * scale);
	const GlInt scissor_w = static_cast<GlInt>(std::max<int64_t>(0, visible.right - visible.left) * scale);
	const GlInt scissor_h = static_cast<GlInt>(std::max<int64_t>(0, visible.bottom - visible.top) * scale);
	::glEnable(kScissorTest);
	::glScissor(scissor_x, scissor_y, scissor_w, scissor_h);
	::glDisable(kDepthTest);
	::glDisable(kCullFace);
	::glDisable(kStencilTest);
	::glDisable(kAlphaTest);
	::glDisable(kColorLogicOp);
	::glDisable(kDither);
	::glPolygonMode(kFront, kFill);
	::glPolygonMode(kBack, kFill);
	::glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
	::glTexEnvi(kTextureEnv, kTextureEnvMode, kModulate);
	return RV_RESULT_OK;
}

RvResult set_blending(SurfaceBlend blend, const GlFunctionTable& functions) noexcept
{
	if(blend == SurfaceBlend::Replace) {
		::glDisable(kBlend);
		return RV_RESULT_OK;
	}
	const auto blend_func = procedure<BlendFuncSeparateFn>(functions, GlProcedure::BlendFuncSeparate);
	const auto blend_equation = procedure<BlendEquationSeparateFn>(functions, GlProcedure::BlendEquationSeparate);
	if(blend_func == nullptr || blend_equation == nullptr) return RV_RESULT_GL_FAILURE;
	::glEnable(kBlend);
	blend_func(kOne, kOneMinusSrcAlpha, kOne, kOneMinusSrcAlpha);
	blend_equation(kFuncAdd, kFuncAdd);
	return RV_RESULT_OK;
}

void vertex(float x, float y) noexcept
{
	::glVertex2f(x, y);
}

float ndc_x(int64_t physical_x, uint32_t physical_width) noexcept
{
	return static_cast<float>(2.0 * static_cast<double>(physical_x) / physical_width - 1.0);
}

float ndc_y(int64_t physical_y, uint32_t physical_height) noexcept
{
	return static_cast<float>(1.0 - 2.0 * static_cast<double>(physical_y) / physical_height);
}

RvResult gl_result() noexcept
{
	return ::glGetError() == kNoError ? RV_RESULT_OK : RV_RESULT_GL_FAILURE;
}

} // namespace

OpenGlSurfaceDevice::OpenGlSurfaceDevice(
	TextureDevice& owner_device, const GlFunctionTable& functions) noexcept
	: owner_device_(owner_device), functions_(functions)
{
}

RvResult OpenGlSurfaceDevice::clear_transparent(
	const TextureSurfaceSnapshot& destination) noexcept
{
	const RvResult owner = owner_device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(!valid_target(destination)) return RV_RESULT_INVALID_ARGUMENT;
	GlStateSnapshot snapshot{};
	ScopedGlState state(functions_, &snapshot);
	const RvResult target = bind_target(functions_, destination, SurfaceRect{0, 0,
		static_cast<int32_t>(destination.logical_width),
		static_cast<int32_t>(destination.logical_height)});
	if(target != RV_RESULT_OK) return target;
	::glDisable(kScissorTest);
	::glClearColor(0.f, 0.f, 0.f, 0.f);
	::glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
	::glClear(kColorBufferBit);
	return gl_result();
}

RvResult OpenGlSurfaceDevice::copy_surface(
	const TextureAllocation& source, const TextureAllocation& destination) noexcept
{
	const RvResult owner = owner_device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(source.framebuffer == 0 || destination.texture == 0 || source.width == 0 ||
		source.height == 0 || source.width != destination.width || source.height != destination.height) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	GlStateSnapshot snapshot{};
	ScopedGlState state(functions_, &snapshot);
	auto bind_framebuffer = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
		functions_, GlProcedure::BindFramebuffer);
	auto bind_texture = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
		functions_, GlProcedure::BindTexture);
	if(bind_framebuffer == nullptr || bind_texture == nullptr) return RV_RESULT_GL_FAILURE;
	bind_framebuffer(kFramebuffer, source.framebuffer);
	bind_texture(kTexture2D, destination.texture);
	::glCopyTexSubImage2D(kTexture2D, 0, 0, 0, 0, 0,
		static_cast<GlInt>(source.width), static_cast<GlInt>(source.height));
	return gl_result();
}

RvResult OpenGlSurfaceDevice::fill(
	const TextureSurfaceSnapshot& destination, const SurfaceRect& rectangle,
	const SurfaceRect& clip, uint32_t argb) noexcept
{
	const RvResult owner = owner_device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(!valid_target(destination) || rectangle.width <= 0 || rectangle.height <= 0 ||
		clip.width < 0 || clip.height < 0) return RV_RESULT_INVALID_ARGUMENT;
	const Bounds visible = visible_clip(destination, clip);
	if(visible.right <= visible.left || visible.bottom <= visible.top) return RV_RESULT_OK;
	GlStateSnapshot snapshot{};
	ScopedGlState state(functions_, &snapshot);
	const RvResult target = bind_target(functions_, destination, clip);
	if(target != RV_RESULT_OK) return target;
	const RvResult blending = set_blending(SurfaceBlend::SourceOver, functions_);
	if(blending != RV_RESULT_OK) return blending;
	if(!snapshot.prepare_quad_matrices()) return RV_RESULT_GL_FAILURE;
	::glDisable(kTexture2D);
	const uint32_t alpha = argb >> 24;
	const float a = static_cast<float>(alpha) / 255.f;
	const float red = static_cast<float>((argb >> 16) & 255u) / 255.f * a;
	const float green = static_cast<float>((argb >> 8) & 255u) / 255.f * a;
	const float blue = static_cast<float>(argb & 255u) / 255.f * a;
	::glColor4f(red, green, blue, a);
	const int64_t scale = destination.storage_scale;
	const int64_t left = static_cast<int64_t>(rectangle.x) * scale;
	const int64_t top = static_cast<int64_t>(rectangle.y) * scale;
	const int64_t right = (static_cast<int64_t>(rectangle.x) + rectangle.width) * scale;
	const int64_t bottom = (static_cast<int64_t>(rectangle.y) + rectangle.height) * scale;
	::glBegin(kQuads);
	vertex(ndc_x(left, destination.active.width), ndc_y(top, destination.active.height));
	vertex(ndc_x(right, destination.active.width), ndc_y(top, destination.active.height));
	vertex(ndc_x(right, destination.active.width), ndc_y(bottom, destination.active.height));
	vertex(ndc_x(left, destination.active.width), ndc_y(bottom, destination.active.height));
	::glEnd();
	return gl_result();
}

RvResult OpenGlSurfaceDevice::blit(
	const TextureAllocation& source, uint32_t source_logical_width,
	uint32_t source_logical_height, uint32_t source_scale,
	const TextureSurfaceSnapshot& destination, const SurfaceRect& source_rectangle,
	const SurfaceRect& destination_rectangle, const SurfaceRect& clip,
	uint32_t transform, SurfaceBlend blend) noexcept
{
	const RvResult owner = owner_device_.check_owner();
	if(owner != RV_RESULT_OK) return owner;
	if(source.texture == 0 || source_logical_width == 0 || source_logical_height == 0 ||
		source_scale == 0 || !valid_target(destination) || destination_rectangle.width <= 0 ||
		destination_rectangle.height <= 0 || clip.width < 0 || clip.height < 0 || transform > 7 ||
		(blend != SurfaceBlend::Replace && blend != SurfaceBlend::SourceOver) ||
		static_cast<uint64_t>(source_logical_width) * source_scale != source.width ||
		static_cast<uint64_t>(source_logical_height) * source_scale != source.height ||
		source_rectangle.x < 0 || source_rectangle.y < 0 || source_rectangle.width <= 0 ||
		source_rectangle.height <= 0 ||
		static_cast<int64_t>(source_rectangle.x) + source_rectangle.width > source_logical_width ||
		static_cast<int64_t>(source_rectangle.y) + source_rectangle.height > source_logical_height) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const Bounds visible = visible_clip(destination, clip);
	if(visible.right <= visible.left || visible.bottom <= visible.top) return RV_RESULT_OK;
	GlStateSnapshot snapshot{};
	ScopedGlState state(functions_, &snapshot);
	const auto bind_framebuffer = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
		functions_, GlProcedure::BindFramebuffer);
	const auto bind_texture = procedure<void (RV_SURFACE_GL_CALL*)(GlEnum, GlUint)>(
		functions_, GlProcedure::BindTexture);
	if(bind_framebuffer == nullptr || bind_texture == nullptr) return RV_RESULT_GL_FAILURE;
	const RvResult target = bind_target(functions_, destination, clip);
	if(target != RV_RESULT_OK) return target;
	bind_texture(kTexture2D, source.texture);
	::glEnable(kTexture2D);
	::glTexEnvi(kTextureEnv, kTextureEnvMode, kModulate);
	::glColor4f(1.f, 1.f, 1.f, 1.f);
	const RvResult blending = set_blending(blend, functions_);
	if(blending != RV_RESULT_OK) return blending;
	if(!snapshot.prepare_quad_matrices()) return RV_RESULT_GL_FAILURE;

	static constexpr uint8_t kCornerMap[8][4] = {
		{0, 1, 2, 3}, {3, 2, 1, 0}, {1, 0, 3, 2}, {2, 3, 0, 1},
		{0, 3, 2, 1}, {3, 0, 1, 2}, {1, 2, 3, 0}, {2, 1, 0, 3}
	};
	const float u0 = static_cast<float>(source_rectangle.x) / source_logical_width;
	const float u1 = static_cast<float>(static_cast<int64_t>(source_rectangle.x) + source_rectangle.width) /
		source_logical_width;
	const float v0 = 1.f - static_cast<float>(source_rectangle.y) / source_logical_height;
	const float v1 = 1.f - static_cast<float>(static_cast<int64_t>(source_rectangle.y) +
		source_rectangle.height) / source_logical_height;
	const int64_t scale = destination.storage_scale;
	const int64_t left = static_cast<int64_t>(destination_rectangle.x) * scale;
	const int64_t top = static_cast<int64_t>(destination_rectangle.y) * scale;
	const int64_t right = (static_cast<int64_t>(destination_rectangle.x) +
		destination_rectangle.width) * scale;
	const int64_t bottom = (static_cast<int64_t>(destination_rectangle.y) +
		destination_rectangle.height) * scale;
	const std::array<float, 4> vertex_x{{ndc_x(left, destination.active.width),
		ndc_x(right, destination.active.width), ndc_x(right, destination.active.width),
		ndc_x(left, destination.active.width)}};
	const std::array<float, 4> vertex_y{{ndc_y(top, destination.active.height),
		ndc_y(top, destination.active.height), ndc_y(bottom, destination.active.height),
		ndc_y(bottom, destination.active.height)}};
	::glBegin(kQuads);
	for(uint32_t index = 0; index < 4; ++index) {
		const uint8_t corner = kCornerMap[transform][index];
		const float u = corner == 0 || corner == 3 ? u0 : u1;
		const float v = corner < 2 ? v0 : v1;
		::glTexCoord2f(u, v);
		vertex(vertex_x[index], vertex_y[index]);
	}
	::glEnd();
	return gl_result();
}

} // namespace revival::graphics
