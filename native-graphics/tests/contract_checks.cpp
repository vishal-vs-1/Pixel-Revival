#include <array>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

#include "command_queue.hpp"
#include "command_protocol.hpp"
#include "context.hpp"
#include "completed_frame.hpp"
#include "frame_presenter.hpp"
#include "gl_functions.hpp"
#include "pixel_transfer.hpp"
#include "scale2x.hpp"
#include "surface_ops.hpp"
#include "surface_protocol.hpp"
#include "texture_store.hpp"

namespace {

int failures = 0;

void check(bool condition, const char* message)
{
	if(!condition) {
		std::cerr << "FAIL: " << message << '\n';
		++failures;
	}
}

std::array<uint8_t, 76> valid_packet()
{
	std::array<uint8_t, 76> bytes{};
	bytes[0] = 'R'; bytes[1] = 'V'; bytes[2] = 'G'; bytes[3] = '1';
	bytes[4] = 1; bytes[6] = 32;
	bytes[8] = 76; bytes[12] = 1; bytes[16] = 7; bytes[24] = 9;
	bytes[32] = static_cast<uint8_t>(RV_OP_COMMIT_FRAME); bytes[36] = 44;
	bytes[40] = 1; bytes[48] = 1; bytes[56] = static_cast<uint8_t>(RV_SCALE_2X);
	bytes[68] = 1; bytes[72] = 1;
	return bytes;
}

RvAttachConfig config_for(uint64_t epoch, uint32_t mode = RV_INPUT_MODE_COMMANDS)
{
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = epoch;
	config.texture_budget_bytes = 128ull * 1024 * 1024;
	config.max_queued_bytes = 1024;
	config.max_queued_ops = 8;
	config.max_pending_frames = 1;
	config.detail_scale = RV_SCALE_2X;
	config.input_mode = mode;
	return config;
}

RvFrameDesc valid_frame()
{
	RvFrameDesc desc{};
	desc.struct_bytes = RV_FRAME_DESC_BYTES;
	desc.abi_version = RV_ABI_VERSION;
	desc.frame_sequence = 1;
	desc.physical_width = 2;
	desc.physical_height = 2;
	desc.physical_stride_bytes = 8;
	desc.logical_width = 2;
	desc.logical_height = 2;
	desc.source_scale = RV_SCALE_ORIGINAL;
	desc.desired_scale = RV_SCALE_2X;
	desc.pixel_format = RV_PIXEL_FORMAT_RGBA8;
	desc.alpha_mode = RV_ALPHA_STRAIGHT;
	desc.origin = RV_ORIGIN_TOP_LEFT;
	return desc;
}

void test_packet_queue()
{
	using namespace revival::graphics;
	OwnedCommandQueue queue(QueueLimits{76, 1, 1}, 7);
	auto packet = valid_packet();
	auto invalid = packet;
	invalid[34] = 1;
	check(queue.submit_packet(invalid.data(), static_cast<uint32_t>(invalid.size())) ==
		RV_RESULT_BAD_PACKET, "reserved packet flags must be rejected");
	QueueSnapshot snapshot = queue.snapshot();
	check(snapshot.queued_bytes == 0 && snapshot.queued_operations == 0 &&
		snapshot.pending_frames == 0, "invalid packet must not partially change queue usage");

	ValidatedPacket parsed{};
	check(validate_packet(packet.data(), static_cast<uint32_t>(packet.size()), 8, &parsed) ==
		RV_RESULT_STALE_HANDLE, "packet epoch mismatch must be rejected");
	check(queue.submit_packet(packet.data(), static_cast<uint32_t>(packet.size())) == RV_RESULT_OK,
		"valid packet must be admitted");
	packet[32] = 0;
	auto next_packet = valid_packet();
	next_packet[24] = 10;
	check(queue.submit_packet(next_packet.data(), static_cast<uint32_t>(next_packet.size())) == RV_RESULT_QUEUE_FULL,
		"queue must enforce byte, operation and committed-frame caps");
	snapshot = queue.snapshot();
	check(snapshot.queued_bytes == 76 && snapshot.queued_operations == 1 &&
		snapshot.pending_frames == 1 && snapshot.packets_submitted == 1,
		"queue counters must reflect one owned packet");
	QueuedItem item{};
	bool found = false;
	check(queue.try_pop(&item, &found) == RV_RESULT_OK && found,
		"admitted packet must be readable");
	check(item.bytes.size() == 76 && item.bytes[32] == 9,
		"producer mutation after submit must not change the owned packet");
	snapshot = queue.snapshot();
	check(snapshot.queued_bytes == 0 && snapshot.inflight_bytes == 76 &&
		snapshot.inflight_operations == 1 && snapshot.inflight_frames == 1,
		"consumer-owned packet memory stays charged against byte/op/frame caps");
	check(queue.try_pop(&item, &found) == RV_RESULT_OK && !found,
		"empty queue must report no item without a capacity error");
	check(queue.release_inflight(76, 1, 1) == RV_RESULT_OK &&
		queue.submit_packet(next_packet.data(), static_cast<uint32_t>(next_packet.size())) == RV_RESULT_OK,
		"released packet capacity can be reused while packet sequence still increases");
	check(queue.submit_packet(next_packet.data(), static_cast<uint32_t>(next_packet.size())) ==
		RV_RESULT_BAD_PACKET, "packet sequence must strictly increase for each accepted packet");
}

void test_frame_queue_and_close()
{
	using namespace revival::graphics;
	OwnedCommandQueue queue(QueueLimits{16, 1, 1}, 3);
	RvFrameDesc desc = valid_frame();
	std::array<uint8_t, 16> pixels{};
	pixels[0] = 42;
	check(queue.submit_completed_frame(&desc, static_cast<uint32_t>(sizeof(desc)), pixels.data(),
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_OK, "valid completed frame must be admitted");
	pixels[0] = 99;
	check(queue.submit_completed_frame(&desc, static_cast<uint32_t>(sizeof(desc)), pixels.data(),
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_QUEUE_FULL, "pending-frame cap must be enforced");
	QueuedItem item{};
	bool found = false;
	check(queue.try_pop(&item, &found) == RV_RESULT_OK && found && item.bytes[0] == 42,
		"completed frame must own its copied pixels");
	check(queue.release_inflight(16, 0, 1) == RV_RESULT_OK,
		"frame memory reservation releases after its consumer finishes processing");
	RvFrameDesc invalid = desc;
	invalid.physical_height = 0;
	check(queue.submit_completed_frame(&invalid, static_cast<uint32_t>(sizeof(invalid)), pixels.data(),
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_INVALID_ARGUMENT, "invalid frame dimensions must be rejected");
	check(queue.submit_completed_frame(&desc, static_cast<uint32_t>(sizeof(desc)), nullptr,
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_INVALID_ARGUMENT, "null frame pixels must be rejected safely");

	check(queue.submit_completed_frame(&desc, static_cast<uint32_t>(sizeof(desc)), pixels.data(),
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_OK, "frame can be queued before close");
	queue.begin_close();
	check(queue.is_closing(), "close state must be visible");
	check(queue.snapshot().queued_bytes == 0 && queue.snapshot().pending_frames == 0,
		"close must release queued frame memory and reservations");
	check(queue.submit_completed_frame(&desc, static_cast<uint32_t>(sizeof(desc)), pixels.data(),
		static_cast<uint32_t>(pixels.size())) ==
		RV_RESULT_CLOSING, "closing queue must reject a new frame");
}

void test_close_race()
{
	using namespace revival::graphics;
	OwnedCommandQueue queue(QueueLimits{RV_MAX_QUEUED_BYTES, RV_MAX_QUEUED_OPS,
		RV_MAX_PENDING_FRAMES}, 7);
	const auto packet = valid_packet();
	std::atomic<bool> started{false};
	std::thread first([&]() {
		started.store(true, std::memory_order_release);
		for(int index = 0; index < 2000; ++index) {
			queue.submit_packet(packet.data(), static_cast<uint32_t>(packet.size()));
		}
	});
	std::thread second([&]() {
		started.store(true, std::memory_order_release);
		for(int index = 0; index < 2000; ++index) {
			queue.submit_packet(packet.data(), static_cast<uint32_t>(packet.size()));
		}
	});
	while(!started.load(std::memory_order_acquire)) {
		std::this_thread::yield();
	}
	std::this_thread::sleep_for(std::chrono::milliseconds(1));
	queue.begin_close();
	first.join();
	second.join();
	check(queue.is_closing(), "concurrent close must finish");
	check(queue.snapshot().queued_bytes == 0 && queue.snapshot().queued_operations == 0,
		"concurrent close must leave no queued payload or operation reservations");
	check(queue.submit_packet(packet.data(), static_cast<uint32_t>(packet.size())) ==
		RV_RESULT_CLOSING, "post-close producer must return promptly with closing status");
}

struct ResolverState {
	void* wgl_address = nullptr;
	bool missing_module_address = false;
	bool use_ext_framebuffer_aliases = false;
	uint32_t module_lookups = 0;
	int token = 0;
};

void* fake_wgl_lookup(void* opaque, const char* name)
{
	auto* state = static_cast<ResolverState*>(opaque);
	if(state->use_ext_framebuffer_aliases && std::strncmp(name, "gl", 2) == 0) {
		if(std::strcmp(name, "glGenFramebuffers") == 0 ||
			std::strcmp(name, "glBindFramebuffer") == 0 ||
			std::strcmp(name, "glFramebufferTexture2D") == 0 ||
			std::strcmp(name, "glCheckFramebufferStatus") == 0 ||
			std::strcmp(name, "glDeleteFramebuffers") == 0) {
			return nullptr;
		}
		if(std::strstr(name, "Framebuffer") != nullptr) {
			return &state->token;
		}
	}
	return state->wgl_address;
}

void* fake_module_lookup(void* opaque, const char* name)
{
	auto* state = static_cast<ResolverState*>(opaque);
	++state->module_lookups;
	if(state->missing_module_address && std::strcmp(name, "glBindTexture") == 0) {
		return nullptr;
	}
	return &state->token;
}

void test_gl_loader()
{
	using namespace revival::graphics;
	ResolverState direct_state{};
	direct_state.wgl_address = &direct_state.token;
	ProcResolver direct_resolver{&direct_state, &fake_wgl_lookup, &fake_module_lookup};
	GlFunctionTable direct_table{};
	check(load_required_gl_functions(direct_resolver, &direct_table, nullptr) &&
		direct_state.module_lookups == 0,
		"valid WGL procedures must be used without module fallback");

	const std::array<uintptr_t, 5> sentinels{{0, 1, 2, 3, UINTPTR_MAX}};
	for(uintptr_t value : sentinels) {
		check(is_invalid_wgl_address(reinterpret_cast<void*>(value)),
			"WGL null and sentinel addresses must be rejected");
		ResolverState state{};
		state.wgl_address = reinterpret_cast<void*>(value);
		ProcResolver resolver{&state, &fake_wgl_lookup, &fake_module_lookup};
		GlFunctionTable table{};
		const char* missing = nullptr;
		check(load_required_gl_functions(resolver, &table, &missing),
			"sentinel WGL result must fall back to opengl32 lookup");
		check(state.module_lookups >= GlFunctionTable::required_count,
			"fallback must resolve each required GL entry point while optional lookups remain non-fatal");
	}

	ResolverState missing_state{};
	missing_state.wgl_address = nullptr;
	missing_state.missing_module_address = true;
	ProcResolver missing_resolver{&missing_state, &fake_wgl_lookup, &fake_module_lookup};
	GlFunctionTable table{};
	table.procedures.fill(&missing_state.token);
	const char* missing = nullptr;
	check(!load_required_gl_functions(missing_resolver, &table, &missing),
		"missing required GL export must fail loading");
	check(missing != nullptr && std::strcmp(missing, "glBindTexture") == 0,
		"loader must identify the missing GL function");
	check(table.procedures[0] == nullptr,
		"failed load must not leave a partial function table visible");

	ResolverState ext_state{};
	ext_state.use_ext_framebuffer_aliases = true;
	ext_state.wgl_address = &ext_state.token;
	ProcResolver ext_resolver{&ext_state, &fake_wgl_lookup, &fake_module_lookup};
	check(load_required_gl_functions(ext_resolver, &table, &missing),
		"EXT framebuffer procedures must satisfy the compatible loader path");
}

class FakeTextureDevice final : public revival::graphics::TextureDevice {
public:
	struct FakeTexture {
		revival::graphics::TextureAllocation allocation{};
		std::vector<uint8_t> gl_bottom_up;
	};

	uint32_t max_dimension = 64;
	bool fail_create = false;
	bool incomplete_framebuffer = false;
	bool fail_upload = false;
	uint32_t next_texture = 1;
	uint32_t next_framebuffer = 100;
	RvResult owner_result = RV_RESULT_OK;
	std::unordered_map<uint32_t, FakeTexture> textures;

	RvResult check_owner() const noexcept override
	{
		return owner_result;
	}

	RvResult max_texture_dimension(uint32_t* out) noexcept override
	{
		if(owner_result != RV_RESULT_OK) return owner_result;
		if(out == nullptr) return RV_RESULT_INVALID_ARGUMENT;
		*out = max_dimension;
		return RV_RESULT_OK;
	}

	RvResult create_rgba8(uint32_t width, uint32_t height,
		revival::graphics::TextureAllocation* out) noexcept override
	{
		if(out == nullptr || width == 0 || height == 0) return RV_RESULT_INVALID_ARGUMENT;
		if(fail_create || incomplete_framebuffer) return RV_RESULT_GL_FAILURE;
		try {
			FakeTexture texture{};
			texture.allocation.texture = next_texture++;
			texture.allocation.framebuffer = next_framebuffer++;
			texture.allocation.width = width;
			texture.allocation.height = height;
			texture.allocation.storage_bytes = static_cast<uint64_t>(width) * height * 4;
			texture.gl_bottom_up.resize(static_cast<size_t>(texture.allocation.storage_bytes));
			const uint32_t id = texture.allocation.texture;
			textures.emplace(id, std::move(texture));
			*out = textures.at(id).allocation;
			return RV_RESULT_OK;
		} catch(...) {
			return RV_RESULT_RESOURCE_LIMIT;
		}
	}

	RvResult upload_rgba8_rows(const revival::graphics::TextureAllocation& allocation,
		uint32_t first_gl_row, uint32_t row_count, const uint8_t* bytes,
		uint32_t byte_count) noexcept override
	{
		if(fail_upload) {
			fail_upload = false;
			return RV_RESULT_GL_FAILURE;
		}
		auto found = textures.find(allocation.texture);
		const uint64_t expected = static_cast<uint64_t>(allocation.width) * row_count * 4;
		if(found == textures.end() || bytes == nullptr || row_count == 0 ||
			first_gl_row > allocation.height || row_count > allocation.height - first_gl_row ||
			expected != byte_count) return RV_RESULT_INVALID_ARGUMENT;
		const size_t row_bytes = static_cast<size_t>(allocation.width) * 4;
		for(uint32_t row = 0; row < row_count; ++row) {
			std::memcpy(found->second.gl_bottom_up.data() +
				static_cast<size_t>(first_gl_row + row) * row_bytes,
				bytes + static_cast<size_t>(row) * row_bytes, row_bytes);
		}
		return RV_RESULT_OK;
	}

	void destroy(const revival::graphics::TextureAllocation& allocation) noexcept override
	{
		textures.erase(allocation.texture);
	}

	std::array<uint8_t, 4> top_left_pixel(uint32_t texture, uint32_t x, uint32_t y) const
	{
		const auto found = textures.find(texture);
		if(found == textures.end()) return {};
		const auto& allocation = found->second.allocation;
		const size_t offset = (static_cast<size_t>(allocation.height - y - 1) * allocation.width + x) * 4;
		return {{found->second.gl_bottom_up[offset], found->second.gl_bottom_up[offset + 1],
			found->second.gl_bottom_up[offset + 2], found->second.gl_bottom_up[offset + 3]}};
	}

	void set_top_left_pixel(uint32_t texture, uint32_t x, uint32_t y,
		const std::array<uint8_t, 4>& pixel)
	{
		auto found = textures.find(texture);
		if(found == textures.end()) return;
		const auto& allocation = found->second.allocation;
		const size_t offset = (static_cast<size_t>(allocation.height - y - 1) * allocation.width + x) * 4;
		std::copy(pixel.begin(), pixel.end(), found->second.gl_bottom_up.begin() + offset);
	}
};

class FakeSurfaceScaler final : public revival::graphics::SurfaceScaler {
public:
	explicit FakeSurfaceScaler(revival::graphics::TextureStore& store) noexcept : store_(store) {}
	bool fail_next = false;
	RvResult apply(RvSurfaceHandle, RvSurfaceHandle destination,
		revival::graphics::ScaleReport* report) noexcept override
	{
		if(fail_next) {
			fail_next = false;
			return RV_RESULT_GL_FAILURE;
		}
		if(report == nullptr) return RV_RESULT_INVALID_ARGUMENT;
		report->path = revival::graphics::ScalePath::GpuScale2x;
		report->passes = 1;
		return store_.mark_modified(destination);
	}

private:
	revival::graphics::TextureStore& store_;
};

class FakeSurfaceRenderer final : public revival::graphics::SurfaceRenderer {
public:
	explicit FakeSurfaceRenderer(FakeTextureDevice& device) noexcept : device_(device) {}
	uint32_t clear_count = 0;
	uint32_t fill_count = 0;
	uint32_t blit_count = 0;
	uint32_t copy_count = 0;

	RvResult clear_transparent(const revival::graphics::TextureSurfaceSnapshot& destination) noexcept override
	{
		++clear_count;
		for(uint32_t y = 0; y < destination.active.height; ++y) {
			for(uint32_t x = 0; x < destination.active.width; ++x) {
				device_.set_top_left_pixel(destination.active.texture, x, y, {{0, 0, 0, 0}});
			}
		}
		return RV_RESULT_OK;
	}

	RvResult copy_surface(const revival::graphics::TextureAllocation& source,
		const revival::graphics::TextureAllocation& destination) noexcept override
	{
		++copy_count;
		auto from = device_.textures.find(source.texture);
		auto to = device_.textures.find(destination.texture);
		if(from == device_.textures.end() || to == device_.textures.end() ||
			source.width != destination.width || source.height != destination.height) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		to->second.gl_bottom_up = from->second.gl_bottom_up;
		return RV_RESULT_OK;
	}

	RvResult fill(const revival::graphics::TextureSurfaceSnapshot& destination,
		const revival::graphics::SurfaceRect& rectangle,
		const revival::graphics::SurfaceRect& clip, uint32_t argb) noexcept override
	{
		++fill_count;
		const int64_t left = std::max<int64_t>({rectangle.x, clip.x, 0});
		const int64_t top = std::max<int64_t>({rectangle.y, clip.y, 0});
		const int64_t right = std::min<int64_t>({
			static_cast<int64_t>(rectangle.x) + rectangle.width,
			static_cast<int64_t>(clip.x) + clip.width, destination.logical_width});
		const int64_t bottom = std::min<int64_t>({
			static_cast<int64_t>(rectangle.y) + rectangle.height,
			static_cast<int64_t>(clip.y) + clip.height, destination.logical_height});
		for(int64_t y = top; y < bottom; ++y) {
			for(int64_t x = left; x < right; ++x) {
				for(uint32_t dy = 0; dy < destination.storage_scale; ++dy) {
					for(uint32_t dx = 0; dx < destination.storage_scale; ++dx) {
						const uint32_t px = static_cast<uint32_t>(x) * destination.storage_scale + dx;
						const uint32_t py = static_cast<uint32_t>(y) * destination.storage_scale + dy;
						device_.set_top_left_pixel(destination.active.texture, px, py,
							blend_fill_pixel(argb, device_.top_left_pixel(
								destination.active.texture, px, py)));
					}
				}
			}
		}
		return RV_RESULT_OK;
	}

	RvResult blit(const revival::graphics::TextureAllocation& source,
		uint32_t source_logical_width, uint32_t source_logical_height, uint32_t source_scale,
		const revival::graphics::TextureSurfaceSnapshot& destination,
		const revival::graphics::SurfaceRect& source_rect,
		const revival::graphics::SurfaceRect& destination_rect,
		const revival::graphics::SurfaceRect& clip, uint32_t transform,
		revival::graphics::SurfaceBlend blend) noexcept override
	{
		++blit_count;
		if(source_scale == 0 ||
			static_cast<uint64_t>(source_logical_width) * source_scale != source.width ||
			static_cast<uint64_t>(source_logical_height) * source_scale != source.height) {
			return RV_RESULT_INVALID_ARGUMENT;
		}
		static constexpr uint8_t map[8][4] = {
			{0, 1, 2, 3}, {3, 2, 1, 0}, {1, 0, 3, 2}, {2, 3, 0, 1},
			{0, 3, 2, 1}, {3, 0, 1, 2}, {1, 2, 3, 0}, {2, 1, 0, 3}
		};
		const int64_t left = std::max<int64_t>({destination_rect.x, clip.x, 0});
		const int64_t top = std::max<int64_t>({destination_rect.y, clip.y, 0});
		const int64_t right = std::min<int64_t>({
			static_cast<int64_t>(destination_rect.x) + destination_rect.width,
			static_cast<int64_t>(clip.x) + clip.width, destination.logical_width});
		const int64_t bottom = std::min<int64_t>({
			static_cast<int64_t>(destination_rect.y) + destination_rect.height,
			static_cast<int64_t>(clip.y) + clip.height, destination.logical_height});
		for(int64_t y = top; y < bottom; ++y) {
			for(int64_t x = left; x < right; ++x) {
				for(uint32_t dy = 0; dy < destination.storage_scale; ++dy) {
					for(uint32_t dx = 0; dx < destination.storage_scale; ++dx) {
						const double p = (static_cast<double>(x - destination_rect.x) +
							(static_cast<double>(dx) + 0.5) / destination.storage_scale) /
							destination_rect.width;
						const double q = (static_cast<double>(y - destination_rect.y) +
							(static_cast<double>(dy) + 0.5) / destination.storage_scale) /
							destination_rect.height;
						double corner_u[4]{};
						double corner_v[4]{};
						const double u0 = static_cast<double>(source_rect.x) / source_logical_width;
						const double u1 = static_cast<double>(source_rect.x + source_rect.width) / source_logical_width;
						const double v0 = 1.0 - static_cast<double>(source_rect.y) / source_logical_height;
						const double v1 = 1.0 - static_cast<double>(source_rect.y + source_rect.height) / source_logical_height;
						corner_u[0] = u0; corner_v[0] = v0;
						corner_u[1] = u1; corner_v[1] = v0;
						corner_u[2] = u1; corner_v[2] = v1;
						corner_u[3] = u0; corner_v[3] = v1;
						double u = 0.0, v = 0.0;
						const double weights[4] = {(1-p)*(1-q), p*(1-q), p*q, (1-p)*q};
						for(size_t index = 0; index < 4; ++index) {
							u += corner_u[map[transform][index]] * weights[index];
							v += corner_v[map[transform][index]] * weights[index];
						}
						const uint32_t source_x = std::min<uint32_t>(source.width - 1,
							static_cast<uint32_t>(std::max(0.0, std::floor(u * source.width))));
						const uint32_t source_y = std::min<uint32_t>(source.height - 1,
							static_cast<uint32_t>(std::max(0.0, std::floor((1.0 - v) * source.height))));
						const uint32_t target_x = static_cast<uint32_t>(x) * destination.storage_scale + dx;
						const uint32_t target_y = static_cast<uint32_t>(y) * destination.storage_scale + dy;
						const auto incoming = device_.top_left_pixel(source.texture, source_x, source_y);
						device_.set_top_left_pixel(destination.active.texture, target_x, target_y,
							blend_pixel(incoming, device_.top_left_pixel(destination.active.texture,
								target_x, target_y), blend));
					}
				}
			}
		}
		return RV_RESULT_OK;
	}

private:
	static std::array<uint8_t, 4> blend_fill_pixel(uint32_t argb,
		const std::array<uint8_t, 4>& destination) noexcept
	{
		const float alpha = static_cast<float>(argb >> 24) / 255.f;
		const float inverse_alpha = 1.f - alpha;
		const std::array<float, 4> source{{
			static_cast<float>((argb >> 16) & 255u) / 255.f * alpha,
			static_cast<float>((argb >> 8) & 255u) / 255.f * alpha,
			static_cast<float>(argb & 255u) / 255.f * alpha,
			alpha}};
		std::array<uint8_t, 4> output{};
		for(size_t channel = 0; channel < output.size(); ++channel) {
			const float value = (source[channel] +
				static_cast<float>(destination[channel]) / 255.f * inverse_alpha) * 255.f;
			output[channel] = static_cast<uint8_t>(std::clamp(
				static_cast<int>(std::floor(value + 0.5f)), 0, 255));
		}
		return output;
	}

	static std::array<uint8_t, 4> blend_pixel(const std::array<uint8_t, 4>& source,
		const std::array<uint8_t, 4>& destination, revival::graphics::SurfaceBlend blend) noexcept
	{
		if(blend == revival::graphics::SurfaceBlend::Replace) return source;
		std::array<uint8_t, 4> output{};
		for(size_t channel = 0; channel < 4; ++channel) {
			const uint32_t value = source[channel] +
				(static_cast<uint32_t>(destination[channel]) * (255u - source[3]) + 127u) / 255u;
			output[channel] = static_cast<uint8_t>(std::min<uint32_t>(255, value));
		}
		return output;
	}

	FakeTextureDevice& device_;
};

void append_u16(std::vector<uint8_t>& bytes, uint16_t value)
{
	bytes.push_back(static_cast<uint8_t>(value));
	bytes.push_back(static_cast<uint8_t>(value >> 8));
}

void append_u32(std::vector<uint8_t>& bytes, uint32_t value)
{
	for(uint32_t shift = 0; shift < 32; shift += 8) {
		bytes.push_back(static_cast<uint8_t>(value >> shift));
	}
}

void append_u64(std::vector<uint8_t>& bytes, uint64_t value)
{
	append_u32(bytes, static_cast<uint32_t>(value));
	append_u32(bytes, static_cast<uint32_t>(value >> 32));
}

void append_rect(std::vector<uint8_t>& bytes, const revival::graphics::SurfaceRect& rect)
{
	append_u32(bytes, static_cast<uint32_t>(rect.x));
	append_u32(bytes, static_cast<uint32_t>(rect.y));
	append_u32(bytes, static_cast<uint32_t>(rect.width));
	append_u32(bytes, static_cast<uint32_t>(rect.height));
}

void set_u16(std::vector<uint8_t>& bytes, size_t offset, uint16_t value)
{
	bytes[offset] = static_cast<uint8_t>(value);
	bytes[offset + 1] = static_cast<uint8_t>(value >> 8);
}

void set_u32(std::vector<uint8_t>& bytes, size_t offset, uint32_t value)
{
	for(uint32_t shift = 0; shift < 32; shift += 8) {
		bytes[offset + shift / 8] = static_cast<uint8_t>(value >> shift);
	}
}

void set_u64(std::vector<uint8_t>& bytes, size_t offset, uint64_t value)
{
	set_u32(bytes, offset, static_cast<uint32_t>(value));
	set_u32(bytes, offset + 4, static_cast<uint32_t>(value >> 32));
}

std::vector<uint8_t> make_surface_packet(
	const std::vector<std::pair<uint32_t, std::vector<uint8_t>>>& operations,
	uint64_t epoch = 7, uint64_t sequence = 11)
{
	std::vector<uint8_t> bytes(RV_PACKET_HEADER_BYTES, 0);
	for(const auto& operation : operations) {
		append_u16(bytes, static_cast<uint16_t>(operation.first));
		append_u16(bytes, 0);
		append_u32(bytes, RV_COMMAND_HEADER_BYTES + static_cast<uint32_t>(operation.second.size()));
		bytes.insert(bytes.end(), operation.second.begin(), operation.second.end());
	}
	set_u32(bytes, 0, RV_PACKET_MAGIC);
	set_u16(bytes, 4, RV_ABI_VERSION);
	set_u16(bytes, 6, RV_PACKET_HEADER_BYTES);
	set_u32(bytes, 8, static_cast<uint32_t>(bytes.size()));
	set_u32(bytes, 12, static_cast<uint32_t>(operations.size()));
	set_u64(bytes, 16, epoch);
	set_u64(bytes, 24, sequence);
	return bytes;
}

std::vector<uint8_t> fill_payload(RvSurfaceHandle destination,
	const revival::graphics::SurfaceRect& rectangle, uint32_t argb,
	const revival::graphics::SurfaceRect& clip)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, destination);
	append_rect(bytes, rectangle);
	append_u32(bytes, argb);
	append_rect(bytes, clip);
	return bytes;
}

std::vector<uint8_t> blit_payload(RvSurfaceHandle source, RvSurfaceHandle destination,
	const revival::graphics::SurfaceRect& source_rect,
	const revival::graphics::SurfaceRect& destination_rect,
	const revival::graphics::SurfaceRect& clip, uint32_t transform,
	uint32_t blend = static_cast<uint32_t>(revival::graphics::SurfaceBlend::Replace))
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, source);
	append_u64(bytes, destination);
	append_rect(bytes, source_rect);
	append_rect(bytes, destination_rect);
	append_rect(bytes, clip);
	append_u32(bytes, transform);
	append_u32(bytes, blend);
	return bytes;
}

std::vector<uint8_t> destroy_payload(RvSurfaceHandle surface)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, surface);
	return bytes;
}

