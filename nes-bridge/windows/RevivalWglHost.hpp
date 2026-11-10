#pragma once

#ifdef _WIN32

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>

#include <cstdint>

namespace revival::nes {

class RevivalWglHost final {
public:
	RevivalWglHost() = default;
	~RevivalWglHost();

	RevivalWglHost(const RevivalWglHost&) = delete;
	RevivalWglHost& operator=(const RevivalWglHost&) = delete;

	bool attach(HWND window) noexcept;
	void detach() noexcept;
	bool make_current() noexcept;
	void set_vertical_sync(bool enabled) noexcept;
	bool swap_buffers() noexcept;
	bool client_size(uint32_t& width, uint32_t& height) const noexcept;
	bool attached() const noexcept { return context_ != nullptr && dc_ != nullptr; }
	HWND window() const noexcept { return window_; }

private:
	HWND window_ = nullptr;
	HDC dc_ = nullptr;
	HGLRC context_ = nullptr;
	using SwapInterval = BOOL (WINAPI*)(int);
	SwapInterval swap_interval_ = nullptr;
	int applied_interval_ = -1;
};

} // namespace revival::nes

#endif
