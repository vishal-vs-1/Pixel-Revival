#include "frame_presenter.hpp"

#include <array>
#include <cstdint>

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
using ActiveTextureFn = void (APIENTRY*)(GlEnum);
using BindTextureFn = void (APIENTRY*)(GlEnum, GlUint);
using TexParameteriFn = void (APIENTRY*)(GlEnum, GlEnum, GlInt);
using BlendFunctionFn = void (APIENTRY*)(GlEnum, GlEnum, GlEnum, GlEnum);
using BlendEquationFn = void (APIENTRY*)(GlEnum, GlEnum);
using UseProgramFn = void (APIENTRY*)(GlUint);

RvResult calculate_overlay_rect(const RvViewport& viewport,
	const TextureSurfaceSnapshot& overlay, FrameViewportRect* out) noexcept
{
	if(out == nullptr || overlay.logical_width == 0 || overlay.logical_height == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	*out = FrameViewportRect{};
	uint64_t width = static_cast<uint64_t>(viewport.framebuffer_height) * overlay.logical_width /
		overlay.logical_height;
	uint64_t height = viewport.framebuffer_height;
	if(width == 0) width = 1;
	if(width > viewport.framebuffer_width) {
		width = viewport.framebuffer_width;
		height = static_cast<uint64_t>(width) * overlay.logical_height / overlay.logical_width;
		if(height == 0) height = 1;
	}
	if(width > UINT32_MAX || height > UINT32_MAX) return RV_RESULT_RESOURCE_LIMIT;
	out->width = static_cast<uint32_t>(width);
	out->height = static_cast<uint32_t>(height);
	out->x = (viewport.framebuffer_width - out->width) / 2;
	out->y = (viewport.framebuffer_height - out->height) / 2;
	return RV_RESULT_OK;
}

constexpr GlEnum kFramebuffer = 0x8D40;
constexpr GlEnum kFramebufferBinding = 0x8CA6;
constexpr GlEnum kTexture2D = 0x0DE1;
constexpr GlEnum kTexture0 = 0x84C0;
constexpr GlEnum kActiveTexture = 0x84E0;
constexpr GlEnum kTextureBinding2D = 0x8069;
constexpr GlEnum kTextureMinFilter = 0x2801;
constexpr GlEnum kTextureMagFilter = 0x2800;
constexpr GlEnum kNearest = 0x2600;
constexpr GlEnum kLinear = 0x2601;
constexpr GlEnum kProjectionMatrix = 0x0BA7;
constexpr GlEnum kModelviewMatrix = 0x0BA6;
constexpr GlEnum kTextureMatrix = 0x0BA8;
constexpr GlEnum kCurrentMatrixMode = 0x0BA0;
constexpr GlEnum kCurrentProgram = 0x8B8D;
constexpr GlEnum kViewport = 0x0BA2;
constexpr GlEnum kColorWriteMask = 0x0C23;
constexpr GlEnum kClearColor = 0x0C22;
constexpr GlEnum kCurrentColor = 0x0B00;
constexpr GlEnum kPolygonMode = 0x0B40;
constexpr GlEnum kTextureEnv = 0x2300;
constexpr GlEnum kTextureEnvMode = 0x2200;
constexpr GlEnum kBlendSrcRgb = 0x80C9;
constexpr GlEnum kBlendDstRgb = 0x80C8;
constexpr GlEnum kBlendSrcAlpha = 0x80CB;
constexpr GlEnum kBlendDstAlpha = 0x80CA;
constexpr GlEnum kBlendEquationRgb = 0x8009;
constexpr GlEnum kBlendEquationAlpha = 0x883D;
constexpr GlEnum kBlend = 0x0BE2;
constexpr GlEnum kScissorTest = 0x0C11;
constexpr GlEnum kDepthTest = 0x0B71;
constexpr GlEnum kCullFace = 0x0B44;
constexpr GlEnum kStencilTest = 0x0B90;
constexpr GlEnum kAlphaTest = 0x0BC0;
constexpr GlEnum kColorLogicOp = 0x0BF2;
constexpr GlEnum kTextureGenS = 0x0C60;
constexpr GlEnum kTextureGenT = 0x0C61;
constexpr GlEnum kTextureGenR = 0x0C62;
constexpr GlEnum kTextureGenQ = 0x0C63;
constexpr GlEnum kLighting = 0x0B50;
constexpr GlEnum kFog = 0x0B60;
constexpr GlEnum kColorMaterial = 0x0B57;
constexpr GlEnum kDither = 0x0BD0;
constexpr GlEnum kFront = 0x0404;
constexpr GlEnum kBack = 0x0405;
constexpr GlEnum kColorBufferBit = 0x00004000;
constexpr GlEnum kQuads = 0x0007;
constexpr GlEnum kOne = 1;
constexpr GlEnum kOneMinusSourceAlpha = 0x0303;
constexpr GlEnum kFuncAdd = 0x8006;
constexpr GlEnum kModulate = 0x2100;

template<typename Function>
Function get_procedure(const GlFunctionTable& functions, GlProcedure procedure) noexcept
{
	return reinterpret_cast<Function>(functions.get(procedure));
}

class PresentationState final {
public:
	PresentationState(BindFramebufferFn bind_framebuffer, ActiveTextureFn active_texture,
		BindTextureFn bind_texture, TexParameteriFn tex_parameter,
		BlendFunctionFn blend_function, BlendEquationFn blend_equation,
		UseProgramFn use_program, const std::array<GlUint, 3>& textures) noexcept
		: bind_framebuffer_(bind_framebuffer), active_texture_(active_texture),
		  bind_texture_(bind_texture), tex_parameter_(tex_parameter),
		  blend_function_(blend_function), blend_equation_(blend_equation),
		  use_program_(use_program)
	{
		::glGetIntegerv(kFramebufferBinding, &framebuffer_);
		::glGetIntegerv(kCurrentMatrixMode, &matrix_mode_);
		::glGetIntegerv(kViewport, viewport_.data());
		::glGetIntegerv(kPolygonMode, polygon_mode_.data());
		::glGetBooleanv(kColorWriteMask, color_mask_.data());
		::glGetFloatv(kClearColor, clear_color_.data());
		::glGetFloatv(kCurrentColor, current_color_.data());
		::glGetIntegerv(kBlendSrcRgb, &blend_src_rgb_);
		::glGetIntegerv(kBlendDstRgb, &blend_dst_rgb_);
		::glGetIntegerv(kBlendSrcAlpha, &blend_src_alpha_);
		::glGetIntegerv(kBlendDstAlpha, &blend_dst_alpha_);
		::glGetIntegerv(kBlendEquationRgb, &blend_equation_rgb_);
		::glGetIntegerv(kBlendEquationAlpha, &blend_equation_alpha_);
		if(use_program_ != nullptr) ::glGetIntegerv(kCurrentProgram, &program_);
		if(active_texture_ != nullptr) ::glGetIntegerv(kActiveTexture, &active_texture_unit_);
		if(active_texture_ != nullptr) active_texture_(kTexture0);
		::glGetTexEnviv(kTextureEnv, kTextureEnvMode, &texture_env_mode_);
		::glGetIntegerv(kTextureBinding2D, &texture_binding_);
		::glGetFloatv(kProjectionMatrix, projection_.data());
		::glGetFloatv(kModelviewMatrix, modelview_.data());
		::glGetFloatv(kTextureMatrix, texture_.data());
		const std::array<GlEnum, 15> capabilities{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kLighting, kFog, kColorMaterial,
			kDither, kTextureGenS, kTextureGenT, kTextureGenR, kTextureGenQ}};
		for(size_t index = 0; index < capabilities.size(); ++index) {
			capabilities_[index] = ::glIsEnabled(capabilities[index]) == GL_TRUE;
		}
		texture_enabled_ = ::glIsEnabled(kTexture2D) == GL_TRUE;
		capture_textures(textures);
		valid_ = true;
	}

	~PresentationState()
	{
		if(!valid_) return;
		if(active_texture_ != nullptr) active_texture_(kTexture0);
		for(size_t index = 0; index < texture_count_; ++index) {
			bind_texture_(kTexture2D, texture_parameters_[index].texture);
			tex_parameter_(kTexture2D, kTextureMinFilter, texture_parameters_[index].min_filter);
			tex_parameter_(kTexture2D, kTextureMagFilter, texture_parameters_[index].mag_filter);
		}
		bind_texture_(kTexture2D, static_cast<GlUint>(texture_binding_));
		::glTexEnvi(kTextureEnv, kTextureEnvMode, texture_env_mode_);
		if(use_program_ != nullptr) use_program_(static_cast<GlUint>(program_));
		::glMatrixMode(0x1701);
		::glLoadMatrixf(projection_.data());
		::glMatrixMode(0x1700);
		::glLoadMatrixf(modelview_.data());
		::glMatrixMode(0x1702);
		::glLoadMatrixf(texture_.data());
		blend_function_(static_cast<GlEnum>(blend_src_rgb_), static_cast<GlEnum>(blend_dst_rgb_),
			static_cast<GlEnum>(blend_src_alpha_), static_cast<GlEnum>(blend_dst_alpha_));
		blend_equation_(static_cast<GlEnum>(blend_equation_rgb_),
			static_cast<GlEnum>(blend_equation_alpha_));
		::glColorMask(color_mask_[0], color_mask_[1], color_mask_[2], color_mask_[3]);
		::glClearColor(clear_color_[0], clear_color_[1], clear_color_[2], clear_color_[3]);
		::glColor4f(current_color_[0], current_color_[1], current_color_[2], current_color_[3]);
		::glPolygonMode(kFront, static_cast<GlEnum>(polygon_mode_[0]));
		::glPolygonMode(kBack, static_cast<GlEnum>(polygon_mode_[1]));
		::glViewport(viewport_[0], viewport_[1], viewport_[2], viewport_[3]);
		const std::array<GlEnum, 15> capabilities{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kLighting, kFog, kColorMaterial,
			kDither, kTextureGenS, kTextureGenT, kTextureGenR, kTextureGenQ}};
		for(size_t index = 0; index < capabilities.size(); ++index) {
			if(capabilities_[index]) ::glEnable(capabilities[index]);
			else ::glDisable(capabilities[index]);
		}
		if(texture_enabled_) ::glEnable(kTexture2D);
		else ::glDisable(kTexture2D);
		if(active_texture_ != nullptr) active_texture_(static_cast<GlEnum>(active_texture_unit_));
		bind_framebuffer_(kFramebuffer, static_cast<GlUint>(framebuffer_));
		::glMatrixMode(static_cast<GlEnum>(matrix_mode_));
	}

