#include "gl_functions.hpp"

#include <cstdint>
#include <limits>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>
#endif

namespace revival::graphics {
namespace {

constexpr const char* kRequiredFunctions[] = {
	"glGetString",
	"glGetIntegerv",
	"glGetError",
	"glGenTextures",
	"glBindTexture",
	"glDeleteTextures",
	"glTexImage2D",
	"glTexSubImage2D",
	"glTexParameteri",
	"glPixelStorei",
	"glGenFramebuffers",
	"glBindFramebuffer",
	"glFramebufferTexture2D",
	"glCheckFramebufferStatus",
	"glDeleteFramebuffers",
	"glBlendFuncSeparate",
	"glBlendEquationSeparate",
};

constexpr const char* kFallbackFunctions[] = {
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	nullptr,
	"glGenFramebuffersEXT",
	"glBindFramebufferEXT",
	"glFramebufferTexture2DEXT",
	"glCheckFramebufferStatusEXT",
	"glDeleteFramebuffersEXT",
	"glBlendFuncSeparateEXT",
	"glBlendEquationSeparateEXT",
};

constexpr const char* kOptionalFunctions[] = {
	"glActiveTexture",
	"glCreateShader",
	"glShaderSource",
	"glCompileShader",
	"glGetShaderiv",
	"glGetShaderInfoLog",
	"glDeleteShader",
	"glCreateProgram",
	"glAttachShader",
	"glLinkProgram",
	"glGetProgramiv",
	"glGetProgramInfoLog",
	"glDeleteProgram",
	"glUseProgram",
	"glGetUniformLocation",
	"glUniform1i",
	"glUniform2f",
};

constexpr const char* kOptionalFallbackFunctions[] = {
	"glActiveTextureARB",
	nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
	nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
};

static_assert(sizeof(kRequiredFunctions) / sizeof(kRequiredFunctions[0]) ==
	GlFunctionTable::required_count, "required GL procedure table must match the enum");
static_assert(sizeof(kOptionalFunctions) / sizeof(kOptionalFunctions[0]) ==
	GlFunctionTable::optional_count, "optional GL procedure table must match the enum");
static_assert(sizeof(kOptionalFallbackFunctions) / sizeof(kOptionalFallbackFunctions[0]) ==
	GlFunctionTable::optional_count, "optional GL aliases must match the enum");

bool usable_module_address(void* address) noexcept
{
	return address != nullptr && !is_invalid_wgl_address(address);
}

#if defined(_WIN32)
void* lookup_wgl(void*, const char* name)
{
	return reinterpret_cast<void*>(::wglGetProcAddress(name));
}

void* lookup_opengl32(void*, const char* name)
{
	static HMODULE module = []() noexcept {
		HMODULE loaded = ::GetModuleHandleW(L"opengl32.dll");
		if(loaded == nullptr) {
			loaded = ::LoadLibraryW(L"opengl32.dll");
		}
		return loaded;
	}();
	if(module == nullptr) {
		return nullptr;
	}
	return reinterpret_cast<void*>(::GetProcAddress(module, name));
}
#endif

} // namespace

bool is_invalid_wgl_address(void* address) noexcept
{
	const uintptr_t value = reinterpret_cast<uintptr_t>(address);
	return value == 0 || value == 1 || value == 2 || value == 3 ||
		value == std::numeric_limits<uintptr_t>::max();
}

bool load_required_gl_functions(
	const ProcResolver& resolver, GlFunctionTable* out, const char** missing_name) noexcept
{
	if(out == nullptr) {
		return false;
	}
	*out = GlFunctionTable{};
	if(missing_name != nullptr) {
		*missing_name = nullptr;
	}
	if(resolver.wgl_lookup == nullptr || resolver.opengl32_lookup == nullptr) {
		return false;
	}

	GlFunctionTable loaded{};
	for(size_t index = 0; index < GlFunctionTable::required_count; ++index) {
		void* address = resolver.wgl_lookup(resolver.user_data, kRequiredFunctions[index]);
		if(is_invalid_wgl_address(address)) {
			if(kFallbackFunctions[index] != nullptr) {
				address = resolver.wgl_lookup(resolver.user_data, kFallbackFunctions[index]);
			}
		}
		if(is_invalid_wgl_address(address)) {
			address = resolver.opengl32_lookup(resolver.user_data, kRequiredFunctions[index]);
		}
		if(is_invalid_wgl_address(address) && kFallbackFunctions[index] != nullptr) {
			address = resolver.opengl32_lookup(resolver.user_data, kFallbackFunctions[index]);
		}
		if(!usable_module_address(address)) {
			if(missing_name != nullptr) {
				*missing_name = kRequiredFunctions[index];
			}
			return false;
		}
		loaded.procedures[index] = address;
	}
	for(size_t optional = 0; optional < GlFunctionTable::optional_count; ++optional) {
		const size_t index = GlFunctionTable::required_count + optional;
		void* address = resolver.wgl_lookup(resolver.user_data, kOptionalFunctions[optional]);
		if(is_invalid_wgl_address(address) && kOptionalFallbackFunctions[optional] != nullptr) {
			address = resolver.wgl_lookup(resolver.user_data, kOptionalFallbackFunctions[optional]);
		}
		if(is_invalid_wgl_address(address)) {
			address = resolver.opengl32_lookup(resolver.user_data, kOptionalFunctions[optional]);
		}
		if(is_invalid_wgl_address(address) && kOptionalFallbackFunctions[optional] != nullptr) {
			address = resolver.opengl32_lookup(resolver.user_data, kOptionalFallbackFunctions[optional]);
		}
		if(!usable_module_address(address)) address = nullptr;
		loaded.procedures[index] = address;
	}
	*out = loaded;
	return true;
}

#if defined(_WIN32)
ProcResolver current_windows_gl_resolver() noexcept
{
	return ProcResolver{nullptr, &lookup_wgl, &lookup_opengl32};
}
#endif

} // namespace revival::graphics
