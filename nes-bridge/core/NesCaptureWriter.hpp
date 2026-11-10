#pragma once

#include "NesTileObserver.hpp"

#include <array>
#include <cstdint>
#include <filesystem>
#include <string>
#include <thread>
#include <vector>

namespace revival::nes {

constexpr std::size_t RV_NES_CAPTURE_PALETTE_BYTES = 192;
constexpr std::size_t RV_NES_CAPTURE_MAX_RECORDS = 128;
constexpr std::size_t RV_NES_CAPTURE_MAX_CHUNKS = 512;
constexpr std::size_t RV_NES_CAPTURE_HEADER_BYTES = 300;
constexpr std::size_t RV_NES_CAPTURE_RECORD_BYTES = 36;

struct NesCaptureWriterStats final {
	uint64_t written_records = 0;
	uint64_t dropped_records = 0;
	uint64_t written_bytes = 0;
	uint32_t chunks = 0;
	uint16_t buffered = 0;
	bool active = false;
	bool paused = false;
};

class NesCaptureWriter final {
public:
	bool start(const std::filesystem::path& directory, const std::string& game_id,
		const std::string& session_id,
		const std::array<uint8_t, RV_NES_CAPTURE_PALETTE_BYTES>& palette);
	bool append(const NesTileObservation& observation);
	bool flush();
	void stop();
	NesCaptureWriterStats stats() const noexcept;

private:
	bool write_chunk();
	static bool valid_hex(const std::string& value, std::size_t length) noexcept;

	std::filesystem::path directory_;
	std::string game_id_;
	std::string session_id_;
	std::array<uint8_t, RV_NES_CAPTURE_PALETTE_BYTES> palette_{};
	std::vector<NesTileObservation> records_;
	uint32_t next_sequence_ = 0;
	uint64_t written_records_ = 0;
	uint64_t dropped_records_ = 0;
	uint64_t written_bytes_ = 0;
	uint32_t chunks_ = 0;
	bool active_ = false;
	bool paused_ = false;
};

class NesCaptureSession final {
public:
	explicit NesCaptureSession(NesTileObserver& observer) noexcept : observer_(observer) {}
	~NesCaptureSession();
	bool start(const std::filesystem::path& directory, const std::string& game_id,
		const std::string& session_id,
		const std::array<uint8_t, RV_NES_CAPTURE_PALETTE_BYTES>& palette);
	// Stop after the emulation thread has joined so the final drain owns the tail.
	void stop();

private:
	void run() noexcept;
	bool drain();

	NesTileObserver& observer_;
	NesCaptureWriter writer_;
	std::atomic<bool> stopping_{false};
	std::thread worker_;
};

} // namespace revival::nes