RvFrameDesc frame_desc(uint32_t format, uint32_t alpha);

RvFrameDesc upload_desc(uint32_t width, uint32_t height, uint32_t scale = 1)
{
	RvFrameDesc desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	desc.physical_width = width * scale;
	desc.physical_height = height * scale;
	desc.physical_stride_bytes = desc.physical_width * 4;
	desc.logical_width = width;
	desc.logical_height = height;
	desc.source_scale = scale;
	return desc;
}

std::vector<uint8_t> create_surface_payload(RvSurfaceHandle surface,
	uint32_t width, uint32_t height, uint32_t scale)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, surface);
	append_u32(bytes, width);
	append_u32(bytes, height);
	append_u32(bytes, scale);
	return bytes;
}

std::vector<uint8_t> begin_upload_payload(RvSurfaceHandle surface, RvFrameDesc descriptor,
	uint32_t total_bytes)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, surface);
	append_u32(bytes, descriptor.struct_bytes);
	append_u32(bytes, descriptor.abi_version);
	append_u64(bytes, descriptor.frame_sequence);
	append_u32(bytes, descriptor.physical_width);
	append_u32(bytes, descriptor.physical_height);
	append_u32(bytes, descriptor.physical_stride_bytes);
	append_u32(bytes, descriptor.logical_width);
	append_u32(bytes, descriptor.logical_height);
	append_u32(bytes, descriptor.source_scale);
	append_u32(bytes, descriptor.desired_scale);
	append_u32(bytes, descriptor.pixel_format);
	append_u32(bytes, descriptor.alpha_mode);
	append_u32(bytes, descriptor.origin);
	append_u32(bytes, total_bytes);
	return bytes;
}

