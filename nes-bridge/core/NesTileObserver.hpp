#pragma once

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace revival::nes {

constexpr std::size_t RV_NES_TILE_BYTES = 16;
constexpr std::size_t RV_NES_OBSERVER_MAX_FRAME_NEW = 64;
constexpr std::size_t RV_NES_OBSERVER_MAX_QUEUE = 4096;
constexpr std::size_t RV_NES_OBSERVER_MAX_UNIQUE = 65536;
constexpr std::size_t RV_NES_OBSERVER_MAX_PROBE = 256;
constexpr std::size_t RV_NES_OBSERVER_MAX_LOAD = RV_NES_OBSERVER_MAX_UNIQUE * 3 / 4;

struct NesTileObservation final {
	uint64_t frame_sequence = 0;
	int32_t tile_index = -1;
	uint32_t palette_colors = 0;
	std::array<uint8_t, RV_NES_TILE_BYTES> tile_data{};
	uint8_t is_chr_ram = 0;
	uint8_t is_sprite = 0;
	uint8_t transparency_required = 0;
	uint8_t reserved = 0;
};

struct NesTileObserverStats final {
	uint64_t frames = 0;
	uint64_t seen = 0;
	uint64_t unique = 0;
	uint64_t admitted = 0;
	uint64_t dropped = 0;
	uint32_t queued = 0;
};

class NesTileObserver final {
public:
	void set_enabled(bool enabled) noexcept;
	bool enabled() const noexcept;
	void reset() noexcept;

	void begin_frame(uint64_t frame_sequence) noexcept;
	bool submit(const NesTileObservation& observation) noexcept;
	bool pop(NesTileObservation& observation) noexcept;
	NesTileObserverStats stats() const noexcept;

private:
	struct UniqueSlot final {
		uint64_t hash = 0;
		NesTileObservation observation{};
		bool occupied = false;
	};

	static uint64_t hash_observation(const NesTileObservation& observation) noexcept;
	static bool same_key(const NesTileObservation& left, const NesTileObservation& right) noexcept;
	bool find_key(const NesTileObservation& observation, uint64_t hash,
		UniqueSlot** match) noexcept;
	bool insert_key(const NesTileObservation& observation, uint64_t hash) noexcept;

	std::atomic<bool> enabled_{false};
	std::array<NesTileObservation, RV_NES_OBSERVER_MAX_QUEUE> queue_{};
	std::array<UniqueSlot, RV_NES_OBSERVER_MAX_UNIQUE> unique_{};
	std::atomic<uint32_t> write_index_{0};
	std::atomic<uint32_t> read_index_{0};
	uint64_t current_frame_ = 0;
	uint32_t frame_new_count_ = 0;
	std::atomic<uint64_t> frames_{0};
	std::atomic<uint64_t> seen_{0};
	std::atomic<uint64_t> unique_count_{0};
	std::atomic<uint64_t> admitted_{0};
	std::atomic<uint64_t> dropped_{0};
};

NesTileObserver& global_nes_tile_observer() noexcept;

} // namespace revival::nes

#if defined(_WIN32)
#define RV_NES_OBSERVER_API __declspec(dllexport)
#define RV_NES_OBSERVER_CALL __stdcall
#else
#define RV_NES_OBSERVER_API
#define RV_NES_OBSERVER_CALL
#endif

extern "C" {
RV_NES_OBSERVER_API void RV_NES_OBSERVER_CALL SetRevivalNesObserver(bool enabled);
RV_NES_OBSERVER_API void RV_NES_OBSERVER_CALL ResetRevivalNesObserver();
RV_NES_OBSERVER_API void RV_NES_OBSERVER_CALL GetRevivalNesObserverStats(
	revival::nes::NesTileObserverStats* stats);
RV_NES_OBSERVER_API bool RV_NES_OBSERVER_CALL PopRevivalNesObserverRecord(
	revival::nes::NesTileObservation* observation);
}
