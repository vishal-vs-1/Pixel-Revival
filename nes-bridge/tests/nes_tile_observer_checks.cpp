#include "NesTileObserver.hpp"

#include <cassert>

using revival::nes::NesTileObservation;
using revival::nes::NesTileObserver;
using revival::nes::RV_NES_OBSERVER_MAX_FRAME_NEW;
using revival::nes::RV_NES_OBSERVER_MAX_QUEUE;

namespace {

NesTileObservation observation(uint64_t frame, int32_t index, bool sprite = false)
{
	NesTileObservation value{};
	value.frame_sequence = frame;
	value.tile_index = index;
	value.palette_colors = 0x00112233u + static_cast<uint32_t>(index);
	value.is_sprite = sprite ? 1 : 0;
	value.transparency_required = sprite ? 1 : 0;
	for(std::size_t i = 0; i < value.tile_data.size(); ++i) {
		value.tile_data[i] = static_cast<uint8_t>(index + static_cast<int32_t>(i));
	}
	return value;
}

} // namespace

int main()
{
	static NesTileObserver observer;
	assert(!observer.enabled());
	assert(!observer.submit(observation(1, 1)));

	observer.set_enabled(true);
	observer.begin_frame(1);
	assert(observer.submit(observation(1, 1)));
	assert(!observer.submit(observation(1, 1)));
	assert(!observer.submit(observation(2, 1)));
	assert(observer.stats().frames == 2);
	assert(observer.stats().unique == 1);
	NesTileObservation upgrade = observation(3, 1);
	upgrade.transparency_required = 1;
	assert(observer.submit(upgrade));

	NesTileObservation popped{};
	assert(observer.pop(popped));
	assert(popped.frame_sequence == 1 && popped.tile_index == 1);
	assert(observer.pop(popped));
	assert(popped.transparency_required == 1);
	assert(!observer.pop(popped));

	observer.reset();
	observer.set_enabled(true);
	for(uint64_t frame = 1; frame <= 64; ++frame) {
		observer.begin_frame(frame);
		for(uint32_t item = 0; item < RV_NES_OBSERVER_MAX_FRAME_NEW; ++item) {
			assert(observer.submit(observation(frame, static_cast<int32_t>((frame - 1) * RV_NES_OBSERVER_MAX_FRAME_NEW + item))));
		}
		assert(!observer.submit(observation(frame, 100000 + static_cast<int32_t>(frame))));
	}
	const auto full = observer.stats();
	assert(full.admitted == RV_NES_OBSERVER_MAX_QUEUE);
	assert(full.queued == RV_NES_OBSERVER_MAX_QUEUE);
	assert(full.dropped == 64);

	observer.reset();
	assert(observer.stats().queued == 0);
	assert(observer.stats().unique == 0);
	return 0;
}