std::vector<uint8_t> upload_rows_payload(RvSurfaceHandle surface, uint32_t first_row,
	uint32_t row_count, uint32_t stride, const std::vector<uint8_t>& pixels)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, surface);
	append_u32(bytes, first_row);
	append_u32(bytes, row_count);
	append_u32(bytes, stride);
	append_u32(bytes, static_cast<uint32_t>(pixels.size()));
	bytes.insert(bytes.end(), pixels.begin(), pixels.end());
	return bytes;
}

std::vector<uint8_t> commit_frame_payload(RvSurfaceHandle surface, uint64_t sequence,
	uint32_t scale, const revival::graphics::SurfaceRect& content)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, surface);
	append_u64(bytes, sequence);
	append_u32(bytes, scale);
	append_rect(bytes, content);
	return bytes;
}

std::vector<uint8_t> scale_surface_payload(RvSurfaceHandle source,
	RvSurfaceHandle destination, uint32_t target_scale, uint32_t reserved = 0)
{
	std::vector<uint8_t> bytes;
	append_u64(bytes, source);
	append_u64(bytes, destination);
	append_u32(bytes, target_scale);
	append_u32(bytes, reserved);
	return bytes;
}

bool upload_pixels(revival::graphics::TextureStore& store, RvSurfaceHandle handle,
	uint32_t width, uint32_t height, const std::vector<uint8_t>& pixels, uint32_t scale = 1)
{
	const RvFrameDesc desc = upload_desc(width, height, scale);
	if(store.begin_upload(handle, desc, static_cast<uint32_t>(pixels.size())) != RV_RESULT_OK) return false;
	if(store.upload_rows(handle, 0, desc.physical_height, desc.physical_stride_bytes,
		pixels.data(), static_cast<uint32_t>(pixels.size())) != RV_RESULT_OK) return false;
	return store.end_upload(handle) == RV_RESULT_OK;
}

