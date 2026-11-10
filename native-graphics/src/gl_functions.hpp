#ifndef REVIVAL_GRAPHICS_GL_FUNCTIONS_HPP
#define REVIVAL_GRAPHICS_GL_FUNCTIONS_HPP

#include <array>
#include <cstddef>

namespace revival::graphics {

using ProcLookup = void* (*)(void* user_data, const char* name);

struct ProcResolver {
	void* user_data = nullptr;
	ProcLookup wgl_lookup = nullptr;
	ProcLookup opengl32_lookup = nullptr;
};

enum class GlProcedure : size_t {
	GetString,
	GetIntegerv,
	GetError,
	GenTextures,
	BindTexture,
	DeleteTextures,
	TexImage2D,
	TexSubImage2D,
	TexParameteri,
	PixelStorei,
	GenFramebuffers,
	BindFramebuffer,
	FramebufferTexture2D,
	CheckFramebufferStatus,
	DeleteFramebuffers,
	BlendFuncSeparate,
	BlendEquationSeparate,
	ActiveTexture,
	CreateShader,
	ShaderSource,
	CompileShader,
	GetShaderiv,
	GetShaderInfoLog,
	DeleteShader,
	CreateProgram,
	AttachShader,
	LinkProgram,
	GetProgramiv,
	GetProgramInfoLog,
	DeleteProgram,
	UseProgram,
	GetUniformLocation,
	Uniform1i,
	Uniform2f,
	Count
};

struct GlFunctionTable {
	static constexpr size_t required_count = static_cast<size_t>(GlProcedure::ActiveTexture);
	static constexpr size_t procedure_count = static_cast<size_t>(GlProcedure::Count);
	static constexpr size_t optional_count = procedure_count - required_count;
	std::array<void*, procedure_count> procedures{};
	void* get(GlProcedure procedure) const noexcept
	{
		return procedures[static_cast<size_t>(procedure)];
	}
};

bool is_invalid_wgl_address(void* address) noexcept;
bool load_required_gl_functions(
	const ProcResolver& resolver, GlFunctionTable* out, const char** missing_name) noexcept;

#if defined(_WIN32)
ProcResolver current_windows_gl_resolver() noexcept;
#endif

} // namespace revival::graphics

#endif