private:
	struct TextureParameters {
		GlUint texture = 0;
		GlInt min_filter = static_cast<GlInt>(kNearest);
		GlInt mag_filter = static_cast<GlInt>(kNearest);
	};

	void capture_textures(const std::array<GlUint, 3>& textures) noexcept
	{
		for(GlUint texture : textures) {
			if(texture == 0) continue;
			bool duplicate = false;
			for(size_t index = 0; index < texture_count_; ++index) {
				duplicate = duplicate || texture_parameters_[index].texture == texture;
			}
			if(duplicate) continue;
			TextureParameters& parameters = texture_parameters_[texture_count_++];
			parameters.texture = texture;
			bind_texture_(kTexture2D, texture);
			::glGetTexParameteriv(kTexture2D, kTextureMinFilter, &parameters.min_filter);
			::glGetTexParameteriv(kTexture2D, kTextureMagFilter, &parameters.mag_filter);
		}
		bind_texture_(kTexture2D, static_cast<GlUint>(texture_binding_));
		if(active_texture_ != nullptr) active_texture_(static_cast<GlEnum>(active_texture_unit_));
	}

	BindFramebufferFn bind_framebuffer_;
	ActiveTextureFn active_texture_;
	BindTextureFn bind_texture_;
	TexParameteriFn tex_parameter_;
	BlendFunctionFn blend_function_;
	BlendEquationFn blend_equation_;
	UseProgramFn use_program_;
	GlInt framebuffer_ = 0;
	GlInt matrix_mode_ = 0x1700;
	GlInt active_texture_unit_ = static_cast<GlInt>(kTexture0);
	GlInt texture_binding_ = 0;
	GlInt program_ = 0;
	GlInt texture_env_mode_ = static_cast<GlInt>(kModulate);
	GlInt blend_src_rgb_ = static_cast<GlInt>(kOne);
	GlInt blend_dst_rgb_ = 0;
	GlInt blend_src_alpha_ = static_cast<GlInt>(kOne);
	GlInt blend_dst_alpha_ = 0;
	GlInt blend_equation_rgb_ = static_cast<GlInt>(kFuncAdd);
	GlInt blend_equation_alpha_ = static_cast<GlInt>(kFuncAdd);
	std::array<GlInt, 4> viewport_{};
	std::array<GlInt, 2> polygon_mode_{};
	std::array<GLboolean, 4> color_mask_{};
	std::array<GLfloat, 4> clear_color_{};
	std::array<GLfloat, 4> current_color_{};
	std::array<bool, 15> capabilities_{};
	std::array<TextureParameters, 3> texture_parameters_{};
	size_t texture_count_ = 0;
	bool texture_enabled_ = false;
	bool valid_ = false;
	std::array<GLfloat, 16> projection_{};
	std::array<GLfloat, 16> modelview_{};
	std::array<GLfloat, 16> texture_{};
};

