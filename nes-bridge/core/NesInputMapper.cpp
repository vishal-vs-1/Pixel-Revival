#include "NesInputMapper.hpp"

#include <algorithm>

namespace revival::nes {

namespace {

constexpr bool single_button(uint16_t value) noexcept
{
	return value != 0 && (value & static_cast<uint16_t>(value - 1)) == 0;
}

} // namespace

bool NesInputMapper::initialize(const NesPlatformInputCodes& codes) noexcept
{
	if(codes.escape == 0 || codes.fullscreen == 0 || codes.escape == codes.fullscreen) {
		return false;
	}
	codes_ = codes;
	binding_count_ = 0;
	keyboard_buttons_ = 0;
	initialized_ = true;
	return true;
}

bool NesInputMapper::valid_button(uint16_t button) noexcept
{
	return single_button(button) && (button & RV_NES_BUTTON_ALL) != 0;
}

bool NesInputMapper::configure(const NesKeyBinding* bindings, std::size_t count) noexcept
{
	if(!initialized_ || bindings == nullptr || count != RV_NES_BUTTON_COUNT) {
		return false;
	}
	std::array<NesKeyBinding, RV_NES_BUTTON_COUNT> next{};
	for(std::size_t index = 0; index < count; ++index) {
		const auto& candidate = bindings[index];
		if(candidate.platform_code == 0 || is_reserved(candidate.platform_code) ||
			!valid_button(candidate.button)) {
			return false;
		}
		for(std::size_t previous = 0; previous < index; ++previous) {
			if(next[previous].platform_code == candidate.platform_code ||
				next[previous].button == candidate.button) {
				return false;
			}
		}
		next[index] = candidate;
	}
	bindings_ = next;
	binding_count_ = count;
	keyboard_buttons_ = 0;
	return true;
}

int NesInputMapper::find_binding(uint32_t platform_code) const noexcept
{
	for(std::size_t index = 0; index < binding_count_; ++index) {
		if(bindings_[index].platform_code == platform_code) {
			return static_cast<int>(index);
		}
	}
	return -1;
}

bool NesInputMapper::is_reserved(uint32_t platform_code) const noexcept
{
	return platform_code != 0 &&
		(platform_code == codes_.escape || platform_code == codes_.fullscreen);
}

bool NesInputMapper::key_down(uint32_t platform_code) noexcept
{
	if(!initialized_ || is_reserved(platform_code)) return false;
	const int index = find_binding(platform_code);
	if(index < 0) return false;
	keyboard_buttons_ = static_cast<uint16_t>(keyboard_buttons_ | bindings_[index].button);
	return true;
}

bool NesInputMapper::key_up(uint32_t platform_code) noexcept
{
	if(!initialized_ || is_reserved(platform_code)) return false;
	const int index = find_binding(platform_code);
	if(index < 0) return false;
	keyboard_buttons_ = static_cast<uint16_t>(keyboard_buttons_ &
		static_cast<uint16_t>(~bindings_[index].button));
	return true;
}

void NesInputMapper::focus_lost() noexcept
{
	keyboard_buttons_ = 0;
}

uint16_t NesInputMapper::sample(NesControllerSample controller) const noexcept
{
	return static_cast<uint16_t>(keyboard_buttons_ |
		(controller.connected ? controller.buttons : 0) & RV_NES_BUTTON_ALL);
}

} // namespace revival::nes
