#include "RevivalWglHost.hpp"

#ifdef _WIN32

namespace revival::nes {

RevivalWglHost::~RevivalWglHost()
{
	detach();
}

bool RevivalWglHost::attach(HWND window) noexcept
{
	if(window == nullptr || attached()) return false;
	HDC dc = ::GetDC(window);
	if(dc == nullptr) return false;

	PIXELFORMATDESCRIPTOR requested{};
	requested.nSize = sizeof(requested);
	requested.nVersion = 1;
	requested.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
	requested.iPixelType = PFD_TYPE_RGBA;
	requested.cColorBits = 32;
	requested.cAlphaBits = 8;
	requested.iLayerType = PFD_MAIN_PLANE;
	int pixel_format = ::GetPixelFormat(dc);
	if(pixel_format == 0) pixel_format = ::ChoosePixelFormat(dc, &requested);
	if(pixel_format == 0 || (::GetPixelFormat(dc) == 0 &&
		!::SetPixelFormat(dc, pixel_format, &requested))) {
		::ReleaseDC(window, dc);
		return false;
	}

	HGLRC context = ::wglCreateContext(dc);
	if(context == nullptr || !::wglMakeCurrent(dc, context)) {
		if(context != nullptr) ::wglDeleteContext(context);
		::ReleaseDC(window, dc);
		return false;
	}

	window_ = window;
	dc_ = dc;
	context_ = context;
	const auto address = ::wglGetProcAddress("wglSwapIntervalEXT");
	const auto value = reinterpret_cast<intptr_t>(address);
	if(value != 0 && value != 1 && value != 2 && value != 3 && value != -1) {
		swap_interval_ = reinterpret_cast<SwapInterval>(address);
	}
	return true;
}

void RevivalWglHost::detach() noexcept
{
	if(context_ != nullptr && ::wglGetCurrentContext() == context_) {
		::wglMakeCurrent(nullptr, nullptr);
	}
	if(context_ != nullptr) ::wglDeleteContext(context_);
	if(window_ != nullptr && dc_ != nullptr) ::ReleaseDC(window_, dc_);
	window_ = nullptr;
	dc_ = nullptr;
	context_ = nullptr;
	swap_interval_ = nullptr;
	applied_interval_ = -1;
}

bool RevivalWglHost::make_current() noexcept
{
	return attached() && ::wglMakeCurrent(dc_, context_) == TRUE;
}

void RevivalWglHost::set_vertical_sync(bool enabled) noexcept
{
	const int interval = enabled ? 1 : 0;
	if(swap_interval_ != nullptr && interval != applied_interval_) {
		// Cache the attempt so unsupported driver settings do not cost a call every frame.
		applied_interval_ = interval;
		swap_interval_(interval);
	}
}

bool RevivalWglHost::swap_buffers() noexcept
{
	return attached() && ::SwapBuffers(dc_) == TRUE;
}

bool RevivalWglHost::client_size(uint32_t& width, uint32_t& height) const noexcept
{
	RECT rect{};
	if(window_ == nullptr || !::GetClientRect(window_, &rect) ||
		rect.right <= rect.left || rect.bottom <= rect.top) {
		return false;
	}
	width = static_cast<uint32_t>(rect.right - rect.left);
	height = static_cast<uint32_t>(rect.bottom - rect.top);
	return true;
}

} // namespace revival::nes

#endif
