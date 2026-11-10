#include "NesTileObserver.hpp"

#include <cstring>

namespace revival::nes {
namespace {

constexpr uint32_t kQueueMask = static_cast<uint32_t>(RV_NES_OBSERVER_MAX_QUEUE - 1);
constexpr uint32_t kUniqueMask = static_cast<uint32_t>(RV_NES_OBSERVER_MAX_UNIQUE - 1);

static_assert((RV_NES_OBSERVER_MAX_QUEUE & kQueueMask) == 0);
static_assert((RV_NES_OBSERVER_MAX_UNIQUE & kUniqueMask) == 0);

uint64_t mix(uint64_t value) noexcept
{
	value ^= value >> 30;
	value *= 0xbf58476d1ce4e5b9ULL;
	value ^= value >> 27;
	value *= 0x94d049bb133111ebULL;
	return value ^ (value >> 31);
}

} // namespace

void NesTileObserver::set_enabled(bool enabled) noexcept
{
	enabled_.store(enabled, std::memory_order_release);
}

bool NesTileObserver::enabled() const noexcept
{
	return enabled_.load(std::memory_order_acquire);
}

void NesTileObserver::reset() noexcept
{
	enabled_.store(false, std::memory_order_release);
	write_index_.store(0, std::memory_order_release);
	read_index_.store(0, std::memory_order_release);
	current_frame_ = 0;
	frame_new_count_ = 0;
	for(auto& slot : unique_) {
		slot = {};
	}
	frames_.store(0, std::memory_order_release);
	seen_.store(0, std::memory_order_release);
	unique_count_.store(0, std::memory_order_release);
	admitted_.store(0, std::memory_order_release);
	dropped_.store(0, std::memory_order_release);
}

void NesTileObserver::begin_frame(uint64_t frame_sequence) noexcept
{
	if(!enabled()) return;
	if(current_frame_ != frame_sequence) {
		current_frame_ = frame_sequence;
		frame_new_count_ = 0;
		frames_.fetch_add(1, std::memory_order_relaxed);
	}
}

uint64_t NesTileObserver::hash_observation(const NesTileObservation& observation) noexcept
{
	uint64_t hash = 1469598103934665603ULL;
	auto add = [&hash](uint8_t value) {
		hash ^= value;
		hash *= 1099511628211ULL;
	};
	for(const uint8_t* bytes = reinterpret_cast<const uint8_t*>(&observation.tile_index);
		bytes != reinterpret_cast<const uint8_t*>(&observation.tile_index) + sizeof(observation.tile_index);
		++bytes) add(*bytes);
	for(const uint8_t* bytes = reinterpret_cast<const uint8_t*>(&observation.palette_colors);
		bytes != reinterpret_cast<const uint8_t*>(&observation.palette_colors) + sizeof(observation.palette_colors);
		++bytes) add(*bytes);
	for(uint8_t byte : observation.tile_data) add(byte);
	add(observation.is_chr_ram);
	add(observation.is_sprite);
	return mix(hash);
}

bool NesTileObserver::same_key(const NesTileObservation& left, const NesTileObservation& right) noexcept
{
	return left.tile_index == right.tile_index &&
		left.palette_colors == right.palette_colors &&
		left.tile_data == right.tile_data &&
		left.is_chr_ram == right.is_chr_ram &&
		left.is_sprite == right.is_sprite;
}

bool NesTileObserver::find_key(const NesTileObservation& observation, uint64_t hash,
	UniqueSlot** match) noexcept
{
	uint32_t index = static_cast<uint32_t>(hash) & kUniqueMask;
	for(std::size_t probe = 0; probe < RV_NES_OBSERVER_MAX_PROBE; ++probe) {
		UniqueSlot& slot = unique_[index];
		if(!slot.occupied) return false;
		if(slot.hash == hash && same_key(slot.observation, observation)) {
			if(match != nullptr) *match = &slot;
			return true;
		}
		index = (index + 1) & kUniqueMask;
	}
	return false;
}

bool NesTileObserver::insert_key(const NesTileObservation& observation, uint64_t hash) noexcept
{
	uint32_t index = static_cast<uint32_t>(hash) & kUniqueMask;
	for(std::size_t probe = 0; probe < RV_NES_OBSERVER_MAX_PROBE; ++probe) {
		UniqueSlot& slot = unique_[index];
		if(!slot.occupied) {
			slot.hash = hash;
			slot.observation = observation;
			slot.occupied = true;
			return true;
		}
		if(slot.hash == hash && same_key(slot.observation, observation)) return false;
		index = (index + 1) & kUniqueMask;
	}
	return false;
}

bool NesTileObserver::submit(const NesTileObservation& observation) noexcept
{
	if(!enabled()) return false;
	begin_frame(observation.frame_sequence);
	seen_.fetch_add(1, std::memory_order_relaxed);
	const uint64_t hash = hash_observation(observation);
	UniqueSlot* existing = nullptr;
	if(find_key(observation, hash, &existing)) {
		if(observation.transparency_required && !existing->observation.transparency_required) {
			existing->observation.transparency_required = 1;
			const uint32_t write = write_index_.load(std::memory_order_relaxed);
			const uint32_t read = read_index_.load(std::memory_order_acquire);
			if(write - read >= RV_NES_OBSERVER_MAX_QUEUE) {
				dropped_.fetch_add(1, std::memory_order_relaxed);
				return false;
			}
			queue_[write & kQueueMask] = observation;
			write_index_.store(write + 1, std::memory_order_release);
			admitted_.fetch_add(1, std::memory_order_relaxed);
			return true;
		}
		return false;
	}
	if(frame_new_count_ >= RV_NES_OBSERVER_MAX_FRAME_NEW) {
		dropped_.fetch_add(1, std::memory_order_relaxed);
		return false;
	}
	if(unique_count_.load(std::memory_order_relaxed) >= RV_NES_OBSERVER_MAX_LOAD) {
		dropped_.fetch_add(1, std::memory_order_relaxed);
		return false;
	}
	const uint32_t write = write_index_.load(std::memory_order_relaxed);
	const uint32_t read = read_index_.load(std::memory_order_acquire);
	if(write - read >= RV_NES_OBSERVER_MAX_QUEUE) {
		dropped_.fetch_add(1, std::memory_order_relaxed);
		return false;
	}
	if(!insert_key(observation, hash)) {
		dropped_.fetch_add(1, std::memory_order_relaxed);
		return false;
	}
	queue_[write & kQueueMask] = observation;
	write_index_.store(write + 1, std::memory_order_release);
	frame_new_count_++;
	unique_count_.fetch_add(1, std::memory_order_relaxed);
	admitted_.fetch_add(1, std::memory_order_relaxed);
	return true;
}

bool NesTileObserver::pop(NesTileObservation& observation) noexcept
{
	const uint32_t read = read_index_.load(std::memory_order_relaxed);
	const uint32_t write = write_index_.load(std::memory_order_acquire);
	if(read == write) return false;
	observation = queue_[read & kQueueMask];
	read_index_.store(read + 1, std::memory_order_release);
	return true;
}

NesTileObserverStats NesTileObserver::stats() const noexcept
{
	const uint32_t read = read_index_.load(std::memory_order_acquire);
	const uint32_t write = write_index_.load(std::memory_order_acquire);
	return {
		frames_.load(std::memory_order_relaxed),
		seen_.load(std::memory_order_relaxed),
		unique_count_.load(std::memory_order_relaxed),
		admitted_.load(std::memory_order_relaxed),
		dropped_.load(std::memory_order_relaxed),
		write - read,
	};
}

NesTileObserver& global_nes_tile_observer() noexcept
{
	static NesTileObserver observer;
	return observer;
}

} // namespace revival::nes

extern "C" {

void RV_NES_OBSERVER_CALL SetRevivalNesObserver(bool enabled)
{
	auto& observer = revival::nes::global_nes_tile_observer();
	// A new opt-in session must not inherit records or deduplication state from
	// a previous frontend lifetime; repeated enables keep the active session.
	if(enabled && !observer.enabled()) observer.reset();
	observer.set_enabled(enabled);
}

void RV_NES_OBSERVER_CALL ResetRevivalNesObserver()
{
	revival::nes::global_nes_tile_observer().reset();
}

void RV_NES_OBSERVER_CALL GetRevivalNesObserverStats(revival::nes::NesTileObserverStats* stats)
{
	if(stats != nullptr) *stats = revival::nes::global_nes_tile_observer().stats();
}

bool RV_NES_OBSERVER_CALL PopRevivalNesObserverRecord(revival::nes::NesTileObservation* observation)
{
	return observation != nullptr && revival::nes::global_nes_tile_observer().pop(*observation);
}

}
