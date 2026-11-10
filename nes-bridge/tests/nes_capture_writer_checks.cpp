#include "NesCaptureWriter.hpp"

#include <array>
#include <filesystem>
#include <memory>
#include <stdexcept>

using revival::nes::NesCaptureWriter;
using revival::nes::NesTileObservation;
using revival::nes::NesCaptureSession;
using revival::nes::NesTileObserver;

void check(bool result)
{
	if(!result) throw std::runtime_error("NES capture writer check failed");
}

int main()
{
	const auto root = std::filesystem::current_path() / "revival-nes-writer-check";
	std::error_code ignored;
	std::filesystem::remove_all(root, ignored);
	std::array<uint8_t, 192> palette{};
	NesTileObservation observation{};
	observation.frame_sequence = 12;
	observation.tile_index = 7;
	observation.palette_colors = 0x00112233;
	NesCaptureWriter writer;
	check(!writer.start(root, "bad", "bad", palette));
	check(writer.start(root, std::string(64, 'a'), std::string(32, 'b'), palette));
	for(int index = 0; index < 129; ++index) {
		observation.frame_sequence = static_cast<uint64_t>(index);
		check(writer.append(observation));
	}
	check(writer.flush());
	writer.stop();
	auto stats = writer.stats();
	check(stats.written_records == 129);
	check(stats.chunks == 2);
	check(!stats.active);
	check(std::filesystem::is_regular_file(root / (std::string(32, 'b') + "-00000000.rnt")));
	check(std::filesystem::is_regular_file(root / (std::string(32, 'b') + "-00000001.rnt")));
	check(std::filesystem::file_size(root / (std::string(32, 'b') + "-00000000.rnt")) == 300 + 128 * 36);
	auto observer = std::make_unique<NesTileObserver>();
	NesCaptureSession capture(*observer);
	check(!observer->enabled());
	check(!capture.start(root, "invalid", std::string(32, 'c'), palette));
	check(!observer->enabled());
	check(capture.start(root, std::string(64, 'a'), std::string(32, 'c'), palette));
	observer->begin_frame(130);
	check(observer->submit(observation));
	capture.stop();
	check(!observer->enabled() && observer->stats().queued == 0);
	check(std::filesystem::file_size(root / (std::string(32, 'c') + "-00000000.rnt")) == 300 + 36);
	check(!std::filesystem::exists(root / (std::string(32, 'c') + "-00000000.part")));
	std::filesystem::remove_all(root, ignored);
	return 0;
}
