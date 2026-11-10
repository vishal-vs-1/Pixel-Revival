#include "NesInputMapper.hpp"

#include <cassert>

using revival::nes::NesControllerSample;
using revival::nes::NesInputMapper;
using revival::nes::NesKeyBinding;
using revival::nes::NesPlatformInputCodes;
using revival::nes::RV_NES_BUTTON_A;
using revival::nes::RV_NES_BUTTON_ALL;
using revival::nes::RV_NES_BUTTON_B;
using revival::nes::RV_NES_BUTTON_DOWN;
using revival::nes::RV_NES_BUTTON_RIGHT;
using revival::nes::RV_NES_BUTTON_SELECT;
using revival::nes::RV_NES_BUTTON_START;
using revival::nes::RV_NES_BUTTON_LEFT;
using revival::nes::RV_NES_BUTTON_UP;

int main()
{
	NesInputMapper mapper;
	assert(!mapper.initialize({0, 0}));
	assert(mapper.initialize({27, 122}));
	assert(mapper.initialized());
	const NesKeyBinding bindings[] = {{38, RV_NES_BUTTON_UP}, {40, RV_NES_BUTTON_DOWN},
		{37, RV_NES_BUTTON_LEFT}, {39, RV_NES_BUTTON_RIGHT}, {88, RV_NES_BUTTON_A},
		{90, RV_NES_BUTTON_B}, {13, RV_NES_BUTTON_START}, {32, RV_NES_BUTTON_SELECT}};
	assert(mapper.configure(bindings, 8));
	assert(!mapper.key_down(27));
	assert(mapper.key_down(38));
	assert(mapper.key_down(88));
	assert(mapper.sample() == (RV_NES_BUTTON_UP | RV_NES_BUTTON_A));
	assert(mapper.key_up(38));
	assert(mapper.sample() == RV_NES_BUTTON_A);
	assert(mapper.sample({RV_NES_BUTTON_RIGHT, true}) ==
		(RV_NES_BUTTON_A | RV_NES_BUTTON_RIGHT));
	assert(mapper.sample({RV_NES_BUTTON_ALL, false}) == RV_NES_BUTTON_A);
	mapper.focus_lost();
	assert(mapper.sample() == 0);
	const NesKeyBinding reserved[] = {{122, RV_NES_BUTTON_UP}, {40, RV_NES_BUTTON_DOWN},
		{37, RV_NES_BUTTON_LEFT}, {39, RV_NES_BUTTON_RIGHT}, {88, RV_NES_BUTTON_A},
		{90, RV_NES_BUTTON_B}, {13, RV_NES_BUTTON_START}, {32, RV_NES_BUTTON_SELECT}};
	assert(!mapper.configure(reserved, 8));
	const NesKeyBinding duplicate[] = {{38, RV_NES_BUTTON_UP}, {38, RV_NES_BUTTON_RIGHT},
		{37, RV_NES_BUTTON_LEFT}, {40, RV_NES_BUTTON_DOWN}, {88, RV_NES_BUTTON_A},
		{90, RV_NES_BUTTON_B}, {13, RV_NES_BUTTON_START}, {32, RV_NES_BUTTON_SELECT}};
	assert(!mapper.configure(duplicate, 8));
	assert(mapper.sample() == 0);
	return 0;
}
