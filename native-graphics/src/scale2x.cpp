#include "scale2x.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <new>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>
#endif

#include "../shaders/scale2x_source.hpp"

namespace revival::graphics {
namespace {

#if defined(_WIN32)
#define RV_SCALE_GL_CALL APIENTRY
#else
#define RV_SCALE_GL_CALL
#endif

using GlEnum = uint32_t;
using GlInt = int32_t;
using GlUint = uint32_t;
using GlSizei = int32_t;
using GlFloat = float;
using GlBoolean = uint8_t;
using ActiveTextureFn = void (RV_SCALE_GL_CALL*)(GlEnum);
using BindTextureFn = void (RV_SCALE_GL_CALL*)(GlEnum, GlUint);
using BindFramebufferFn = void (RV_SCALE_GL_CALL*)(GlEnum, GlUint);
using UseProgramFn = void (RV_SCALE_GL_CALL*)(GlUint);
using CreateShaderFn = GlUint (RV_SCALE_GL_CALL*)(GlEnum);
using ShaderSourceFn = void (RV_SCALE_GL_CALL*)(GlUint, GlSizei, const char* const*, const GlInt*);
using CompileShaderFn = void (RV_SCALE_GL_CALL*)(GlUint);
using GetShaderivFn = void (RV_SCALE_GL_CALL*)(GlUint, GlEnum, GlInt*);
using DeleteShaderFn = void (RV_SCALE_GL_CALL*)(GlUint);
using CreateProgramFn = GlUint (RV_SCALE_GL_CALL*)();
using AttachShaderFn = void (RV_SCALE_GL_CALL*)(GlUint, GlUint);
using LinkProgramFn = void (RV_SCALE_GL_CALL*)(GlUint);
using GetProgramivFn = void (RV_SCALE_GL_CALL*)(GlUint, GlEnum, GlInt*);
using DeleteProgramFn = void (RV_SCALE_GL_CALL*)(GlUint);
using GetUniformLocationFn = GlInt (RV_SCALE_GL_CALL*)(GlUint, const char*);
using Uniform1iFn = void (RV_SCALE_GL_CALL*)(GlInt, GlInt);
using Uniform2fFn = void (RV_SCALE_GL_CALL*)(GlInt, GlFloat, GlFloat);
using GetErrorFn = GlEnum (RV_SCALE_GL_CALL*)();

constexpr GlEnum kTexture2D = 0x0DE1;
constexpr GlEnum kTextureBinding2D = 0x8069;
constexpr GlEnum kTexture0 = 0x84C0;
constexpr GlEnum kActiveTexture = 0x84E0;
constexpr GlEnum kFramebuffer = 0x8D40;
constexpr GlEnum kFramebufferBinding = 0x8CA6;
constexpr GlEnum kViewport = 0x0BA2;
constexpr GlEnum kScissorBox = 0x0C10;
constexpr GlEnum kScissorTest = 0x0C11;
constexpr GlEnum kBlend = 0x0BE2;
constexpr GlEnum kDepthTest = 0x0B71;
constexpr GlEnum kCullFace = 0x0B44;
constexpr GlEnum kStencilTest = 0x0B90;
constexpr GlEnum kAlphaTest = 0x0BC0;
constexpr GlEnum kColorLogicOp = 0x0BF2;
constexpr GlEnum kDither = 0x0BD0;
constexpr GlEnum kColorWriteMask = 0x0C23;
constexpr GlEnum kPolygonMode = 0x0B40;
constexpr GlEnum kFront = 0x0404;
constexpr GlEnum kBack = 0x0405;
constexpr GlEnum kFill = 0x1B02;
constexpr GlEnum kCurrentProgram = 0x8B8D;
constexpr GlEnum kVertexShader = 0x8B31;
constexpr GlEnum kFragmentShader = 0x8B30;
constexpr GlEnum kCompileStatus = 0x8B81;
constexpr GlEnum kLinkStatus = 0x8B82;
constexpr GlEnum kQuads = 0x0007;
constexpr GlEnum kNoError = 0;

template<typename Function>
Function procedure(const GlFunctionTable& functions, GlProcedure name) noexcept
{
	return reinterpret_cast<Function>(functions.get(name));
}

bool valid_scale(uint32_t scale) noexcept
{
	return scale == RV_SCALE_ORIGINAL || scale == RV_SCALE_2X || scale == RV_SCALE_4X;
}

bool same_pixel(const uint8_t* first, const uint8_t* second) noexcept
{
	return first[0] == second[0] && first[1] == second[1] &&
		first[2] == second[2] && first[3] == second[3];
}

class ScaleGlState {
public:
	ScaleGlState(ActiveTextureFn active_texture,
		BindTextureFn bind_texture, BindFramebufferFn bind_framebuffer,
		UseProgramFn use_program) noexcept
		: active_texture_(active_texture), bind_texture_(bind_texture),
		  bind_framebuffer_(bind_framebuffer), use_program_(use_program)
	{
		::glGetIntegerv(kFramebufferBinding, &framebuffer_);
		::glGetIntegerv(kViewport, viewport_.data());
		::glGetIntegerv(kScissorBox, scissor_.data());
		::glGetIntegerv(kPolygonMode, polygon_mode_.data());
		::glGetIntegerv(kCurrentProgram, &program_);
		::glGetBooleanv(kColorWriteMask, color_mask_.data());
		const std::array<GlEnum, 8> names{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kDither}};
		for(size_t index = 0; index < names.size(); ++index) {
			capabilities_[index] = ::glIsEnabled(names[index]) == GL_TRUE;
		}
		::glGetIntegerv(kActiveTexture, &active_texture_unit_);
		active_texture_(kTexture0);
		::glGetIntegerv(kTextureBinding2D, &texture_unit_zero_);
		active_texture_(static_cast<GlEnum>(active_texture_unit_));
		valid_ = true;
	}