RvFrameDesc frame_desc(uint32_t format, uint32_t alpha)
{
	RvFrameDesc desc{};
	desc.struct_bytes = RV_FRAME_DESC_BYTES;
	desc.abi_version = RV_ABI_VERSION;
	desc.physical_width = 2;
	desc.physical_height = 2;
	desc.physical_stride_bytes = 8;
	desc.logical_width = 2;
	desc.logical_height = 2;
	desc.source_scale = RV_SCALE_ORIGINAL;
	desc.desired_scale = RV_SCALE_2X;
	desc.pixel_format = format;
	desc.alpha_mode = alpha;
	desc.origin = RV_ORIGIN_TOP_LEFT;
	return desc;
}

void test_pixel_transfer()
{
	using namespace revival::graphics;
	RvFrameDesc desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	const std::array<uint8_t, 16> rgba{{
		1, 2, 3, 128, 200, 100, 50, 128,
		90, 80, 70, 0, 10, 20, 30, 255
	}};
	std::vector<uint8_t> converted;
	check(convert_rows_to_gl_rgba(desc, 0, 2, 8, rgba.data(),
		static_cast<uint32_t>(rgba.size()), &converted) ==
		RV_RESULT_OK, "straight RGBA rows must convert");
	const std::array<uint8_t, 16> expected{{
		0, 0, 0, 0, 10, 20, 30, 255,
		1, 1, 2, 128, 100, 50, 25, 128
	}};
	check(converted.size() == expected.size() &&
		std::equal(converted.begin(), converted.end(), expected.begin()),
		"straight-alpha rounding, zero-alpha cleanup and top-to-bottom reversal must match exactly");

	desc = frame_desc(RV_PIXEL_FORMAT_BGRA8, RV_ALPHA_STRAIGHT);
	const std::array<uint8_t, 8> bgra{{30, 20, 10, 255, 0, 0, 0, 0}};
	check(convert_rows_to_gl_rgba(desc, 0, 1, 8, bgra.data(), 8, &converted) ==
		RV_RESULT_OK && converted.size() == 8 && converted[0] == 10 &&
		converted[1] == 20 && converted[2] == 30 && converted[3] == 255,
		"BGRA rows must become premultiplied RGBA without changing opaque colors");

	desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_PREMULTIPLIED);
	const std::array<uint8_t, 8> premultiplied{{60, 40, 20, 128, 9, 8, 7, 0}};
	check(convert_rows_to_gl_rgba(desc, 0, 1, 8, premultiplied.data(), 8, &converted) ==
		RV_RESULT_OK && converted[0] == 60 && converted[1] == 40 && converted[2] == 20 &&
		converted[4] == 0 && converted[5] == 0 && converted[6] == 0 && converted[7] == 0,
		"premultiplied input must not be multiplied again and zero-alpha RGB must be cleared");

	desc = frame_desc(RV_PIXEL_FORMAT_BGRA8, RV_ALPHA_OPAQUE);
	const std::array<uint8_t, 8> opaque{{30, 20, 10, 0, 0, 0, 0, 255}};
	check(convert_rows_to_gl_rgba(desc, 0, 1, 8, opaque.data(), 8, &converted) ==
		RV_RESULT_OK && converted[0] == 10 && converted[1] == 20 &&
		converted[2] == 30 && converted[3] == 255,
		"opaque BGRA input must ignore source alpha and force alpha to 255");
	desc.alpha_mode = 99;
	check(convert_rows_to_gl_rgba(desc, 0, 1, 8, opaque.data(), 8, &converted) ==
		RV_RESULT_INVALID_ARGUMENT,
		"pixel conversion must reject unsupported alpha modes even outside the store");
	desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	desc.physical_stride_bytes = 12;
	check(convert_rows_to_gl_rgba(desc, 0, 1, 8, opaque.data(), 8, &converted) ==
		RV_RESULT_INVALID_ARGUMENT,
		"row conversion must require the descriptor's declared source stride");
}

void test_surface_protocol()
{
	using namespace revival::graphics;
	const SurfaceRect full{0, 0, 3, 2};
	auto bytes = make_surface_packet({
		{RV_OP_FILL, fill_payload(31, SurfaceRect{-1, 0, 2, 2}, 0x80FF0000u, full)},
		{RV_OP_BLIT, blit_payload(31, 32, SurfaceRect{0, 0, 3, 2}, full, full, 7)}});
	ValidatedPacket packet{};
	check(validate_packet(bytes.data(), static_cast<uint32_t>(bytes.size()), 7, &packet) ==
		RV_RESULT_OK, "surface commands with exact payloads must pass packet validation");
	std::vector<SurfaceCommand> commands;
	check(decode_surface_commands(bytes.data(), static_cast<uint32_t>(bytes.size()), packet,
		&commands) == RV_RESULT_OK && commands.size() == 2 &&
		commands[0].destination == 31 && commands[0].destination_rect.x == -1 &&
		commands[0].argb == 0x80FF0000u && commands[1].source == 31 &&
		commands[1].destination == 32 && commands[1].transform == 7,
		"surface decoder must preserve signed rectangles, color, order and transform");

	OwnedCommandQueue queue(QueueLimits{1024, 8, 1}, 7);
	auto malformed = make_surface_packet({{RV_OP_FILL, std::vector<uint8_t>{1, 2, 3}}});
	check(queue.submit_packet(malformed.data(), static_cast<uint32_t>(malformed.size())) ==
		RV_RESULT_BAD_PACKET && queue.snapshot().queued_operations == 0,
		"malformed surface payload must be rejected before queue admission");
	auto invalid_transform = make_surface_packet({
		{RV_OP_BLIT, blit_payload(31, 32, SurfaceRect{0, 0, 1, 1},
			SurfaceRect{0, 0, 1, 1}, SurfaceRect{0, 0, 1, 1}, 8)}});
	check(validate_packet(invalid_transform.data(), static_cast<uint32_t>(invalid_transform.size()),
		7, &packet) == RV_RESULT_INVALID_ARGUMENT,
		"transforms outside the eight defined corner maps must fail validation");
	auto invalid_blend = make_surface_packet({
		{RV_OP_COPY, blit_payload(31, 32, SurfaceRect{0, 0, 1, 1},
			SurfaceRect{0, 0, 1, 1}, SurfaceRect{0, 0, 1, 1}, 0, 99)}});
	check(validate_packet(invalid_blend.data(), static_cast<uint32_t>(invalid_blend.size()),
		7, &packet) == RV_RESULT_INVALID_ARGUMENT,
		"unknown replace/source-over blend mode must fail validation");
}

void test_generic_command_protocol()
{
	using namespace revival::graphics;
	RvFrameDesc desc = upload_desc(2, 2);
	desc.frame_sequence = 0;
	const std::vector<uint8_t> first_row{1, 2, 3, 255, 4, 5, 6, 255};
	const std::vector<uint8_t> second_row{7, 8, 9, 255, 10, 11, 12, 255};
	auto packet_bytes = make_surface_packet({
		{RV_OP_CREATE_SURFACE, create_surface_payload(31, 2, 2, RV_SCALE_ORIGINAL)},
		{RV_OP_BEGIN_UPLOAD, begin_upload_payload(31, desc, 16)},
		{RV_OP_UPLOAD_ROWS, upload_rows_payload(31, 0, 1, 8, first_row)},
		{RV_OP_UPLOAD_ROWS, upload_rows_payload(31, 1, 1, 8, second_row)},
		{RV_OP_END_UPLOAD, destroy_payload(31)},
		{RV_OP_FILL, fill_payload(31, SurfaceRect{0, 0, 1, 1}, 0xFFFF0000u,
			SurfaceRect{0, 0, 2, 2})},
		{RV_OP_COMMIT_FRAME, commit_frame_payload(31, 1, RV_SCALE_2X, SurfaceRect{0, 0, 2, 2})},
		{RV_OP_CLEAR_FRAME, {}}
	});
	ValidatedPacket validated{};
	std::vector<GraphicsCommand> commands;
	check(validate_packet(packet_bytes.data(), static_cast<uint32_t>(packet_bytes.size()), 7,
		&validated) == RV_RESULT_OK &&
		decode_graphics_commands(packet_bytes.data(), static_cast<uint32_t>(packet_bytes.size()),
			validated, &commands) == RV_RESULT_OK && commands.size() == 8 &&
		commands[2].data_bytes == first_row.size() &&
		commands[2].data_offset + commands[2].data_bytes <= packet_bytes.size() &&
		commands[6].frame_sequence == 1 && commands[7].opcode == RV_OP_CLEAR_FRAME,
		"lifecycle, row-upload, draw, frame commit and clear payloads decode in order");

	RvFrameDesc invalid_desc = desc;
	invalid_desc.frame_sequence = 1;
	auto invalid_begin = make_surface_packet({
		{RV_OP_BEGIN_UPLOAD, begin_upload_payload(31, invalid_desc, 16)}});
	check(validate_packet(invalid_begin.data(), static_cast<uint32_t>(invalid_begin.size()), 7,
		&validated) == RV_RESULT_INVALID_ARGUMENT,
		"an upload descriptor cannot smuggle a frame sequence");
	auto invalid_rows = make_surface_packet({
		{RV_OP_UPLOAD_ROWS, upload_rows_payload(31, 0, 1, 8,
			std::vector<uint8_t>{1, 2, 3})}});
	check(validate_packet(invalid_rows.data(), static_cast<uint32_t>(invalid_rows.size()), 7,
		&validated) == RV_RESULT_INVALID_ARGUMENT,
		"row data byte count must exactly match row count and stride");
	auto zero_sequence = make_surface_packet({{RV_OP_CLEAR_FRAME, {}}}, 7, 0);
	check(validate_packet(zero_sequence.data(), static_cast<uint32_t>(zero_sequence.size()), 7,
		&validated) == RV_RESULT_BAD_PACKET,
		"zero packet sequence must be rejected at the native boundary");
	auto scale = make_surface_packet({
		{RV_OP_SCALE_SURFACE, scale_surface_payload(31, 32, RV_SCALE_2X)}});
	check(validate_packet(scale.data(), static_cast<uint32_t>(scale.size()), 7, &validated) == RV_RESULT_OK &&
		decode_graphics_commands(scale.data(), static_cast<uint32_t>(scale.size()), validated, &commands) == RV_RESULT_OK &&
		commands.size() == 1 && commands[0].scale_source == 31 &&
		commands[0].scale_destination == 32 && commands[0].scale_target == RV_SCALE_2X,
		"surface-scale command must preserve handles and target density");
	auto invalid_scale = make_surface_packet({
		{RV_OP_SCALE_SURFACE, scale_surface_payload(31, 32, RV_SCALE_ORIGINAL)}});
	check(validate_packet(invalid_scale.data(), static_cast<uint32_t>(invalid_scale.size()), 7, &validated) ==
		RV_RESULT_INVALID_ARGUMENT, "surface-scale must reject original density");
}

