#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

namespace revival::nes {

constexpr std::size_t RV_NES_BUTTON_COUNT = 8;
constexpr uint16_t RV_NES_BUTTON_UP = 1u << 0;
constexpr uint16_t RV_NES_BUTTON_DOWN = 1u << 1;
constexpr uint16_t RV_NES_BUTTON_LEFT = 1u << 2;
constexpr uint16_t RV_NES_BUTTON_RIGHT = 1u << 3;
constexpr uint16_t RV_NES_BUTTON_A = 1u << 4;
constexpr uint16_t RV_NES_BUTTON_B = 1u << 5;
constexpr uint16_t RV_NES_BUTTON_START = 1u << 6;
constexpr uint16_t RV_NES_BUTTON_SELECT = 1u << 7;
constexpr uint16_t RV_NES_BUTTON_ALL = (1u << RV_NES_BUTTON_COUNT) - 1u;

struct NesPlatformInputCodes final {
	// These values come from the initialized frontend, not from a fallback map.
	uint32_t escape = 0;
	uint32_t fullscreen = 0;
};

struct NesKeyBinding final {
	uint32_t platform_code = 0;
	uint16_t button = 0;
};

struct NesControllerSample final {
	uint16_t buttons = 0;
	bool connected = false;
};

class NesInputMapper final {
public:
	bool initialize(const NesPlatformInputCodes& codes) noexcept;
	bool initialized() const noexcept { return initialized_; }
	bool configure(const NesKeyBinding* bindings, std::size_t count) noexcept;
	bool key_down(uint32_t platform_code) noexcept;
	bool key_up(uint32_t platform_code) noexcept;
	void focus_lost() noexcept;
	uint16_t sample(NesControllerSample controller = {}) const noexcept;
	bool is_reserved(uint32_t platform_code) const noexcept;
	uint16_t keyboard_buttons() const noexcept { return keyboard_buttons_; }

private:
	static bool valid_button(uint16_t button) noexcept;
	int find_binding(uint32_t platform_code) const noexcept;

	NesPlatformInputCodes codes_{};
	std::array<NesKeyBinding, RV_NES_BUTTON_COUNT> bindings_{};
	std::size_t binding_count_ = 0;
	uint16_t keyboard_buttons_ = 0;
	bool initialized_ = false;
};

} // namespace revival::nes