	~ScaleGlState()
	{
		if(!valid_) return;
		bind_framebuffer_(kFramebuffer, static_cast<GlUint>(framebuffer_));
		::glViewport(viewport_[0], viewport_[1], viewport_[2], viewport_[3]);
		::glScissor(scissor_[0], scissor_[1], scissor_[2], scissor_[3]);
		::glPolygonMode(kFront, static_cast<GlEnum>(polygon_mode_[0]));
		::glPolygonMode(kBack, static_cast<GlEnum>(polygon_mode_[1]));
		::glColorMask(color_mask_[0], color_mask_[1], color_mask_[2], color_mask_[3]);
		use_program_(static_cast<GlUint>(program_));
		active_texture_(kTexture0);
		bind_texture_(kTexture2D, static_cast<GlUint>(texture_unit_zero_));
		active_texture_(static_cast<GlEnum>(active_texture_unit_));
		const std::array<GlEnum, 8> names{{kBlend, kScissorTest, kDepthTest, kCullFace,
			kStencilTest, kAlphaTest, kColorLogicOp, kDither}};
		for(size_t index = 0; index < names.size(); ++index) {
			if(capabilities_[index]) ::glEnable(names[index]);
			else ::glDisable(names[index]);
		}
	}

private:
	ActiveTextureFn active_texture_;
	BindTextureFn bind_texture_;
	BindFramebufferFn bind_framebuffer_;
	UseProgramFn use_program_;
	GlInt framebuffer_ = 0;
	GlInt active_texture_unit_ = static_cast<GlInt>(kTexture0);
	GlInt texture_unit_zero_ = 0;
	GlInt program_ = 0;
	std::array<GlInt, 4> viewport_{};
	std::array<GlInt, 4> scissor_{};
	std::array<GlInt, 2> polygon_mode_{};
	std::array<GlBoolean, 4> color_mask_{};
	std::array<bool, 8> capabilities_{};
	bool valid_ = false;
};

class TemporaryTexture {
public:
	explicit TemporaryTexture(TextureStore& store) noexcept : store_(store) {}
	~TemporaryTexture()
	{
		if(allocation_.texture != 0) store_.release_scratch(allocation_);
	}
	RvResult allocate(uint32_t width, uint32_t height) noexcept
	{
		return store_.reserve_temporary(width, height, &allocation_);
	}
	const TextureAllocation& get() const noexcept { return allocation_; }
	RvResult release() noexcept
	{
		if(allocation_.texture == 0) return RV_RESULT_OK;
		const RvResult status = store_.release_scratch(allocation_);
		if(status == RV_RESULT_OK) allocation_ = TextureAllocation{};
		return status;
	}

private:
	TextureStore& store_;
	TextureAllocation allocation_{};
};

} // namespace