void test_surface_operations()
{
	using namespace revival::graphics;
	FakeTextureDevice device;
	TextureStore store(device, 8192);
	FakeSurfaceRenderer renderer(device);
	SurfaceOperations operations(store, renderer);
	const std::array<uint8_t, 24> source_pixels{{
		10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255,
		40, 0, 0, 255, 50, 0, 0, 255, 60, 0, 0, 255
	}};
	check(store.create_surface(100, 3, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		upload_pixels(store, 100, 3, 2,
			std::vector<uint8_t>(source_pixels.begin(), source_pixels.end())),
		"asymmetric source fixture must create and upload");
	static constexpr uint8_t expected[8][6] = {
		{10, 20, 30, 40, 50, 60}, {40, 50, 60, 10, 20, 30},
		{30, 20, 10, 60, 50, 40}, {60, 50, 40, 30, 20, 10},
		{10, 40, 20, 50, 30, 60}, {40, 10, 50, 20, 60, 30},
		{30, 60, 20, 50, 10, 40}, {60, 30, 50, 20, 40, 10}
	};
	for(uint32_t transform = 0; transform < 8; ++transform) {
		const uint32_t width = transform < 4 ? 3 : 2;
		const uint32_t height = transform < 4 ? 2 : 3;
		const RvSurfaceHandle destination = 200 + transform;
		check(store.create_surface(destination, width, height, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
			"each transform destination must allocate independently");
		SurfaceCommand command{};
		command.opcode = RV_OP_BLIT;
		command.source = 100;
		command.destination = destination;
		command.source_rect = SurfaceRect{0, 0, 3, 2};
		command.destination_rect = SurfaceRect{0, 0, static_cast<int32_t>(width), static_cast<int32_t>(height)};
		command.clip = command.destination_rect;
		command.transform = transform;
		command.blend = SurfaceBlend::Replace;
		check(operations.execute({command}) == RV_RESULT_OK,
			"all eight transforms must execute against the synthetic surface");
		TextureSurfaceSnapshot snapshot{};
		check(store.inspect(destination, &snapshot) == RV_RESULT_OK && snapshot.initialized,
			"a written surface must become initialized");
		for(uint32_t y = 0; y < height; ++y) {
			for(uint32_t x = 0; x < width; ++x) {
				check(device.top_left_pixel(snapshot.active.texture, x, y)[0] ==
					expected[transform][y * width + x],
					"asymmetric fixture must match the expected MIDP corner mapping");
			}
		}
	}

	check(store.create_surface(300, 3, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"clip destination must allocate");
	SurfaceCommand clipped{};
	clipped.opcode = RV_OP_BLIT;
	clipped.source = 100;
	clipped.destination = 300;
	clipped.source_rect = SurfaceRect{0, 0, 3, 2};
	clipped.destination_rect = SurfaceRect{0, 0, 3, 2};
	clipped.clip = SurfaceRect{1, 0, 1, 2};
	clipped.blend = SurfaceBlend::Replace;
	check(operations.execute({clipped}) == RV_RESULT_OK,
		"surface scissor must accept a narrow top-left clip");
	TextureSurfaceSnapshot clipped_surface{};
	check(store.inspect(300, &clipped_surface) == RV_RESULT_OK &&
		device.top_left_pixel(clipped_surface.active.texture, 0, 0)[3] == 0 &&
		device.top_left_pixel(clipped_surface.active.texture, 1, 0)[0] == 20 &&
		device.top_left_pixel(clipped_surface.active.texture, 2, 0)[3] == 0 &&
		device.top_left_pixel(clipped_surface.active.texture, 1, 1)[0] == 50,
		"clip must preserve pixels outside the requested column and row range");

	check(store.create_surface(400, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"fill destination must allocate");
	SurfaceCommand fill{};
	fill.opcode = RV_OP_FILL;
	fill.destination = 400;
	fill.destination_rect = SurfaceRect{-1, 0, 2, 2};
	fill.clip = SurfaceRect{0, 0, 2, 2};
	fill.argb = 0x80FF0000u;
	check(operations.execute({fill}) == RV_RESULT_OK,
		"partially out-of-bounds translucent fill must clip safely");
	TextureSurfaceSnapshot filled{};
	check(store.inspect(400, &filled) == RV_RESULT_OK &&
		device.top_left_pixel(filled.active.texture, 0, 0) == std::array<uint8_t, 4>{{128, 0, 0, 128}} &&
		device.top_left_pixel(filled.active.texture, 1, 0) == std::array<uint8_t, 4>{{0, 0, 0, 0}},
		"fill must use exact premultiplied color and leave clipped pixels transparent");
	const std::array<uint8_t, 4> opaque_low_red{{2, 0, 0, 255}};
	check(store.create_surface(403, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		upload_pixels(store, 403, 1, 1,
			std::vector<uint8_t>(opaque_low_red.begin(), opaque_low_red.end())),
		"fill precision fixture must upload a small opaque destination color");
	SurfaceCommand precise_fill{};
	precise_fill.opcode = RV_OP_FILL;
	precise_fill.destination = 403;
	precise_fill.destination_rect = SurfaceRect{0, 0, 1, 1};
	precise_fill.clip = precise_fill.destination_rect;
	precise_fill.argb = 0x80010000u;
	check(operations.execute({precise_fill}) == RV_RESULT_OK,
		"translucent fill must composite with legacy floating-point color precision");
	TextureSurfaceSnapshot precise_fill_surface{};
	check(store.inspect(403, &precise_fill_surface) == RV_RESULT_OK &&
		device.top_left_pixel(precise_fill_surface.active.texture, 0, 0) ==
			std::array<uint8_t, 4>{{1, 0, 0, 255}},
		"fill must avoid early channel rounding before blending");

	const std::array<uint8_t, 4> half_red{{200, 0, 0, 128}};
	const std::array<uint8_t, 4> opaque_blue{{0, 0, 255, 255}};
	check(store.create_surface(401, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		upload_pixels(store, 401, 1, 1, std::vector<uint8_t>(half_red.begin(), half_red.end())) &&
		store.create_surface(402, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		upload_pixels(store, 402, 1, 1, std::vector<uint8_t>(opaque_blue.begin(), opaque_blue.end())),
		"source-over fixtures must upload their alpha and destination colors");
	SurfaceCommand over{};
	over.opcode = RV_OP_BLIT;
	over.source = 401;
	over.destination = 402;
	over.source_rect = SurfaceRect{0, 0, 1, 1};
	over.destination_rect = SurfaceRect{0, 0, 1, 1};
	over.clip = over.destination_rect;
	over.blend = SurfaceBlend::SourceOver;
	check(operations.execute({over}) == RV_RESULT_OK,
		"premultiplied source-over must operate on an initialized destination");
	TextureSurfaceSnapshot over_surface{};
	check(store.inspect(402, &over_surface) == RV_RESULT_OK &&
		device.top_left_pixel(over_surface.active.texture, 0, 0) == std::array<uint8_t, 4>{{100, 0, 127, 255}},
		"source-over must match the independent premultiplied alpha result");

	check(store.create_surface(500, 3, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		upload_pixels(store, 500, 3, 2, std::vector<uint8_t>(source_pixels.begin(), source_pixels.end())),
		"self-copy source must initialize");
	const uint64_t before_self_copy = store.resident_bytes();
	SurfaceCommand overlap{};
	overlap.opcode = RV_OP_COPY;
	overlap.source = 500;
	overlap.destination = 500;
	overlap.source_rect = SurfaceRect{0, 0, 2, 2};
	overlap.destination_rect = SurfaceRect{1, 0, 2, 2};
	overlap.clip = SurfaceRect{0, 0, 3, 2};
	overlap.blend = SurfaceBlend::Replace;
	check(operations.execute({overlap}) == RV_RESULT_OK && renderer.copy_count == 1 &&
		store.resident_bytes() == before_self_copy,
		"overlapping self-copy must use and retire one budgeted snapshot");
	TextureSurfaceSnapshot copied{};
	check(store.inspect(500, &copied) == RV_RESULT_OK &&
		device.top_left_pixel(copied.active.texture, 0, 0)[0] == 10 &&
		device.top_left_pixel(copied.active.texture, 1, 0)[0] == 10 &&
		device.top_left_pixel(copied.active.texture, 2, 0)[0] == 20 &&
		device.top_left_pixel(copied.active.texture, 0, 1)[0] == 40 &&
		device.top_left_pixel(copied.active.texture, 1, 1)[0] == 40 &&
		device.top_left_pixel(copied.active.texture, 2, 1)[0] == 50,
		"overlapping self-copy must read from the original snapshot, not feedback");

	check(store.create_surface(600, 3, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"preflight target must allocate");
	const uint32_t fills_before_invalid = renderer.fill_count;
	SurfaceCommand invalid_dependency{};
	invalid_dependency.opcode = RV_OP_BLIT;
	invalid_dependency.source = 999;
	invalid_dependency.destination = 600;
	invalid_dependency.source_rect = SurfaceRect{0, 0, 1, 1};
	invalid_dependency.destination_rect = SurfaceRect{0, 0, 1, 1};
	invalid_dependency.clip = SurfaceRect{0, 0, 3, 2};
	const std::vector<SurfaceCommand> invalid_sequence{fill, invalid_dependency};
	check(operations.execute(invalid_sequence) == RV_RESULT_STALE_HANDLE &&
		renderer.fill_count == fills_before_invalid,
		"invalid later dependencies must fail before earlier commands mutate surfaces");
	SurfaceCommand malformed_command = fill;
	malformed_command.clip.height = -1;
	check(operations.execute({fill, malformed_command}) == RV_RESULT_INVALID_ARGUMENT &&
		renderer.fill_count == fills_before_invalid,
		"typed operation callers must also reject malformed later commands before drawing");
	TextureSurfaceSnapshot untouched{};
	check(store.inspect(400, &untouched) == RV_RESULT_OK && untouched.initialized,
		"preflight must not retire or corrupt unrelated resources");
	check(store.create_surface(700, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"destroy-order fixture must allocate");
	SurfaceCommand destroy{};
	destroy.opcode = RV_OP_DESTROY_SURFACE;
	destroy.destination = 700;
	SurfaceCommand after_destroy = fill;
	after_destroy.destination = 700;
	check(operations.execute({destroy, after_destroy}) == RV_RESULT_STALE_HANDLE &&
		store.inspect(700, &untouched) == RV_RESULT_OK,
		"destroy followed by use must be rejected without applying the packet");

	check(store.create_surface(800, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		store.create_surface(801, 1, 1, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"ordered packet surfaces must allocate");
	auto ordered_packet = make_surface_packet({
		{RV_OP_FILL, fill_payload(800, SurfaceRect{0, 0, 1, 1}, 0xFFFF0000u,
			SurfaceRect{0, 0, 1, 1})},
		{RV_OP_BLIT, blit_payload(800, 801, SurfaceRect{0, 0, 1, 1},
			SurfaceRect{0, 0, 1, 1}, SurfaceRect{0, 0, 1, 1}, 0)}});
	ValidatedPacket ordered_validated{};
	std::vector<SurfaceCommand> ordered_commands;
	const RvResult ordered_decode = validate_packet(ordered_packet.data(),
		static_cast<uint32_t>(ordered_packet.size()), 7, &ordered_validated);
	check(ordered_decode == RV_RESULT_OK && decode_surface_commands(ordered_packet.data(),
		static_cast<uint32_t>(ordered_packet.size()), ordered_validated, &ordered_commands) == RV_RESULT_OK &&
		operations.execute(ordered_commands) == RV_RESULT_OK,
		"ordered fill then blit packet must execute as one validated sequence");
	TextureSurfaceSnapshot ordered_destination{};
	check(store.inspect(801, &ordered_destination) == RV_RESULT_OK &&
		device.top_left_pixel(ordered_destination.active.texture, 0, 0) ==
		std::array<uint8_t, 4>{{255, 0, 0, 255}},
		"a later command must see the initialized result of an earlier command");
}

void test_texture_store()
{
	using namespace revival::graphics;
	FakeTextureDevice device;
	TextureStore store(device, 64);
	device.owner_result = RV_RESULT_WRONG_THREAD;
	check(store.create_surface(40, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_WRONG_THREAD &&
		store.resident_bytes() == 0 && device.textures.empty(),
		"surface mutation must stop before touching state from a non-owner thread");
	device.owner_result = RV_RESULT_OK;
	check(store.create_surface(41, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"bounded surface creation must allocate texture and framebuffer");
	TextureSurfaceSnapshot snapshot{};
	check(store.inspect(41, &snapshot) == RV_RESULT_OK && !snapshot.initialized &&
		store.resident_bytes() == 16,
		"new storage must be accounted but remain unavailable until upload completes");
	const uint32_t original_texture = snapshot.active.texture;
	RvFrameDesc desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	desc.physical_stride_bytes = 12;
	const std::array<uint8_t, 24> pixels{{
		200, 120, 80, 128, 4, 5, 6, 0, 90, 91, 92, 93,
		16, 64, 255, 0, 20, 30, 40, 255, 94, 95, 96, 97
	}};
	check(store.begin_upload(41, desc, static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK,
		"full upload must allocate a private staging texture within budget");
	check(store.upload_rows(41, 0, 1, 12, pixels.data(), 12) == RV_RESULT_OK,
		"first padded row chunk must upload");
	check(store.inspect(41, &snapshot) == RV_RESULT_OK &&
		snapshot.active.texture == original_texture && !snapshot.initialized &&
		snapshot.pending_rows == 1,
		"an incomplete upload must not replace the visible resource");
	check(store.end_upload(41) == RV_RESULT_BAD_PACKET,
		"commit must fail while any row is missing");
	check(store.upload_rows(41, 0, 1, 12, pixels.data(), 12) == RV_RESULT_BAD_PACKET,
		"overlapping row chunks must be rejected");
	check(store.upload_rows(41, 1, 1, 12, pixels.data() + 12, 12) == RV_RESULT_OK,
		"final padded row chunk must upload");
	check(store.end_upload(41) == RV_RESULT_OK,
		"complete staged upload must atomically become active");
	check(store.inspect(41, &snapshot) == RV_RESULT_OK && snapshot.initialized &&
		snapshot.active.texture != original_texture && !snapshot.upload_pending &&
		store.resident_bytes() == 16,
		"replacement must retire old storage and leave only the active texture accounted");
	check(device.top_left_pixel(snapshot.active.texture, 0, 0) ==
		std::array<uint8_t, 4>{{100, 60, 40, 128}} &&
		device.top_left_pixel(snapshot.active.texture, 1, 0) ==
		std::array<uint8_t, 4>{{0, 0, 0, 0}} &&
		device.top_left_pixel(snapshot.active.texture, 0, 1) ==
		std::array<uint8_t, 4>{{0, 0, 0, 0}} &&
		device.top_left_pixel(snapshot.active.texture, 1, 1) ==
		std::array<uint8_t, 4>{{20, 30, 40, 255}},
		"staged rows must preserve top-left order and canonical premultiplied pixels");

	const uint32_t committed_texture = snapshot.active.texture;
	check(store.begin_upload(41, desc, static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK &&
		store.upload_rows(41, 0, 1, 12, pixels.data(), 12) == RV_RESULT_OK,
		"replacement upload can begin without changing the active texture");
	check(store.inspect(41, &snapshot) == RV_RESULT_OK &&
		snapshot.active.texture == committed_texture && snapshot.initialized,
		"a new partial replacement must retain the prior committed image");
	check(store.abort_upload(41) == RV_RESULT_OK && store.resident_bytes() == 16,
		"aborting replacement must free staging bytes only");

	device.fail_upload = true;
	check(store.begin_upload(41, desc, static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK &&
		store.upload_rows(41, 0, 1, 12, pixels.data(), 12) == RV_RESULT_GL_FAILURE,
		"graphics upload failure must be reported");
	check(store.inspect(41, &snapshot) == RV_RESULT_OK &&
		snapshot.active.texture == committed_texture && !snapshot.upload_pending &&
		store.resident_bytes() == 16,
		"failed staging upload must be discarded without changing the active texture");
	check(store.destroy_surface(41) == RV_RESULT_OK && store.resident_bytes() == 0 &&
		device.textures.empty(), "surface retirement must release texture and framebuffer storage");
	check(store.destroy_surface(41) == RV_RESULT_STALE_HANDLE,
		"retired surface handle must become stale");
	const RvFrameDesc first_frame_desc = frame_desc(
		RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	const std::array<uint8_t, 16> first_frame_pixels{{
		255, 0, 0, 255, 0, 255, 0, 255,
		0, 0, 255, 255, 255, 255, 255, 255
	}};
	check(store.create_upload_surface(42, first_frame_desc,
		static_cast<uint32_t>(first_frame_pixels.size())) == RV_RESULT_OK &&
		store.resident_bytes() == 16 &&
		store.upload_rows(42, 0, first_frame_desc.physical_height,
			first_frame_desc.physical_stride_bytes, first_frame_pixels.data(),
			static_cast<uint32_t>(first_frame_pixels.size())) == RV_RESULT_OK &&
		store.end_upload(42) == RV_RESULT_OK && store.inspect(42, &snapshot) == RV_RESULT_OK &&
		snapshot.initialized && store.resident_bytes() == 16,
		"a new completed frame must upload into one staged allocation without a redundant empty texture");
	check(store.destroy_surface(42) == RV_RESULT_OK && store.resident_bytes() == 0,
		"single-allocation frame upload storage must retire cleanly after the committed copy is made");

	TextureStore too_small(device, 31);
	check(too_small.create_surface(1, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		too_small.begin_upload(1, frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT), 16) ==
		RV_RESULT_RESOURCE_LIMIT && too_small.resident_bytes() == 16,
		"texture budget must include both active and staged generations");
	too_small.clear();

	FakeTextureDevice limited_device;
	limited_device.max_dimension = 8;
	TextureStore dimension_limited(limited_device, 4096);
	check(dimension_limited.create_surface(2, 5, 1, RV_SCALE_2X) == RV_RESULT_RESOURCE_LIMIT,
		"texture dimensions above the queried device limit must fail before allocation");
	limited_device.fail_create = true;
	check(dimension_limited.create_surface(3, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_GL_FAILURE &&
		dimension_limited.resident_bytes() == 0,
		"failed framebuffer allocation must leave no resource accounting behind");
	FakeTextureDevice retirement_device;
	TextureStore retirement_store(retirement_device, 64);
	const RvFrameDesc retirement_desc = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_STRAIGHT);
	check(retirement_store.create_surface(4, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK &&
		retirement_store.begin_upload(4, retirement_desc, 16) == RV_RESULT_OK &&
		retirement_store.destroy_surface(4) == RV_RESULT_OK &&
		retirement_store.resident_bytes() == 0 && retirement_device.textures.empty(),
		"retiring a surface with staged data must release both generations");
}

void test_scale2x_reference()
{
	using namespace revival::graphics;
	const auto run_center_case = [](uint8_t north, uint8_t west, uint8_t east,
		uint8_t south, uint32_t subpixel_x, uint32_t subpixel_y) {
		std::array<uint8_t, 36> pixels{};
		for(size_t index = 0; index < 9; ++index) pixels[index * 4 + 3] = 255;
		pixels[(1 * 3 + 1) * 4] = 50;
		pixels[(0 * 3 + 1) * 4] = north;
		pixels[(1 * 3 + 0) * 4] = west;
		pixels[(1 * 3 + 2) * 4] = east;
		pixels[(2 * 3 + 1) * 4] = south;
		std::vector<uint8_t> output;
		const RvResult status = scale2x_reference_rgba(
			pixels.data(), pixels.size(), 3, 3, &output);
		if(status != RV_RESULT_OK || output.size() != 144) return false;
		const size_t target_offset =
			(static_cast<size_t>(2 + subpixel_y) * 6 + 2 + subpixel_x) * 4;
		return output[target_offset] == 10 &&
			output[target_offset + 3] == 255;
	};
	check(run_center_case(10, 10, 20, 30, 0, 0),
		"Scale2x must copy the matching northwest neighbors into the upper-left subpixel");
	check(run_center_case(10, 20, 10, 30, 1, 0),
		"Scale2x must copy the matching north/east neighbors into the upper-right subpixel");
	check(run_center_case(20, 10, 30, 10, 0, 1),
		"Scale2x must copy the matching west/south neighbors into the lower-left subpixel");
	check(run_center_case(20, 30, 10, 10, 1, 1),
		"Scale2x must copy the matching south/east neighbors into the lower-right subpixel");

	const std::array<uint8_t, 4> single_pixel{{0, 0, 0, 0}};
	std::vector<uint8_t> one_by_one;
	check(scale2x_reference_rgba(single_pixel.data(), single_pixel.size(), 1, 1, &one_by_one) ==
		RV_RESULT_OK && one_by_one == std::vector<uint8_t>{0, 0, 0, 0, 0, 0, 0, 0,
			0, 0, 0, 0, 0, 0, 0, 0},
		"Scale2x must clamp all four neighbors at a transparent one-pixel edge");
	std::array<uint8_t, 36> alpha_distinction{};
	for(size_t index = 0; index < 9; ++index) alpha_distinction[index * 4 + 3] = 255;
	alpha_distinction[(1 * 3 + 1) * 4] = 50;
	alpha_distinction[(0 * 3 + 1) * 4] = 10;
	alpha_distinction[(0 * 3 + 1) * 4 + 3] = 128;
	alpha_distinction[(1 * 3 + 0) * 4] = 10;
	alpha_distinction[(1 * 3 + 2) * 4] = 20;
	alpha_distinction[(2 * 3 + 1) * 4] = 30;
	std::vector<uint8_t> alpha_output;
	check(scale2x_reference_rgba(alpha_distinction.data(), alpha_distinction.size(), 3, 3,
		&alpha_output) == RV_RESULT_OK && alpha_output.size() == 144 &&
		alpha_output[(2 * 6 + 2) * 4] == 50 && alpha_output[(2 * 6 + 2) * 4 + 3] == 255,
		"Scale2x color equality must include alpha when deciding whether to copy a neighbor");
	check(scale2x_reference_rgba(nullptr, 0, 1, 1, &one_by_one) == RV_RESULT_INVALID_ARGUMENT,
		"Scale2x must reject a missing source buffer");
	check(scale2x_reference_rgba(single_pixel.data(), 3, 1, 1, &one_by_one) == RV_RESULT_INVALID_ARGUMENT,
		"Scale2x must reject a source byte count that does not match its dimensions");
	check(scale2x_reference_rgba(single_pixel.data(), single_pixel.size(), UINT32_MAX, UINT32_MAX,
		&one_by_one) == RV_RESULT_RESOURCE_LIMIT,
		"Scale2x must reject oversized dimensions before multiplying their byte count");
}

void test_temporary_texture_budget()
{
	using namespace revival::graphics;
	FakeTextureDevice device;
	TextureStore store(device, 64);
	check(store.create_surface(901, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"temporary texture fixture must create its bounded source surface");
	TextureAllocation temporary{};
	check(store.reserve_temporary(3, 2, &temporary) == RV_RESULT_OK &&
		temporary.width == 3 && temporary.height == 2 && store.resident_bytes() == 40,
		"temporary allocation must charge its actual physical dimensions to the context budget");
	check(store.release_scratch(temporary) == RV_RESULT_OK && store.resident_bytes() == 16,
		"temporary allocation retirement must return its bytes to the context budget");
	check(store.reserve_temporary(5, 3, &temporary) == RV_RESULT_RESOURCE_LIMIT &&
		temporary.texture == 0 && store.resident_bytes() == 16,
		"temporary allocation must fail before exceeding the configured budget");
	device.max_dimension = UINT32_MAX;
	check(store.reserve_temporary(UINT32_MAX, UINT32_MAX, &temporary) == RV_RESULT_RESOURCE_LIMIT &&
		temporary.texture == 0 && store.resident_bytes() == 16,
		"temporary allocation must reject byte-size overflow before asking the device to allocate");
	check(store.destroy_surface(901) == RV_RESULT_OK && store.resident_bytes() == 0,
		"temporary texture accounting must leave the source lifecycle intact");
}

void test_frame_and_overlay_mailboxes()
{
	using namespace revival::graphics;
	CompletedFrameMailbox frames(48, 2);
	RvFrameDesc descriptor = valid_frame();
	std::array<uint8_t, 16> pixels{};
	const auto submit_frame = [&](uint8_t marker) {
		pixels.fill(marker);
		descriptor.frame_sequence = marker;
		return frames.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(),
			static_cast<uint32_t>(pixels.size()));
	};
	check(submit_frame(1) == RV_RESULT_OK && submit_frame(2) == RV_RESULT_OK &&
		submit_frame(3) == RV_RESULT_OK,
		"completed frame mailbox must accept valid owned frames within its configured per-frame cap");
	FrameMailboxSnapshot frame_stats = frames.snapshot();
	check(frame_stats.pending_frames == 2 && frame_stats.queued_bytes == 32 &&
		frame_stats.frames_replaced == 1,
		"completed frame mailbox must replace only the oldest unconsumed complete frame at capacity");
	CompletedFrame next{};
	bool found = false;
	check(frames.try_pop(&next, &found) == RV_RESULT_OK && found &&
		next.descriptor.frame_sequence == 2 && next.pixels.front() == 2,
		"mailbox replacement must preserve the order and owned bytes of remaining complete frames");
	check(frames.snapshot().queued_bytes == 16 && frames.snapshot().inflight_bytes == 16 &&
		frames.snapshot().inflight_frames == 1,
		"consuming a frame must keep its owned bytes reserved until upload and commit finish");
	pixels.fill(4);
	descriptor.frame_sequence = 4;
	check(frames.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK,
		"a frame producer may enqueue after the render thread consumes one frame");
	check(frames.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_BAD_PACKET,
		"duplicate or older frame sequence numbers must not re-enter the completed-frame queue");
	check(frames.snapshot().queued_bytes == 16 && frames.snapshot().inflight_bytes == 16 &&
		frames.snapshot().frames_replaced == 2,
		"new frames may replace queued frames without exceeding the in-flight memory cap");
	RvFrameDesc invalid = descriptor;
	invalid.physical_stride_bytes = UINT32_MAX;
	check(frames.submit(&invalid, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_RESOURCE_LIMIT &&
		frames.snapshot().pending_frames == 1 && frames.snapshot().inflight_frames == 1,
		"invalid oversized frame must not displace valid frames in the mailbox");
	frames.begin_close();
	check(frames.try_pop(&next, &found) == RV_RESULT_CLOSING && !found &&
		frames.snapshot().queued_bytes == 0,
		"closing the frame mailbox must release pending bytes and reject further consumption");
	check(frames.release_inflight(std::move(next)) == RV_RESULT_OK && frames.snapshot().inflight_bytes == 0,
		"closing must leave consumer-owned bytes reserved until the consumer releases them");
	descriptor.frame_sequence = 5;
	check(frames.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_CLOSING,
		"closing must reject new frames after releasing queued copies");

	CompletedFrameMailbox reuse(16, 1);
	check(reuse.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		reuse.try_pop(&next, &found) == RV_RESULT_OK && found, "first pooled frame must be readable");
	const auto* allocation = next.pixels.data();
	check(reuse.release_inflight(std::move(next)) == RV_RESULT_OK, "retire the first frame");
	descriptor.frame_sequence++;
	pixels.fill(8);
	check(reuse.submit(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		reuse.try_pop(&next, &found) == RV_RESULT_OK && found && next.pixels.data() == allocation &&
		next.pixels.front() == 8, "same-size reuse keeps owned copies within the original byte cap");

	OverlayMailbox overlays(48);
	descriptor = valid_frame();
	pixels.fill(5);
	check(overlays.submit(0, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		overlays.submit(1, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK,
		"overlay mailbox must own both independent generic overlay slots");
	pixels.fill(6);
	descriptor.frame_sequence = 6;
	check(overlays.submit(0, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		overlays.snapshot().queued_bytes == 32 && overlays.snapshot().pending_updates == 2,
		"a newer update must replace only its own pending overlay slot");
	std::array<OverlayUpdate, 2> updates{};
	check(overlays.drain(&updates) == RV_RESULT_OK && updates[0].pending && updates[1].pending &&
		updates[0].pixels.front() == 6 && updates[1].pixels.front() == 5 &&
		overlays.snapshot().queued_bytes == 0 && overlays.snapshot().inflight_bytes == 32,
		"draining overlays must transfer owned updates without mixing slots or releasing their budget early");
	updates = {};
	check(overlays.release_inflight(32, 2) == RV_RESULT_OK,
		"the renderer releases overlay bytes after each update has been applied or rejected");
	descriptor.physical_width = descriptor.physical_height = 0;
	descriptor.physical_stride_bytes = descriptor.logical_width = descriptor.logical_height = 0;
	check(overlays.submit(1, &descriptor, RV_FRAME_DESC_BYTES, nullptr, 0) == RV_RESULT_OK,
		"an empty versioned descriptor must clear a generic overlay slot");
	check(overlays.drain(&updates) == RV_RESULT_OK && updates[1].pending && updates[1].clear,
		"overlay clear must arrive as an explicit slot update");
	check(overlays.release_inflight(0, 1) == RV_RESULT_OK,
		"a clear update releases its slot count without holding pixel bytes");
	check(overlays.submit(2, &descriptor, RV_FRAME_DESC_BYTES, nullptr, 0) == RV_RESULT_INVALID_ARGUMENT,
		"overlay submissions must reject slots outside the two-slot ABI");
	descriptor = valid_frame();
	OverlayMailbox bounded_overlays(32);
	check(bounded_overlays.submit(0, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		bounded_overlays.try_pop(&updates[0], &found) == RV_RESULT_OK && found &&
		bounded_overlays.submit(1, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_OK &&
		bounded_overlays.submit(0, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) == RV_RESULT_RESOURCE_LIMIT &&
		bounded_overlays.snapshot().inflight_bytes == 16,
		"in-flight HUD uploads must remain inside the context byte cap while producers submit newer overlays");
	std::vector<uint8_t>().swap(updates[0].pixels);
	check(bounded_overlays.release_inflight(16, 1) == RV_RESULT_OK,
		"the bounded overlay mailbox releases its in-flight allocation after consumer ownership ends");

	RvAttachConfig config = config_for(99, RV_INPUT_MODE_COMPLETED_FRAMES);
	config.max_queued_bytes = 32;
	config.max_pending_frames = 1;
	int context_token = 0;
	int procedure_token = 0;
	GlFunctionTable functions{};
	functions.procedures.fill(&procedure_token);
	ContextSession session(99, config, std::this_thread::get_id(), &context_token, functions);
	descriptor = valid_frame();
	pixels.fill(7);
	check(session.submit_completed_frame(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) ==
		RV_RESULT_OK && session.submit_overlay(0, &descriptor, RV_FRAME_DESC_BYTES,
			pixels.data(), 16) == RV_RESULT_OK,
		"a context must share its queued-byte allowance between completed frames and HUDs");
	pixels.fill(8);
	descriptor.frame_sequence = 8;
	check(session.submit_completed_frame(&descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) ==
		RV_RESULT_OK && session.completed_frames().snapshot().frames_replaced == 1,
		"a full-frame slot may be replaced without dropping pending HUD updates");
	check(session.submit_overlay(1, &descriptor, RV_FRAME_DESC_BYTES, pixels.data(), 16) ==
		RV_RESULT_RESOURCE_LIMIT && session.overlays().snapshot().pending_updates == 1,
		"separate HUD slots must not let a context exceed its aggregate queued-byte budget");
	session.begin_close();
	check(session.completed_frames().snapshot().queued_bytes == 0 &&
		session.overlays().snapshot().queued_bytes == 0,
		"context close must release both frame and HUD mailbox copies");
}

void test_committed_frame_and_viewport()
{
	using namespace revival::graphics;
	FakeTextureDevice device;
	TextureStore store(device, 512);
	check(store.create_surface(500, 2, 2, RV_SCALE_ORIGINAL) == RV_RESULT_OK,
		"frame presenter fixture must create a native-size source");
	RvFrameDesc descriptor = valid_frame();
	std::array<uint8_t, 16> pixels{};
	check(store.begin_upload(500, descriptor, static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK &&
		store.upload_rows(500, 0, 2, 8, pixels.data(), static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK &&
		store.end_upload(500) == RV_RESULT_OK,
		"frame presenter source must become a complete initialized generation");
	TextureSurfaceSnapshot source{};
	check(store.inspect(500, &source) == RV_RESULT_OK && source.generation == 1,
		"a complete upload must receive a monotonic surface generation");
	FakeSurfaceScaler scaler(store);
	FramePresenter presenter(store, scaler);
	ScaleReport scale_report{};
	const SurfaceRect full{0, 0, 2, 2};
	check(presenter.commit(500, full, 1, RV_SCALE_2X, 501, &scale_report) == RV_RESULT_OK,
		"first frame commit must create an independent scaled snapshot");
	CommittedFrameSnapshot first{};
	check(presenter.current(&first) == RV_RESULT_OK && first.surface == 501 &&
		first.source_generation == 1 && first.storage_scale == RV_SCALE_2X,
		"committed frame must retain source generation, density and logical surface identity");
	CommittedFrameSnapshot second{};
	scaler.fail_next = true;
	check(presenter.commit(500, full, 2, RV_SCALE_2X, 502, &scale_report) == RV_RESULT_GL_FAILURE &&
		presenter.current(&second) == RV_RESULT_OK && second.surface == first.surface &&
		second.frame_sequence == first.frame_sequence,
		"a failed back-buffer render must leave the previous complete frame visible");
	check(presenter.commit(500, full, 2, RV_SCALE_2X, 502, &scale_report) == RV_RESULT_OK,
		"second commit must prepare a back surface without mutating the displayed snapshot");
	check(presenter.current(&second) == RV_RESULT_OK && second.surface == 502,
		"new complete frame must become visible only after its snapshot has finished");
	TextureSurfaceSnapshot old_front{};
	check(store.inspect(second.surface, &old_front) == RV_RESULT_OK,
		"the current front surface must remain inspectable");
	check(presenter.commit(500, full, 3, RV_SCALE_2X, 503, &scale_report) == RV_RESULT_OK &&
		store.inspect(second.surface, &source) == RV_RESULT_OK &&
		source.generation == old_front.generation,
		"a later back-buffer write must leave the displayed front surface unchanged");
	check(presenter.commit(500, full, 3, RV_SCALE_2X, 504, &scale_report) == RV_RESULT_BAD_PACKET &&
		presenter.current(&second) == RV_RESULT_OK && second.frame_sequence == 3,
		"out-of-order frame commits must be rejected without replacing the current frame");
	check(presenter.commit(500, SurfaceRect{1, 1, 2, 2}, 4, RV_SCALE_2X, 505,
		&scale_report) == RV_RESULT_INVALID_ARGUMENT,
		"frame commits must reject content rectangles outside their logical source");
	presenter.clear();
	check(presenter.current(&second) == RV_RESULT_STALE_HANDLE,
		"clear-frame must hide presentation without invalidating cached game surfaces");
	check(presenter.commit(500, full, 4, RV_SCALE_2X, 506, &scale_report) == RV_RESULT_OK &&
		store.resident_bytes() == 144,
		"frame buffers must remain reusable after clear without leaking replaced snapshot textures");
	presenter.destroy();
	check(store.destroy_surface(500) == RV_RESULT_OK && store.resident_bytes() == 0,
		"frame presenter teardown must retire both snapshots and leave unrelated sources intact");
	check(store.create_surface(510, 1, 1, RV_SCALE_4X) == RV_RESULT_OK,
		"already detailed sources must use their declared physical density");
	RvFrameDesc detailed = frame_desc(RV_PIXEL_FORMAT_RGBA8, RV_ALPHA_OPAQUE);
	detailed.frame_sequence = 5;
	detailed.physical_width = 4;
	detailed.physical_height = 4;
	detailed.physical_stride_bytes = 16;
	detailed.logical_width = 1;
	detailed.logical_height = 1;
	detailed.source_scale = RV_SCALE_4X;
	detailed.desired_scale = RV_SCALE_2X;
	std::array<uint8_t, 64> detailed_pixels{};
	check(store.begin_upload(510, detailed, static_cast<uint32_t>(detailed_pixels.size())) == RV_RESULT_OK &&
		store.upload_rows(510, 0, 4, 16, detailed_pixels.data(),
			static_cast<uint32_t>(detailed_pixels.size())) == RV_RESULT_OK &&
		store.end_upload(510) == RV_RESULT_OK,
		"the 4x frame fixture must upload with explicit logical and physical dimensions");
	FramePresenter detailed_presenter(store, scaler);
	check(detailed_presenter.commit(510, SurfaceRect{0, 0, 1, 1}, 5,
		RV_SCALE_2X, 511, &scale_report) == RV_RESULT_OK &&
		detailed_presenter.current(&second) == RV_RESULT_OK && second.storage_scale == RV_SCALE_4X &&
		second.logical_width == 1 && second.logical_height == 1,
		"a prepared 4x frame must remain 4x when the selected detail target is 2x");
	detailed_presenter.destroy();
	check(store.destroy_surface(510) == RV_RESULT_OK && store.resident_bytes() == 0,
		"already detailed frame snapshots must retire within the same texture budget");

	RvViewport viewport{};
	viewport.struct_bytes = RV_VIEWPORT_BYTES;
	viewport.abi_version = RV_ABI_VERSION;
	viewport.framebuffer_width = 1920;
	viewport.framebuffer_height = 1080;
	viewport.aspect_num = 4;
	viewport.aspect_den = 3;
	viewport.filter = RV_FILTER_NEAREST;
	viewport.fit = RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT;
	FrameViewportRect rectangle{};
	check(calculate_viewport_rect(viewport, &rectangle) == RV_RESULT_OK &&
		rectangle.x == 240 && rectangle.y == 0 && rectangle.width == 1440 && rectangle.height == 1080,
		"viewport math must preserve a 4:3 frame and center it inside a 16:9 desktop buffer");
	viewport.aspect_num = 16;
	viewport.aspect_den = 9;
	check(calculate_viewport_rect(viewport, &rectangle) == RV_RESULT_OK &&
		rectangle.x == 0 && rectangle.y == 0 && rectangle.width == 1920 && rectangle.height == 1080,
		"viewport math must fill a matching 16:9 framebuffer");
	viewport.filter = 0;
	check(calculate_viewport_rect(viewport, &rectangle) == RV_RESULT_INVALID_ARGUMENT,
		"viewport validation must reject an unknown texture filter");
}

void test_context_registry()
{
	using namespace revival::graphics;
	ContextRegistry registry;
	RvAttachConfig first_config = config_for(7);
	RvAttachConfig second_config = config_for(8);
	int first_identity = 0;
	int second_identity = 0;
	int gl_token = 0;
	GlFunctionTable functions{};
	functions.procedures.fill(&gl_token);
	RvContextHandle first_handle = 0;
	RvContextHandle second_handle = 0;
	const auto owner = std::this_thread::get_id();
	check(registry.create(first_config, owner, &first_identity, functions, &first_handle) == RV_RESULT_OK,
		"valid context must register");
	check(registry.create(second_config, owner, &second_identity, functions, &second_handle) == RV_RESULT_OK,
		"second context must register independently");
	check(first_handle != 0 && second_handle != 0 && first_handle != second_handle,
		"context handles must be unique and nonzero");
	auto first = registry.find(first_handle);
	auto second = registry.find(second_handle);
	check(first && second, "registered contexts must be discoverable");
	if(first && second) {
		check(first->check_owner(owner, &first_identity) == RV_RESULT_OK,
			"matching owner thread and current context must pass");
		check(first->check_owner(std::thread::id{}, &first_identity) == RV_RESULT_WRONG_THREAD,
			"non-owner thread must be rejected");
		check(first->check_owner(owner, &second_identity) == RV_RESULT_NO_CONTEXT,
			"different GL context must be rejected");
		check(first->allocate_resource_id() == (UINT64_C(1) << 63) &&
			first->allocate_resource_id() == ((UINT64_C(1) << 63) + 1) &&
			second->allocate_resource_id() == (UINT64_C(1) << 63),
			"resource IDs must have an independent namespace per context");
	}
	check(registry.begin_close(first_handle) == RV_RESULT_OK,
		"begin-close must be callable without the GL owner thread");
	check(first && first->queue().is_closing(), "registered queue must enter closing state");
	check(registry.destroy_on_owner(first_handle, std::thread::id{}, &first_identity) ==
		RV_RESULT_WRONG_THREAD, "destroy from non-owner thread must fail");
	check(registry.destroy_on_owner(first_handle, owner, &second_identity) == RV_RESULT_NO_CONTEXT,
		"destroy with a different current context must fail");
	check(registry.destroy_on_owner(first_handle, owner, &first_identity) == RV_RESULT_OK,
		"destroy on matching owner/current context must succeed");
	check(!registry.find(first_handle) && registry.begin_close(first_handle) == RV_RESULT_STALE_HANDLE,
		"destroyed handle must become stale");
	check(registry.destroy_on_owner(second_handle, owner, &second_identity) == RV_RESULT_OK,
		"second context must destroy independently");

	RvAttachConfig invalid = config_for(9);
	invalid.max_queued_ops = RV_MAX_QUEUED_OPS + 1;
	RvContextHandle not_created = 99;
	check(registry.create(invalid, owner, &first_identity, functions, &not_created) ==
		RV_RESULT_INVALID_ARGUMENT && not_created == 0,
		"invalid context budget must fail without returning a handle");
	functions.procedures[2] = nullptr;
	RvAttachConfig valid = config_for(10);
	check(registry.create(valid, owner, &first_identity, functions, &not_created) ==
		RV_RESULT_GL_FAILURE && not_created == 0,
		"context must not register with an incomplete GL function table");
}

} // namespace

int main()
{
	test_packet_queue();
	test_frame_queue_and_close();
	test_close_race();
	test_gl_loader();
	test_pixel_transfer();
	test_surface_protocol();
	test_generic_command_protocol();
	test_surface_operations();
	test_texture_store();
	test_scale2x_reference();
	test_temporary_texture_budget();
	test_frame_and_overlay_mailboxes();
	test_committed_frame_and_viewport();
	test_context_registry();
	if(failures != 0) {
		std::cerr << failures << " native graphics contract check(s) failed.\n";
		return 1;
	}
	std::cout << "Native graphics CPU contract checks passed.\n";
	return 0;
}
