#include "NesCaptureWriter.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <fstream>
#include <system_error>

namespace revival::nes {
namespace {

template <typename Integer>
void put_le(std::vector<uint8_t>& output, std::size_t offset, Integer value) noexcept
{
	for(std::size_t byte = 0; byte < sizeof(Integer); ++byte) {
		output[offset + byte] = static_cast<uint8_t>(value >> (byte * 8));
	}
}

std::string sequence_name(uint32_t sequence)
{
	constexpr char digits[] = "0123456789abcdef";
	std::string result(8, '0');
	for(std::size_t index = 0; index < result.size(); ++index) {
		const std::size_t shift = (result.size() - index - 1) * 4;
		result[index] = digits[(sequence >> shift) & 0x0F];
	}
	return result;
}

} // namespace

bool NesCaptureWriter::valid_hex(const std::string& value, std::size_t length) noexcept
{
	if(value.size() != length) return false;
	return std::all_of(value.begin(), value.end(), [](char value) {
		return (value >= '0' && value <= '9') ||
			(value >= 'a' && value <= 'f');
	});
}

bool NesCaptureWriter::start(const std::filesystem::path& directory,
	const std::string& game_id, const std::string& session_id,
	const std::array<uint8_t, RV_NES_CAPTURE_PALETTE_BYTES>& palette)
{
	if(active_ || !valid_hex(game_id, 64) || !valid_hex(session_id, 32)) return false;
	std::error_code error;
	std::filesystem::create_directories(directory, error);
	if(error) return false;
	directory_ = directory;
	game_id_ = game_id;
	session_id_ = session_id;
	palette_ = palette;
	records_.clear();
	records_.reserve(RV_NES_CAPTURE_MAX_RECORDS);
	next_sequence_ = 0;
	written_records_ = 0;
	dropped_records_ = 0;
	written_bytes_ = 0;
	chunks_ = 0;
	paused_ = false;
	active_ = true;
	return true;
}

bool NesCaptureWriter::append(const NesTileObservation& observation)
{
	if(!active_ || paused_) {
		++dropped_records_;
		return false;
	}
	if(records_.size() >= RV_NES_CAPTURE_MAX_RECORDS && !write_chunk()) {
		++dropped_records_;
		return false;
	}
	records_.push_back(observation);
	return true;
}

bool NesCaptureWriter::write_chunk()
{
	if(records_.empty()) return true;
	if(next_sequence_ >= RV_NES_CAPTURE_MAX_CHUNKS) {
		paused_ = true;
		dropped_records_ += records_.size();
		records_.clear();
		return false;
	}
	std::vector<uint8_t> bytes(RV_NES_CAPTURE_HEADER_BYTES +
		records_.size() * RV_NES_CAPTURE_RECORD_BYTES, 0);
	bytes[0] = 'R'; bytes[1] = 'N'; bytes[2] = 'T'; bytes[3] = '1';
	std::copy(game_id_.begin(), game_id_.end(), bytes.begin() + 4);
	std::copy(session_id_.begin(), session_id_.end(), bytes.begin() + 68);
	put_le<uint32_t>(bytes, 100, next_sequence_);
	put_le<uint16_t>(bytes, 104, static_cast<uint16_t>(records_.size()));
	std::copy(palette_.begin(), palette_.end(), bytes.begin() + 108);
	for(std::size_t index = 0; index < records_.size(); ++index) {
		const auto& record = records_[index];
		const std::size_t offset = RV_NES_CAPTURE_HEADER_BYTES +
			index * RV_NES_CAPTURE_RECORD_BYTES;
		put_le<uint64_t>(bytes, offset, record.frame_sequence);
		put_le<int32_t>(bytes, offset + 8, record.tile_index);
		put_le<uint32_t>(bytes, offset + 12, record.palette_colors);
		std::copy(record.tile_data.begin(), record.tile_data.end(), bytes.begin() + offset + 16);
		bytes[offset + 32] = record.is_chr_ram;
		bytes[offset + 33] = record.is_sprite;
		bytes[offset + 34] = record.transparency_required;
		bytes[offset + 35] = record.reserved;
	}
	const std::string stem = session_id_ + "-" + sequence_name(next_sequence_);
	const auto temporary = directory_ / (stem + ".part");
	const auto destination = directory_ / (stem + ".rnt");
	if(std::filesystem::exists(std::filesystem::symlink_status(temporary)) ||
		std::filesystem::exists(std::filesystem::symlink_status(destination))) {
		paused_ = true;
		dropped_records_ += records_.size();
		records_.clear();
		return false;
	}
	{
		std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
		output.write(reinterpret_cast<const char*>(bytes.data()),
			static_cast<std::streamsize>(bytes.size()));
		output.flush();
		if(!output.good()) {
			output.close();
			std::error_code ignored;
			std::filesystem::remove(temporary, ignored);
			paused_ = true;
			dropped_records_ += records_.size();
			records_.clear();
			return false;
		}
	}
	std::error_code error;
	std::filesystem::rename(temporary, destination, error);
	if(error) {
		std::error_code ignored;
		std::filesystem::remove(temporary, ignored);
		paused_ = true;
		dropped_records_ += records_.size();
		records_.clear();
		return false;
	}
	written_bytes_ += bytes.size();
	written_records_ += records_.size();
	++chunks_;
	++next_sequence_;
	records_.clear();
	return true;
}

bool NesCaptureWriter::flush()
{
	if(!active_ || paused_) return false;
	return write_chunk();
}

void NesCaptureWriter::stop()
{
	if(!active_) return;
	flush();
	active_ = false;
}

NesCaptureWriterStats NesCaptureWriter::stats() const noexcept
{
	return {written_records_, dropped_records_, written_bytes_, chunks_,
		static_cast<uint16_t>(records_.size()), active_, paused_};
}

NesCaptureSession::~NesCaptureSession()
{
	stop();
}

bool NesCaptureSession::start(const std::filesystem::path& directory,
	const std::string& game_id, const std::string& session_id,
	const std::array<uint8_t, RV_NES_CAPTURE_PALETTE_BYTES>& palette)
{
	if(worker_.joinable() || observer_.enabled()) return false;
	if(!writer_.start(directory, game_id, session_id, palette)) return false;
	stopping_.store(false);
	try {
		worker_ = std::thread([this] { run(); });
	} catch(...) {
		writer_.stop();
		throw;
	}
	observer_.set_enabled(true);
	return true;
}

void NesCaptureSession::stop()
{
	if(!worker_.joinable()) return;
	observer_.set_enabled(false);
	stopping_.store(true);
	worker_.join();
}

bool NesCaptureSession::drain()
{
	NesTileObservation record;
	for(std::size_t count = 0; count < RV_NES_OBSERVER_MAX_QUEUE; ++count) {
		if(!observer_.pop(record)) break;
		if(!writer_.append(record)) return false;
	}
	return true;
}

void NesCaptureSession::run() noexcept
{
	using namespace std::chrono_literals;
	auto next_flush = std::chrono::steady_clock::now() + 1s;
	bool healthy = true;
	try {
		while(!stopping_.load() && healthy) {
			healthy = drain();
			const auto now = std::chrono::steady_clock::now();
			if(healthy && now >= next_flush) {
				healthy = writer_.flush();
				next_flush = now + 1s;
			}
			if(healthy) std::this_thread::sleep_for(50ms);
		}
		if(healthy) healthy = drain() && writer_.flush();
	} catch(...) {
		healthy = false;
	}
	if(!healthy) observer_.set_enabled(false);
	const auto stats = writer_.stats();
	std::fprintf(stderr, "[revival-capture] %s chunks=%u records=%llu\n",
		healthy ? "finished" : "stopped after writer failure or limit", stats.chunks,
		static_cast<unsigned long long>(stats.written_records));
}

} // namespace revival::nes