RvResult scale2x_reference_rgba(const uint8_t* source, size_t source_bytes,
	uint32_t width, uint32_t height, std::vector<uint8_t>* output) noexcept
{
	if(source == nullptr || output == nullptr || width == 0 || height == 0) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const uint64_t source_pixels = static_cast<uint64_t>(width) * height;
	if(source_pixels > RV_MAX_LOGICAL_SOURCE_PIXELS) return RV_RESULT_RESOURCE_LIMIT;
	const uint64_t expected_source_bytes = source_pixels * 4;
	if(expected_source_bytes != source_bytes) return RV_RESULT_INVALID_ARGUMENT;
	if(width > UINT32_MAX / 2 || height > UINT32_MAX / 2) return RV_RESULT_RESOURCE_LIMIT;
	const uint32_t output_width = width * 2;
	const uint32_t output_height = height * 2;
	const uint64_t output_bytes = static_cast<uint64_t>(output_width) * output_height * 4;
	if(output_bytes > RV_MAX_QUEUED_BYTES) return RV_RESULT_RESOURCE_LIMIT;

	try {
		std::vector<uint8_t> scaled(static_cast<size_t>(output_bytes));
		auto pixel = [&](int64_t x, int64_t y) -> const uint8_t* {
			x = std::max<int64_t>(0, std::min<int64_t>(width - 1, x));
			y = std::max<int64_t>(0, std::min<int64_t>(height - 1, y));
			return source + (static_cast<size_t>(y) * width + static_cast<size_t>(x)) * 4;
		};
		for(uint32_t y = 0; y < height; ++y) {
			for(uint32_t x = 0; x < width; ++x) {
				const uint8_t* center = pixel(x, y);
				const uint8_t* north = pixel(x, static_cast<int64_t>(y) - 1);
				const uint8_t* west = pixel(static_cast<int64_t>(x) - 1, y);
				const uint8_t* east = pixel(static_cast<int64_t>(x) + 1, y);
				const uint8_t* south = pixel(x, static_cast<int64_t>(y) + 1);
				for(uint32_t dy = 0; dy < 2; ++dy) {
					for(uint32_t dx = 0; dx < 2; ++dx) {
						const uint8_t* chosen = center;
						if(!same_pixel(north, south) && !same_pixel(west, east)) {
							if(dx == 0 && dy == 0 && same_pixel(west, north)) chosen = west;
							else if(dx == 1 && dy == 0 && same_pixel(north, east)) chosen = east;
							else if(dx == 0 && dy == 1 && same_pixel(west, south)) chosen = west;
							else if(dx == 1 && dy == 1 && same_pixel(south, east)) chosen = east;
						}
						const size_t output_offset =
							(static_cast<size_t>(y * 2 + dy) * output_width + x * 2 + dx) * 4;
						std::copy_n(chosen, 4, scaled.data() + output_offset);
					}
				}
			}
		}
		*output = std::move(scaled);
		return RV_RESULT_OK;
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

Scale2xRenderer::Scale2xRenderer(TextureStore& store, SurfaceOperations& operations,
	const GlFunctionTable& functions) noexcept
	: store_(store), operations_(operations), functions_(functions)
{
}

RvResult Scale2xRenderer::apply(RvSurfaceHandle source_handle,
	RvSurfaceHandle destination_handle, ScaleReport* report) noexcept
{
	if(report == nullptr) return RV_RESULT_INVALID_ARGUMENT;
	*report = ScaleReport{};
	TextureSurfaceSnapshot source{};
	TextureSurfaceSnapshot destination{};
	RvResult status = store_.inspect(source_handle, &source);
	if(status != RV_RESULT_OK) return status;
	status = store_.inspect(destination_handle, &destination);
	if(status != RV_RESULT_OK) return status;
	report->source_scale = source.storage_scale;
	report->requested_scale = destination.storage_scale;
	if(!source.initialized || source.logical_width != destination.logical_width ||
		source.logical_height != destination.logical_height ||
		!valid_scale(source.storage_scale) || !valid_scale(destination.storage_scale)) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	if(source_handle == destination_handle) {
		if(source.storage_scale != destination.storage_scale) return RV_RESULT_INVALID_ARGUMENT;
		report->path = ScalePath::AlreadyDetailed;
		return RV_RESULT_OK;
	}

	if(source.storage_scale >= destination.storage_scale) {
		status = compatibility_copy(source_handle, destination_handle);
		if(status == RV_RESULT_OK) {
			report->path = ScalePath::CompatibilitySampling;
			++statistics_.compatibility_surfaces;
		}
		return status;
	}

	status = ensure_program();
	if(status != RV_RESULT_OK) {
		report->fallback_reason = shader_failure_reason_;
		status = compatibility_copy(source_handle, destination_handle);
		if(status == RV_RESULT_OK) {
			report->path = ScalePath::CompatibilitySampling;
			++statistics_.compatibility_surfaces;
		}
		return status;
	}

	const uint32_t factor = destination.storage_scale / source.storage_scale;
	if(factor != 2 && factor != 4) return RV_RESULT_UNSUPPORTED_OPERATION;
	const uint32_t pass_count = factor == 2 ? 1 : 2;
	TemporaryTexture intermediate(store_);
	const TextureAllocation* input = &source.active;
	uint32_t input_width = source.active.width;
	uint32_t input_height = source.active.height;
	if(pass_count == 2) {
		if(input_width > UINT32_MAX / 2 || input_height > UINT32_MAX / 2) {
			report->fallback_reason = ScaleFallbackReason::TemporaryResourceLimit;
			status = compatibility_copy(source_handle, destination_handle);
			if(status == RV_RESULT_OK) {
				report->path = ScalePath::CompatibilitySampling;
				++statistics_.compatibility_surfaces;
			}
			return status;
		}
		status = intermediate.allocate(input_width * 2, input_height * 2);
		if(status != RV_RESULT_OK) {
			report->fallback_reason = ScaleFallbackReason::TemporaryResourceLimit;
			status = compatibility_copy(source_handle, destination_handle);
			if(status == RV_RESULT_OK) {
				report->path = ScalePath::CompatibilitySampling;
				++statistics_.compatibility_surfaces;
			}
			return status;
		}
	}

	const TextureAllocation& first_output = pass_count == 2 ? intermediate.get() : destination.active;
	status = draw_pass(*input, first_output, input_width, input_height);
	if(status == RV_RESULT_OK) {
		++statistics_.completed_gpu_passes;
		report->passes = 1;
	}
	if(status == RV_RESULT_OK && pass_count == 2) {
		status = draw_pass(intermediate.get(), destination.active,
			intermediate.get().width, intermediate.get().height);
		if(status == RV_RESULT_OK) {
			++statistics_.completed_gpu_passes;
			report->passes = 2;
		}
	}
	const RvResult released = intermediate.release();
	if(status == RV_RESULT_OK && released != RV_RESULT_OK) status = released;
	if(status == RV_RESULT_OK) {
		status = store_.mark_modified(destination_handle);
		if(status == RV_RESULT_OK) {
			report->path = ScalePath::GpuScale2x;
			++statistics_.gpu_scaled_surfaces;
			return RV_RESULT_OK;
		}
	}

	report->fallback_reason = ScaleFallbackReason::GpuExecutionFailed;
	if(!shader_failed_) {
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::GpuExecutionFailed;
		++statistics_.shader_failures;
	}
	status = compatibility_copy(source_handle, destination_handle);
	if(status == RV_RESULT_OK) {
		report->path = ScalePath::CompatibilitySampling;
		++statistics_.compatibility_surfaces;
	}
	return status;
}

RvResult Scale2xRenderer::ensure_program() noexcept
{
	if(program_ != 0) return RV_RESULT_OK;
	if(shader_failed_) return RV_RESULT_UNSUPPORTED_OPERATION;
	const auto create_shader = procedure<CreateShaderFn>(functions_, GlProcedure::CreateShader);
	const auto shader_source = procedure<ShaderSourceFn>(functions_, GlProcedure::ShaderSource);
	const auto compile_shader = procedure<CompileShaderFn>(functions_, GlProcedure::CompileShader);
	const auto get_shader = procedure<GetShaderivFn>(functions_, GlProcedure::GetShaderiv);
	const auto delete_shader = procedure<DeleteShaderFn>(functions_, GlProcedure::DeleteShader);
	const auto create_program = procedure<CreateProgramFn>(functions_, GlProcedure::CreateProgram);
	const auto attach_shader = procedure<AttachShaderFn>(functions_, GlProcedure::AttachShader);
	const auto link_program = procedure<LinkProgramFn>(functions_, GlProcedure::LinkProgram);
	const auto get_program = procedure<GetProgramivFn>(functions_, GlProcedure::GetProgramiv);
	const auto delete_program = procedure<DeleteProgramFn>(functions_, GlProcedure::DeleteProgram);
	const auto get_uniform = procedure<GetUniformLocationFn>(functions_, GlProcedure::GetUniformLocation);
	if(create_shader == nullptr || shader_source == nullptr || compile_shader == nullptr ||
		get_shader == nullptr || delete_shader == nullptr || create_program == nullptr ||
		attach_shader == nullptr || link_program == nullptr || get_program == nullptr ||
		delete_program == nullptr || get_uniform == nullptr ||
		functions_.get(GlProcedure::ActiveTexture) == nullptr ||
		functions_.get(GlProcedure::UseProgram) == nullptr ||
		functions_.get(GlProcedure::Uniform1i) == nullptr ||
		functions_.get(GlProcedure::Uniform2f) == nullptr) {
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ShaderUnavailable;
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}

	const char* vertex_text = scale2x_shader::kVertex;
	const char* fragment_text = scale2x_shader::kFragment;
	const GlUint vertex_shader = create_shader(kVertexShader);
	const GlUint fragment_shader = create_shader(kFragmentShader);
	if(vertex_shader == 0 || fragment_shader == 0) {
		if(vertex_shader != 0) delete_shader(vertex_shader);
		if(fragment_shader != 0) delete_shader(fragment_shader);
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ShaderCompileFailed;
		++statistics_.shader_failures;
		return RV_RESULT_GL_FAILURE;
	}
	shader_source(vertex_shader, 1, &vertex_text, nullptr);
	compile_shader(vertex_shader);
	GlInt vertex_ok = 0;
	get_shader(vertex_shader, kCompileStatus, &vertex_ok);
	shader_source(fragment_shader, 1, &fragment_text, nullptr);
	compile_shader(fragment_shader);
	GlInt fragment_ok = 0;
	get_shader(fragment_shader, kCompileStatus, &fragment_ok);
	if(vertex_ok == 0 || fragment_ok == 0) {
		delete_shader(vertex_shader);
		delete_shader(fragment_shader);
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ShaderCompileFailed;
		++statistics_.shader_failures;
		return RV_RESULT_GL_FAILURE;
	}
	const GlUint candidate = create_program();
	if(candidate == 0) {
		delete_shader(vertex_shader);
		delete_shader(fragment_shader);
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ProgramLinkFailed;
		++statistics_.shader_failures;
		return RV_RESULT_GL_FAILURE;
	}
	attach_shader(candidate, vertex_shader);
	attach_shader(candidate, fragment_shader);
	link_program(candidate);
	GlInt linked = 0;
	get_program(candidate, kLinkStatus, &linked);
	delete_shader(vertex_shader);
	delete_shader(fragment_shader);
	if(linked == 0) {
		delete_program(candidate);
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ProgramLinkFailed;
		++statistics_.shader_failures;
		return RV_RESULT_GL_FAILURE;
	}
	const GlInt source_texture = get_uniform(candidate, "sourceTexture");
	const GlInt source_size = get_uniform(candidate, "sourceSize");
	const GlInt output_size = get_uniform(candidate, "outputSize");
	if(source_texture < 0 || source_size < 0 || output_size < 0) {
		delete_program(candidate);
		shader_failed_ = true;
		shader_failure_reason_ = ScaleFallbackReason::ProgramLinkFailed;
		++statistics_.shader_failures;
		return RV_RESULT_GL_FAILURE;
	}
	program_ = candidate;
	source_texture_uniform_ = source_texture;
	source_size_uniform_ = source_size;
	output_size_uniform_ = output_size;
	return RV_RESULT_OK;
}

RvResult Scale2xRenderer::draw_pass(const TextureAllocation& source,
	const TextureAllocation& destination, uint32_t source_width,
	uint32_t source_height) noexcept
{
	if(source.texture == 0 || destination.framebuffer == 0 || source_width == 0 || source_height == 0 ||
		source.width != source_width || source.height != source_height ||
		source_width > UINT32_MAX / 2 || source_height > UINT32_MAX / 2 ||
		destination.width != source_width * 2 || destination.height != source_height * 2) {
		return RV_RESULT_INVALID_ARGUMENT;
	}
	const auto active_texture = procedure<ActiveTextureFn>(functions_, GlProcedure::ActiveTexture);
	const auto bind_texture = procedure<BindTextureFn>(functions_, GlProcedure::BindTexture);
	const auto bind_framebuffer = procedure<BindFramebufferFn>(functions_, GlProcedure::BindFramebuffer);
	const auto use_program = procedure<UseProgramFn>(functions_, GlProcedure::UseProgram);
	const auto uniform_1i = procedure<Uniform1iFn>(functions_, GlProcedure::Uniform1i);
	const auto uniform_2f = procedure<Uniform2fFn>(functions_, GlProcedure::Uniform2f);
	const auto get_error = procedure<GetErrorFn>(functions_, GlProcedure::GetError);
	if(active_texture == nullptr || bind_texture == nullptr || bind_framebuffer == nullptr ||
		use_program == nullptr || uniform_1i == nullptr || uniform_2f == nullptr || get_error == nullptr) {
		return RV_RESULT_UNSUPPORTED_OPERATION;
	}
	ScaleGlState saved(active_texture, bind_texture, bind_framebuffer, use_program);
	active_texture(kTexture0);
	bind_framebuffer(kFramebuffer, destination.framebuffer);
	::glViewport(0, 0, static_cast<GlInt>(destination.width), static_cast<GlInt>(destination.height));
	::glDisable(kScissorTest);
	::glDisable(kBlend);
	::glDisable(kDepthTest);
	::glDisable(kCullFace);
	::glDisable(kStencilTest);
	::glDisable(kAlphaTest);
	::glDisable(kColorLogicOp);
	::glDisable(kDither);
	::glPolygonMode(kFront, kFill);
	::glPolygonMode(kBack, kFill);
	::glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
	bind_texture(kTexture2D, source.texture);
	use_program(program_);
	uniform_1i(source_texture_uniform_, 0);
	uniform_2f(source_size_uniform_, static_cast<GlFloat>(source_width),
		static_cast<GlFloat>(source_height));
	uniform_2f(output_size_uniform_, static_cast<GlFloat>(destination.width),
		static_cast<GlFloat>(destination.height));
	::glBegin(kQuads);
	::glVertex2f(-1.f, -1.f);
	::glVertex2f(1.f, -1.f);
	::glVertex2f(1.f, 1.f);
	::glVertex2f(-1.f, 1.f);
	::glEnd();
	return get_error() == kNoError ? RV_RESULT_OK : RV_RESULT_GL_FAILURE;
}

RvResult Scale2xRenderer::compatibility_copy(
	RvSurfaceHandle source, RvSurfaceHandle destination) noexcept
{
	TextureSurfaceSnapshot source_surface{};
	TextureSurfaceSnapshot destination_surface{};
	RvResult status = store_.inspect(source, &source_surface);
	if(status != RV_RESULT_OK) return status;
	status = store_.inspect(destination, &destination_surface);
	if(status != RV_RESULT_OK) return status;
	SurfaceCommand command{};
	command.opcode = RV_OP_BLIT;
	command.source = source;
	command.destination = destination;
	command.source_rect = SurfaceRect{0, 0,
		static_cast<int32_t>(source_surface.logical_width),
		static_cast<int32_t>(source_surface.logical_height)};
	command.destination_rect = SurfaceRect{0, 0,
		static_cast<int32_t>(destination_surface.logical_width),
		static_cast<int32_t>(destination_surface.logical_height)};
	command.clip = command.destination_rect;
	command.blend = SurfaceBlend::Replace;
	try {
		return operations_.execute(std::vector<SurfaceCommand>{command});
	} catch(const std::bad_alloc&) {
		return RV_RESULT_RESOURCE_LIMIT;
	} catch(...) {
		return RV_RESULT_INTERNAL_ERROR;
	}
}

void Scale2xRenderer::destroy() noexcept
{
	if(program_ == 0 || store_.check_owner() != RV_RESULT_OK) return;
	const auto delete_program = procedure<DeleteProgramFn>(functions_, GlProcedure::DeleteProgram);
	if(delete_program != nullptr) delete_program(program_);
	program_ = 0;
	source_texture_uniform_ = -1;
	source_size_uniform_ = -1;
	output_size_uniform_ = -1;
}

Scale2xStatistics Scale2xRenderer::statistics() const noexcept
{
	return statistics_;
}

} // namespace revival::graphics