void draw_quad(const FrameViewportRect& viewport, float u0, float v0, float u1, float v1)
{
	const GLdouble left = viewport.x;
	const GLdouble top = viewport.y;
	const GLdouble right = static_cast<GLdouble>(viewport.x) + viewport.width;
	const GLdouble bottom = static_cast<GLdouble>(viewport.y) + viewport.height;
	::glBegin(kQuads);
	::glTexCoord2f(u0, 1.f - v0); ::glVertex2d(left, top);
	::glTexCoord2f(u1, 1.f - v0); ::glVertex2d(right, top);
	::glTexCoord2f(u1, 1.f - v1); ::glVertex2d(right, bottom);
	::glTexCoord2f(u0, 1.f - v1); ::glVertex2d(left, bottom);
	::glEnd();
}

} // namespace

OpenGlFrameOutputRenderer::OpenGlFrameOutputRenderer(const GlFunctionTable& functions) noexcept
	: functions_(functions)
{
}

RvResult OpenGlFrameOutputRenderer::render(const CommittedFrameSnapshot& frame,
	const std::array<RvSurfaceHandle, 2>& overlay_handles, const TextureStore& store,
	const RvViewport& viewport) noexcept
{
	const RvResult valid = validate_viewport(viewport);
	if(valid != RV_RESULT_OK) return valid;
	TextureSurfaceSnapshot base{};
	if(frame.valid) {
		const RvResult inspected = store.inspect(frame.surface, &base);
		if(inspected != RV_RESULT_OK) return inspected;
		if(!base.initialized || frame.logical_width != base.logical_width ||
			frame.logical_height != base.logical_height || frame.storage_scale != base.storage_scale ||
			frame.content.x < 0 || frame.content.y < 0 || frame.content.width <= 0 ||
			frame.content.height <= 0 ||
			static_cast<int64_t>(frame.content.x) + frame.content.width > base.logical_width ||
			static_cast<int64_t>(frame.content.y) + frame.content.height > base.logical_height) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
	}
	std::array<TextureSurfaceSnapshot, 2> overlays{};
	std::array<GlUint, 3> texture_names{};
	if(frame.valid) texture_names[0] = base.active.texture;
	for(size_t index = 0; index < overlay_handles.size(); ++index) {
		if(overlay_handles[index] == 0) continue;
		const RvResult inspected = store.inspect(overlay_handles[index], &overlays[index]);
		if(inspected != RV_RESULT_OK) return inspected;
		if(!overlays[index].initialized) return RV_RESULT_INVALID_ARGUMENT;
		texture_names[index + 1] = overlays[index].active.texture;
	}
	FrameViewportRect target{};
	const RvResult viewport_result = calculate_viewport_rect(viewport, &target);
	if(viewport_result != RV_RESULT_OK) return viewport_result;
	const auto bind_framebuffer = get_procedure<BindFramebufferFn>(functions_, GlProcedure::BindFramebuffer);
	const auto active_texture = get_procedure<ActiveTextureFn>(functions_, GlProcedure::ActiveTexture);
	const auto bind_texture = get_procedure<BindTextureFn>(functions_, GlProcedure::BindTexture);
	const auto tex_parameter = get_procedure<TexParameteriFn>(functions_, GlProcedure::TexParameteri);
	const auto blend_function = get_procedure<BlendFunctionFn>(functions_, GlProcedure::BlendFuncSeparate);
	const auto blend_equation = get_procedure<BlendEquationFn>(functions_, GlProcedure::BlendEquationSeparate);
	const auto use_program = get_procedure<UseProgramFn>(functions_, GlProcedure::UseProgram);
	if(bind_framebuffer == nullptr || bind_texture == nullptr || tex_parameter == nullptr ||
		blend_function == nullptr || blend_equation == nullptr) return RV_RESULT_UNSUPPORTED_OPERATION;
	PresentationState saved(bind_framebuffer, active_texture, bind_texture, tex_parameter,
		blend_function, blend_equation, use_program, texture_names);
	if(active_texture != nullptr) active_texture(kTexture0);
	bind_framebuffer(kFramebuffer, 0);
	const std::array<GlEnum, 15> capabilities{{kScissorTest, kBlend, kDepthTest, kCullFace,
		kStencilTest, kAlphaTest, kColorLogicOp, kLighting, kFog, kColorMaterial,
		kDither, kTextureGenS, kTextureGenT, kTextureGenR, kTextureGenQ}};
	for(GlEnum capability : capabilities) ::glDisable(capability);
	::glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
	::glPolygonMode(kFront, 0x1B02);
	::glPolygonMode(kBack, 0x1B02);
	::glViewport(0, 0, static_cast<GLsizei>(viewport.framebuffer_width),
		static_cast<GLsizei>(viewport.framebuffer_height));
	::glClearColor(0.f, 0.f, 0.f, 1.f);
	::glClear(kColorBufferBit);
	::glMatrixMode(0x1701);
	::glLoadIdentity();
	::glOrtho(0.0, viewport.framebuffer_width, viewport.framebuffer_height, 0.0, -1.0, 1.0);
	::glMatrixMode(0x1700);
	::glLoadIdentity();
	::glMatrixMode(0x1702);
	::glLoadIdentity();
	if(use_program != nullptr) use_program(0);
	::glColor4f(1.f, 1.f, 1.f, 1.f);
	::glTexEnvi(kTextureEnv, kTextureEnvMode, kModulate);
	const GlInt filter = viewport.filter == RV_FILTER_LINEAR ? kLinear : kNearest;
	const bool has_overlay = overlay_handles[0] != 0 || overlay_handles[1] != 0;
	if(frame.valid) {
		::glEnable(kTexture2D);
		::glEnable(kBlend);
		blend_equation(kFuncAdd, kFuncAdd);
		blend_function(kOne, kOneMinusSourceAlpha, kOne, kOneMinusSourceAlpha);
		bind_texture(kTexture2D, base.active.texture);
		tex_parameter(kTexture2D, kTextureMinFilter, filter);
		tex_parameter(kTexture2D, kTextureMagFilter, filter);
		const float inverse_width = 1.f / static_cast<float>(base.logical_width);
		const float inverse_height = 1.f / static_cast<float>(base.logical_height);
		draw_quad(target,
			static_cast<float>(frame.content.x) * inverse_width,
			static_cast<float>(frame.content.y) * inverse_height,
			static_cast<float>(frame.content.x + frame.content.width) * inverse_width,
			static_cast<float>(frame.content.y + frame.content.height) * inverse_height);
	}
	if(has_overlay) {
		::glEnable(kTexture2D);
		::glEnable(kBlend);
		blend_equation(kFuncAdd, kFuncAdd);
		blend_function(kOne, kOneMinusSourceAlpha, kOne, kOneMinusSourceAlpha);
		for(size_t index = 0; index < overlay_handles.size(); ++index) {
			if(overlay_handles[index] == 0) continue;
			// HUD canvases can use either game or desktop dimensions, so fit each canvas independently.
			FrameViewportRect overlay_target{};
			const RvResult overlay_target_result = calculate_overlay_rect(viewport,
				overlays[index], &overlay_target);
			if(overlay_target_result != RV_RESULT_OK) return overlay_target_result;
			bind_texture(kTexture2D, overlays[index].active.texture);
			tex_parameter(kTexture2D, kTextureMinFilter, filter);
			tex_parameter(kTexture2D, kTextureMagFilter, filter);
			draw_quad(overlay_target, 0.f, 0.f, 1.f, 1.f);
		}
	}
	return RV_RESULT_OK;
}

} // namespace revival::graphics
