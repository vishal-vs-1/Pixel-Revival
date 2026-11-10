#pragma once

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>

namespace revival::nes {

// The two sample buffers have independent writers and are read only after both
// Mesen threads stop. A full buffer stops taking samples without blocking.
class NesRendererProbe final {
public:
	using Clock = std::chrono::steady_clock;
	using Tick = Clock::time_point;

	bool enabled() const noexcept { return enabled_; }
	Tick tick() const noexcept { return enabled_ ? Clock::now() : Tick{}; }

	void submit(Tick started, uint64_t bytes, uint32_t center_pixel) noexcept
	{
		submit_.add(started, bytes, center_pixel);
	}

	void present(Tick started) noexcept
	{
		present_.add(started, 0);
	}

	void pump(Tick started, uint64_t bytes, uint32_t frames) noexcept { pump_.add(started, bytes, frames); }
	void draw(Tick started) noexcept { draw_.add(started, 0); }
	void swap(Tick started) noexcept { swap_.add(started, 0); }
	void stock_upload(uint64_t bytes, bool frame) noexcept
	{
		++stock_uploads_;
		stock_upload_bytes_ += bytes;
		if(frame) ++stock_frame_uploads_;
	}

	void report() const noexcept
	{
		if(!enabled_) return;
		submit_.report("submit");
		present_.report("present");
		pump_.report("pump");
		draw_.report("draw");
		swap_.report("swap");
		std::fprintf(stderr, "[revival-upload-check] %llu %llu %llu\n",
			static_cast<unsigned long long>(stock_uploads_),
			static_cast<unsigned long long>(stock_upload_bytes_),
			static_cast<unsigned long long>(stock_frame_uploads_));
	}

private:
	struct Samples {
		struct Entry {
			uint64_t timestamp = 0;
			uint64_t elapsed = 0;
			uint64_t bytes = 0;
			uint32_t center_pixel = 0;
		};
		std::array<Entry, 1024> entries{};
		std::size_t count = 0;

		void add(Tick started, uint64_t bytes, uint32_t center_pixel = 0) noexcept
		{
			if(count == entries.size()) return;
			using Nanoseconds = std::chrono::nanoseconds;
			const auto ended = Clock::now();
			entries[count++] = {
				static_cast<uint64_t>(std::chrono::duration_cast<Nanoseconds>(started.time_since_epoch()).count()),
				static_cast<uint64_t>(std::chrono::duration_cast<Nanoseconds>(ended - started).count()),
				bytes, center_pixel
			};
		}

		void report(const char* name) const noexcept
		{
			for(std::size_t i = 0; i < count; ++i) {
				const auto& entry = entries[i];
				std::fprintf(stderr, "[revival-render-check] %s %llu %llu %llu %u\n",
					name, static_cast<unsigned long long>(entry.timestamp),
					static_cast<unsigned long long>(entry.elapsed),
					static_cast<unsigned long long>(entry.bytes), entry.center_pixel);
			}
		}
	};

	static bool requested() noexcept
	{
		char value[2]{};
		std::size_t size = 0;
		return getenv_s(&size, value, sizeof(value), "J2ME_REVIVAL_MESEN_RENDER_CHECK") == 0 &&
			size == sizeof(value) && value[0] == '1';
	}

	const bool enabled_ = requested();
	Samples submit_;
	Samples present_;
	Samples pump_;
	Samples draw_;
	Samples swap_;
	uint64_t stock_uploads_ = 0;
	uint64_t stock_upload_bytes_ = 0;
	uint64_t stock_frame_uploads_ = 0;
};

inline NesRendererProbe& renderer_probe() noexcept
{
	static NesRendererProbe probe;
	return probe;
}

} // namespace revival::nes
