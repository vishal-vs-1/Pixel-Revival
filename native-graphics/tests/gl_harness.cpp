#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <Windows.h>
#include <GL/gl.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <iostream>
#include <utility>
#include <vector>

#include "gl_functions.hpp"
#include "diagnostics.hpp"
#include "frame_presenter.hpp"
#include "context.hpp"
#include "scale2x.hpp"
#include "surface_device.hpp"
#include "surface_ops.hpp"
#include "texture_store.hpp"

namespace {

constexpr wchar_t kWindowClass[] = L"RevivalGraphicsSurfaceHiddenContext";

using namespace revival::graphics;

LRESULT CALLBACK hidden_window_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam)
{
	return ::DefWindowProcW(window, message, wparam, lparam);
}

int skip(const char* reason)
{
	std::cout << "SKIP: " << reason << '\n';
	return 77;
}

RvFrameDesc make_desc(uint32_t width, uint32_t height, uint64_t sequence)
{
	RvFrameDesc desc{};
	desc.struct_bytes = RV_FRAME_DESC_BYTES;
	desc.abi_version = RV_ABI_VERSION;
	desc.frame_sequence = sequence;
	desc.physical_width = width;
	desc.physical_height = height;
	desc.physical_stride_bytes = width * 4;
	desc.logical_width = width;
	desc.logical_height = height;
	desc.source_scale = RV_SCALE_ORIGINAL;
	desc.desired_scale = RV_SCALE_2X;
	desc.pixel_format = RV_PIXEL_FORMAT_RGBA8;
	desc.alpha_mode = RV_ALPHA_OPAQUE;
	desc.origin = RV_ORIGIN_TOP_LEFT;
	return desc;
}

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

void append_rect(std::vector<uint8_t>& bytes, const SurfaceRect& rectangle)
{
	append_u32(bytes, static_cast<uint32_t>(rectangle.x));
	append_u32(bytes, static_cast<uint32_t>(rectangle.y));
	append_u32(bytes, static_cast<uint32_t>(rectangle.width));
	append_u32(bytes, static_cast<uint32_t>(rectangle.height));
}

std::vector<uint8_t> make_packet(
	const std::vector<std::pair<uint16_t, std::vector<uint8_t>>>& commands,
	uint64_t epoch, uint64_t sequence)
{
	std::vector<uint8_t> packet(RV_PACKET_HEADER_BYTES, 0);
	for(const auto& command : commands) {
		append_u16(packet, command.first);
		append_u16(packet, 0);
		append_u32(packet, RV_COMMAND_HEADER_BYTES + static_cast<uint32_t>(command.second.size()));
		packet.insert(packet.end(), command.second.begin(), command.second.end());
	}
	packet[0] = 'R'; packet[1] = 'V'; packet[2] = 'G'; packet[3] = '1';
	packet[4] = 1; packet[6] = static_cast<uint8_t>(RV_PACKET_HEADER_BYTES);
	for(uint32_t shift = 0; shift < 32; shift += 8) {
		packet[8 + shift / 8] = static_cast<uint8_t>(packet.size() >> shift);
		packet[12 + shift / 8] = static_cast<uint8_t>(commands.size() >> shift);
	}
	for(uint32_t shift = 0; shift < 64; shift += 8) {
		packet[16 + shift / 8] = static_cast<uint8_t>(epoch >> shift);
		packet[24 + shift / 8] = static_cast<uint8_t>(sequence >> shift);
	}
	return packet;
}

std::vector<uint8_t> create_payload(RvSurfaceHandle surface,
	uint32_t width, uint32_t height, uint32_t scale)
{
	std::vector<uint8_t> payload;
	append_u64(payload, surface);
	append_u32(payload, width);
	append_u32(payload, height);
	append_u32(payload, scale);
	return payload;
}

std::vector<uint8_t> begin_upload_payload(RvSurfaceHandle surface,
	const RvFrameDesc& desc, uint32_t bytes)
{
	std::vector<uint8_t> payload;
	append_u64(payload, surface);
	append_u32(payload, desc.struct_bytes);
	append_u32(payload, desc.abi_version);
	append_u64(payload, desc.frame_sequence);
	append_u32(payload, desc.physical_width);
	append_u32(payload, desc.physical_height);
	append_u32(payload, desc.physical_stride_bytes);
	append_u32(payload, desc.logical_width);
	append_u32(payload, desc.logical_height);
	append_u32(payload, desc.source_scale);
	append_u32(payload, desc.desired_scale);
	append_u32(payload, desc.pixel_format);
	append_u32(payload, desc.alpha_mode);
	append_u32(payload, desc.origin);
	append_u32(payload, bytes);
	return payload;
}

std::vector<uint8_t> upload_rows_payload(RvSurfaceHandle surface,
	uint32_t row, const std::vector<uint8_t>& bytes)
{
	std::vector<uint8_t> payload;
	append_u64(payload, surface);
	append_u32(payload, row);
	append_u32(payload, 1);
	append_u32(payload, 16);
	append_u32(payload, static_cast<uint32_t>(bytes.size()));
	payload.insert(payload.end(), bytes.begin(), bytes.end());
	return payload;
}

std::vector<uint8_t> fill_packet_payload(RvSurfaceHandle surface)
{
	std::vector<uint8_t> payload;
	append_u64(payload, surface);
	append_rect(payload, SurfaceRect{0, 0, 1, 1});
	append_u32(payload, 0xFFFF0000u);
	append_rect(payload, SurfaceRect{0, 0, 2, 2});
	return payload;
}

std::vector<uint8_t> commit_payload(RvSurfaceHandle surface, uint64_t sequence)
{
	std::vector<uint8_t> payload;
	append_u64(payload, surface);
	append_u64(payload, sequence);
	append_u32(payload, RV_SCALE_2X);
	append_rect(payload, SurfaceRect{0, 0, 2, 2});
	return payload;
}

bool upload_rgba_surface(TextureStore& store, RvSurfaceHandle handle,
	uint32_t width, uint32_t height, const std::vector<uint8_t>& pixels,
	uint64_t sequence, uint32_t alpha_mode)
{
	if(store.create_surface(handle, width, height, RV_SCALE_ORIGINAL) != RV_RESULT_OK) return false;
	RvFrameDesc desc = make_desc(width, height, sequence);
	desc.alpha_mode = alpha_mode;
	RvResult status = store.begin_upload(handle, desc, static_cast<uint32_t>(pixels.size()));
	if(status == RV_RESULT_OK) {
		status = store.upload_rows(handle, 0, height, width * 4, pixels.data(),
			static_cast<uint32_t>(pixels.size()));
	}
	if(status == RV_RESULT_OK) status = store.end_upload(handle);
	return status == RV_RESULT_OK;
}

bool read_rgba_top_down(TextureStore& store, const GlFunctionTable& functions,
	RvSurfaceHandle handle, std::vector<uint8_t>* out)
{
	TextureSurfaceSnapshot surface{};
	if(out == nullptr || store.inspect(handle, &surface) != RV_RESULT_OK) return false;
	auto bind_framebuffer = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
		functions.get(GlProcedure::BindFramebuffer));
	GLint previous_framebuffer = 0;
	::glGetIntegerv(0x8CA6, &previous_framebuffer);
	bind_framebuffer(0x8D40, surface.active.framebuffer);
	std::vector<uint8_t> bottom_up(static_cast<size_t>(surface.active.width) * surface.active.height * 4);
	::glReadPixels(0, 0, static_cast<GLsizei>(surface.active.width),
		static_cast<GLsizei>(surface.active.height), GL_RGBA, GL_UNSIGNED_BYTE, bottom_up.data());
	const bool gl_ok = ::glGetError() == GL_NO_ERROR;
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	if(!gl_ok) return false;
	out->resize(bottom_up.size());
	const size_t row_bytes = static_cast<size_t>(surface.active.width) * 4;
	for(uint32_t y = 0; y < surface.active.height; ++y) {
		const size_t source_offset = static_cast<size_t>(surface.active.height - 1 - y) * row_bytes;
		const size_t destination_offset = static_cast<size_t>(y) * row_bytes;
		std::copy_n(bottom_up.data() + source_offset, row_bytes, out->data() + destination_offset);
	}
	return true;
}

bool run_transform_readback(TextureStore& store, SurfaceOperations& operations,
	const GlFunctionTable& functions, RvSurfaceHandle source_handle,
	uint32_t transform, uint32_t destination_width, uint32_t destination_height,
	const std::array<uint8_t, 6>& expected_top_down)
{
	const RvSurfaceHandle destination_handle = 100 + transform;
	if(store.create_surface(destination_handle, destination_width, destination_height,
		RV_SCALE_ORIGINAL) != RV_RESULT_OK) return false;
	SurfaceCommand command{};
	command.opcode = RV_OP_BLIT;
	command.source = source_handle;
	command.destination = destination_handle;
	command.source_rect = SurfaceRect{0, 0, 3, 2};
	command.destination_rect = SurfaceRect{0, 0,
		static_cast<int32_t>(destination_width), static_cast<int32_t>(destination_height)};
	command.clip = command.destination_rect;
	command.transform = transform;
	command.blend = SurfaceBlend::Replace;
	if(operations.execute(std::vector<SurfaceCommand>{command}) != RV_RESULT_OK) {
		store.destroy_surface(destination_handle);
		return false;
	}
	std::vector<uint8_t> readback;
	const bool read_ok = read_rgba_top_down(store, functions, destination_handle, &readback);
	bool matches = read_ok;
	for(size_t index = 0; matches && index < expected_top_down.size(); ++index) {
		const size_t offset = index * 4;
		matches = readback[offset] == expected_top_down[index] &&
			readback[offset + 1] == 0 && readback[offset + 2] == 0 && readback[offset + 3] == 255;
	}
	store.destroy_surface(destination_handle);
	return matches;
}

bool test_gpu_surface_transforms(TextureStore& store, SurfaceOperations& operations,
	const GlFunctionTable& functions)
{
	constexpr RvSurfaceHandle source_handle = 10;
	const std::array<uint8_t, 24> pixels{{
		10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255,
		40, 0, 0, 255, 50, 0, 0, 255, 60, 0, 0, 255
	}};
	if(store.create_surface(source_handle, 3, 2, RV_SCALE_ORIGINAL) != RV_RESULT_OK) return false;
	RvResult status = store.begin_upload(source_handle, make_desc(3, 2, 2),
		static_cast<uint32_t>(pixels.size()));
	if(status == RV_RESULT_OK) {
		status = store.upload_rows(source_handle, 0, 2, 12,
			pixels.data(), static_cast<uint32_t>(pixels.size()));
	}
	if(status == RV_RESULT_OK) status = store.end_upload(source_handle);
	if(status != RV_RESULT_OK) {
		store.destroy_surface(source_handle);
		return false;
	}
	const std::array<std::array<uint8_t, 6>, 8> expected{{
		{{10, 20, 30, 40, 50, 60}}, {{40, 50, 60, 10, 20, 30}},
		{{30, 20, 10, 60, 50, 40}}, {{60, 50, 40, 30, 20, 10}},
		{{10, 40, 20, 50, 30, 60}}, {{40, 10, 50, 20, 60, 30}},
		{{30, 60, 20, 50, 10, 40}}, {{60, 30, 50, 20, 40, 10}}
	}};
	bool passed = true;
	for(uint32_t transform = 0; transform < expected.size() && passed; ++transform) {
		const bool quarter_turn = transform >= 4;
		const uint32_t width = quarter_turn ? 2 : 3;
		const uint32_t height = quarter_turn ? 3 : 2;
		passed = run_transform_readback(store, operations, functions, source_handle,
			transform, width, height, expected[transform]);
	}
	store.destroy_surface(source_handle);
	return passed;
}

bool test_gpu_clip_blend_and_self_copy(TextureStore& store,
	SurfaceOperations& operations, const GlFunctionTable& functions)
{
	const RvSurfaceHandle clipped_handle = 20;
	if(store.create_surface(clipped_handle, 2, 1, RV_SCALE_ORIGINAL) != RV_RESULT_OK) return false;
	SurfaceCommand fill{};
	fill.opcode = RV_OP_FILL;
	fill.destination = clipped_handle;
	fill.destination_rect = SurfaceRect{0, 0, 2, 1};
	fill.clip = SurfaceRect{0, 0, 1, 1};
	fill.argb = 0x80FF0000u;
	bool passed = operations.execute(std::vector<SurfaceCommand>{fill}) == RV_RESULT_OK;
	std::vector<uint8_t> pixels;
	passed = passed && read_rgba_top_down(store, functions, clipped_handle, &pixels) &&
		pixels == std::vector<uint8_t>{128, 0, 0, 128, 0, 0, 0, 0};
	store.destroy_surface(clipped_handle);
	if(!passed) return false;

	const RvSurfaceHandle fill_overlap_handle = 24;
	if(!upload_rgba_surface(store, fill_overlap_handle, 1, 1,
		std::vector<uint8_t>{2, 0, 0, 255}, 24, RV_ALPHA_OPAQUE)) {
		store.destroy_surface(fill_overlap_handle);
		return false;
	}
	SurfaceCommand precise_fill{};
	precise_fill.opcode = RV_OP_FILL;
	precise_fill.destination = fill_overlap_handle;
	precise_fill.destination_rect = SurfaceRect{0, 0, 1, 1};
	precise_fill.clip = precise_fill.destination_rect;
	precise_fill.argb = 0x80010000u;
	passed = operations.execute(std::vector<SurfaceCommand>{precise_fill}) == RV_RESULT_OK &&
		read_rgba_top_down(store, functions, fill_overlap_handle, &pixels) &&
		pixels == std::vector<uint8_t>{1, 0, 0, 255};
	store.destroy_surface(fill_overlap_handle);
	if(!passed) return false;

	const RvSurfaceHandle destination_handle = 21;
	const RvSurfaceHandle source_handle = 22;
	if(!upload_rgba_surface(store, destination_handle, 2, 1,
		std::vector<uint8_t>{0, 0, 255, 255, 0, 0, 255, 255}, 21, RV_ALPHA_OPAQUE)) {
		store.destroy_surface(destination_handle);
		return false;
	}
	if(!upload_rgba_surface(store, source_handle, 1, 1,
		std::vector<uint8_t>{200, 0, 0, 128}, 22, RV_ALPHA_STRAIGHT)) {
		store.destroy_surface(source_handle);
		store.destroy_surface(destination_handle);
		return false;
	}
	SurfaceCommand blend{};
	blend.opcode = RV_OP_BLIT;
	blend.source = source_handle;
	blend.destination = destination_handle;
	blend.source_rect = SurfaceRect{0, 0, 1, 1};
	blend.destination_rect = SurfaceRect{0, 0, 1, 1};
	blend.clip = SurfaceRect{0, 0, 2, 1};
	blend.blend = SurfaceBlend::SourceOver;
	passed = operations.execute(std::vector<SurfaceCommand>{blend}) == RV_RESULT_OK &&
		read_rgba_top_down(store, functions, destination_handle, &pixels);
	const auto within_one = [](uint8_t actual, uint8_t expected) {
		return actual >= expected - 1 && actual <= expected + 1;
	};
	if(passed) {
		passed = within_one(pixels[0], 100) && pixels[1] == 0 && within_one(pixels[2], 127) && pixels[3] == 255 &&
			pixels[4] == 0 && pixels[5] == 0 && pixels[6] == 255 && pixels[7] == 255;
	}
	store.destroy_surface(source_handle);
	store.destroy_surface(destination_handle);
	if(!passed) return false;

	const RvSurfaceHandle self_handle = 23;
	if(!upload_rgba_surface(store, self_handle, 2, 1,
		std::vector<uint8_t>{255, 0, 0, 255, 0, 255, 0, 255}, 23, RV_ALPHA_OPAQUE)) {
		store.destroy_surface(self_handle);
		return false;
	}
	SurfaceCommand self_copy{};
	self_copy.opcode = RV_OP_COPY;
	self_copy.source = self_handle;
	self_copy.destination = self_handle;
	self_copy.source_rect = SurfaceRect{0, 0, 1, 1};
	self_copy.destination_rect = SurfaceRect{1, 0, 1, 1};
	self_copy.clip = SurfaceRect{0, 0, 2, 1};
	self_copy.blend = SurfaceBlend::Replace;
	passed = operations.execute(std::vector<SurfaceCommand>{self_copy}) == RV_RESULT_OK &&
		read_rgba_top_down(store, functions, self_handle, &pixels) &&
		pixels == std::vector<uint8_t>{255, 0, 0, 255, 255, 0, 0, 255};
	store.destroy_surface(self_handle);
	return passed;
}

bool test_gpu_scale2x(TextureStore& store, SurfaceOperations& operations,
	const GlFunctionTable& functions)
{
	constexpr RvSurfaceHandle source_handle = 30;
	constexpr RvSurfaceHandle target_2x = 31;
	constexpr RvSurfaceHandle target_4x = 32;
	const std::vector<uint8_t> source{
		10, 0, 0, 255, 10, 0, 0, 255, 20, 0, 0, 255,
		10, 0, 0, 255, 50, 0, 0, 255, 10, 0, 0, 255,
		30, 0, 0, 255, 10, 0, 0, 255, 40, 0, 0, 255};
	if(!upload_rgba_surface(store, source_handle, 3, 3, source, 30, RV_ALPHA_OPAQUE) ||
		store.create_surface(target_2x, 3, 3, RV_SCALE_2X) != RV_RESULT_OK ||
		store.create_surface(target_4x, 3, 3, RV_SCALE_4X) != RV_RESULT_OK) {
		store.destroy_surface(target_4x);
		store.destroy_surface(target_2x);
		store.destroy_surface(source_handle);
		return false;
	}

	std::vector<uint8_t> expected_2x;
	std::vector<uint8_t> expected_4x;
	bool passed = scale2x_reference_rgba(source.data(), source.size(), 3, 3, &expected_2x) == RV_RESULT_OK &&
		scale2x_reference_rgba(expected_2x.data(), expected_2x.size(), 6, 6, &expected_4x) == RV_RESULT_OK;
	Scale2xRenderer renderer(store, operations, functions);
	ScaleReport report{};
	std::vector<uint8_t> actual;
	passed = passed && renderer.apply(source_handle, target_2x, &report) == RV_RESULT_OK &&
		report.path == ScalePath::GpuScale2x && report.passes == 1 &&
		read_rgba_top_down(store, functions, target_2x, &actual) && actual == expected_2x;
	passed = passed && renderer.apply(source_handle, target_4x, &report) == RV_RESULT_OK &&
		report.path == ScalePath::GpuScale2x && report.passes == 2 &&
		read_rgba_top_down(store, functions, target_4x, &actual) && actual == expected_4x;
	passed = passed && renderer.apply(target_2x, target_4x, &report) == RV_RESULT_OK &&
		report.path == ScalePath::GpuScale2x && report.passes == 1 &&
		read_rgba_top_down(store, functions, target_4x, &actual) && actual == expected_4x;
	passed = passed && renderer.apply(target_2x, target_2x, &report) == RV_RESULT_OK &&
		report.path == ScalePath::AlreadyDetailed;
	const Scale2xStatistics statistics = renderer.statistics();
	passed = passed && statistics.completed_gpu_passes == 4 &&
		statistics.gpu_scaled_surfaces == 3 && statistics.compatibility_surfaces == 0 &&
		statistics.shader_failures == 0;
	renderer.destroy();

	GlFunctionTable compatibility_functions = functions;
	for(size_t index = GlFunctionTable::required_count;
		index < GlFunctionTable::procedure_count; ++index) {
		compatibility_functions.procedures[index] = nullptr;
	}
	if(store.create_surface(33, 3, 3, RV_SCALE_2X) != RV_RESULT_OK) passed = false;
	else {
		Scale2xRenderer compatibility_renderer(store, operations, compatibility_functions);
		ScaleReport compatibility_report{};
		passed = passed && compatibility_renderer.apply(source_handle, 33, &compatibility_report) == RV_RESULT_OK &&
			compatibility_report.path == ScalePath::CompatibilitySampling &&
			compatibility_report.fallback_reason == ScaleFallbackReason::ShaderUnavailable;
		compatibility_renderer.destroy();
		store.destroy_surface(33);
	}
	store.destroy_surface(target_4x);
	store.destroy_surface(target_2x);
	store.destroy_surface(source_handle);
	return passed;
}

bool test_gpu_frame_presentation(TextureStore& store, SurfaceOperations& operations,
	const GlFunctionTable& functions)
{
	struct CallerState {
		GLint framebuffer = 0;
		GLint matrix_mode = 0;
		GLint active_texture = 0;
		GLint texture_binding = 0;
		GLint program = 0;
		GLboolean texture_enabled = GL_FALSE;
		std::array<GLfloat, 16> projection{};
		std::array<GLfloat, 16> modelview{};
		std::array<GLfloat, 16> texture_matrix{};
	};
	auto capture_state = [] {
		CallerState state{};
		::glGetIntegerv(0x8CA6, &state.framebuffer);
		::glGetIntegerv(0x0BA0, &state.matrix_mode);
		::glGetIntegerv(0x84E0, &state.active_texture);
		::glGetIntegerv(0x8069, &state.texture_binding);
		::glGetIntegerv(0x8B8D, &state.program);
		state.texture_enabled = ::glIsEnabled(GL_TEXTURE_2D);
		::glGetFloatv(0x0BA7, state.projection.data());
		::glGetFloatv(0x0BA6, state.modelview.data());
		::glGetFloatv(0x0BA8, state.texture_matrix.data());
		return state;
	};
	auto same_state = [](const CallerState& left, const CallerState& right) {
		return left.framebuffer == right.framebuffer && left.matrix_mode == right.matrix_mode &&
			left.active_texture == right.active_texture && left.texture_binding == right.texture_binding &&
			left.program == right.program && left.texture_enabled == right.texture_enabled &&
			left.projection == right.projection && left.modelview == right.modelview &&
			left.texture_matrix == right.texture_matrix;
	};
	constexpr RvSurfaceHandle source_handle = 40;
	constexpr RvSurfaceHandle committed_handle = 41;
	const std::vector<uint8_t> source{
		255, 0, 0, 255, 0, 255, 0, 255,
		0, 0, 255, 255, 255, 255, 255, 255};
	if(!upload_rgba_surface(store, source_handle, 2, 2, source, 40, RV_ALPHA_OPAQUE)) {
		store.destroy_surface(source_handle);
		return false;
	}
	Scale2xRenderer scaler(store, operations, functions);
	FramePresenter frames(store, scaler);
	ScaleReport scale_report{};
	const SurfaceRect content{0, 0, 2, 2};
	const CallerState before_commit = capture_state();
	bool passed = frames.commit(source_handle, content, 1, RV_SCALE_2X,
		committed_handle, &scale_report) == RV_RESULT_OK;
	if(!passed) std::cerr << "Frame snapshot commit failed.\n";
	if(!same_state(before_commit, capture_state())) {
		std::cerr << "Frame snapshot commit changed caller GL state.\n";
		passed = false;
	}
	std::vector<uint8_t> expected;
	passed = passed && scale2x_reference_rgba(source.data(), source.size(), 2, 2, &expected) == RV_RESULT_OK;
	CommittedFrameSnapshot frame{};
	passed = passed && frames.current(&frame) == RV_RESULT_OK;
	RvViewport viewport{};
	viewport.struct_bytes = RV_VIEWPORT_BYTES;
	viewport.abi_version = RV_ABI_VERSION;
	viewport.framebuffer_width = 8;
	viewport.framebuffer_height = 8;
	viewport.aspect_num = 1;
	viewport.aspect_den = 1;
	viewport.filter = RV_FILTER_NEAREST;
	viewport.fit = RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT;
	OpenGlFrameOutputRenderer output(functions);
	GLint previous_framebuffer = 0;
	::glGetIntegerv(0x8CA6, &previous_framebuffer);
	const std::array<RvSurfaceHandle, 2> no_overlays{{0, 0}};
	const CallerState before_render = capture_state();
	const RvResult output_result = output.render(frame, no_overlays, store, viewport);
	if(output_result != RV_RESULT_OK) std::cerr << "Frame output returned " << output_result << ".\n";
	const CallerState after_render = capture_state();
	if(!same_state(before_render, after_render)) {
		std::cerr << "Frame output renderer changed caller GL state.\n";
		std::cerr << "  framebuffer " << before_render.framebuffer << " -> " << after_render.framebuffer
			<< ", matrix mode " << before_render.matrix_mode << " -> " << after_render.matrix_mode
			<< ", active texture " << before_render.active_texture << " -> " << after_render.active_texture
			<< ", texture binding " << before_render.texture_binding << " -> " << after_render.texture_binding
			<< ", program " << before_render.program << " -> " << after_render.program
			<< ", texturing " << static_cast<int>(before_render.texture_enabled) << " -> "
			<< static_cast<int>(after_render.texture_enabled) << ".\n";
		for(size_t index = 0; index < before_render.projection.size(); ++index) {
			if(before_render.projection[index] != after_render.projection[index] ||
				before_render.modelview[index] != after_render.modelview[index] ||
				before_render.texture_matrix[index] != after_render.texture_matrix[index]) {
				std::cerr << "  matrix values changed at index " << index << ": projection "
					<< before_render.projection[index] << " -> " << after_render.projection[index]
					<< ", modelview " << before_render.modelview[index] << " -> "
					<< after_render.modelview[index] << ", texture "
					<< before_render.texture_matrix[index] << " -> "
					<< after_render.texture_matrix[index] << ".\n";
				break;
			}
		}
		passed = false;
	}
	const GLenum render_error = ::glGetError();
	if(render_error != GL_NO_ERROR) std::cerr << "Frame output raised GL error " << render_error << ".\n";
	passed = passed && output_result == RV_RESULT_OK;
	GLint restored_framebuffer = 0;
	::glGetIntegerv(0x8CA6, &restored_framebuffer);
	if(restored_framebuffer != previous_framebuffer) std::cerr << "Frame output did not restore the caller framebuffer.\n";
	passed = passed && restored_framebuffer == previous_framebuffer;
	auto bind_framebuffer = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
		functions.get(GlProcedure::BindFramebuffer));
	bind_framebuffer(0x8D40, 0);
	std::vector<uint8_t> bottom_up(8 * 8 * 4);
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, bottom_up.data());
	const GLenum read_error = ::glGetError();
	if(read_error != GL_NO_ERROR) std::cerr << "Frame output readback raised GL error " << read_error << ".\n";
	passed = passed && read_error == GL_NO_ERROR;
	for(uint32_t y = 0; passed && y < 8; ++y) {
		for(uint32_t x = 0; passed && x < 8; ++x) {
			const size_t actual_offset = (static_cast<size_t>(7 - y) * 8 + x) * 4;
			const size_t expected_offset = (static_cast<size_t>(y / 2) * 4 + x / 2) * 4;
			const bool pixel_matches = std::equal(expected.begin() + expected_offset,
				expected.begin() + expected_offset + 4,
				bottom_up.begin() + actual_offset);
			if(!pixel_matches) {
				std::cerr << "Frame output pixel at " << x << "," << y << " expected "
					<< static_cast<uint32_t>(expected[expected_offset]) << ","
					<< static_cast<uint32_t>(expected[expected_offset + 1]) << ","
					<< static_cast<uint32_t>(expected[expected_offset + 2]) << ","
					<< static_cast<uint32_t>(expected[expected_offset + 3]) << " got "
					<< static_cast<uint32_t>(bottom_up[actual_offset]) << ","
					<< static_cast<uint32_t>(bottom_up[actual_offset + 1]) << ","
					<< static_cast<uint32_t>(bottom_up[actual_offset + 2]) << ","
					<< static_cast<uint32_t>(bottom_up[actual_offset + 3]) << ".\n";
			}
			passed = passed && pixel_matches;
		}
	}
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	RvReadbackRect readback_rect{};
	readback_rect.struct_bytes = RV_READBACK_RECT_BYTES;
	readback_rect.abi_version = RV_ABI_VERSION;
	readback_rect.width = 2;
	readback_rect.height = 2;
	readback_rect.output_format = RV_PIXEL_FORMAT_BGRA8;
	readback_rect.output_alpha_mode = RV_ALPHA_STRAIGHT;
	std::array<uint8_t, 24> diagnostic_pixels{};
	diagnostic_pixels.fill(0xCD);
	GraphicsDiagnostics diagnostics(functions);
	passed = passed && diagnostics.readback(store, source_handle, readback_rect,
		RV_READBACK_RECT_BYTES, diagnostic_pixels.data(),
		static_cast<uint32_t>(diagnostic_pixels.size()), 12) == RV_RESULT_OK;
	const std::array<uint8_t, 24> expected_bgra{{
		0, 0, 255, 255, 0, 255, 0, 255, 0xCD, 0xCD, 0xCD, 0xCD,
		255, 0, 0, 255, 255, 255, 255, 255, 0xCD, 0xCD, 0xCD, 0xCD
	}};
	passed = passed && diagnostic_pixels == expected_bgra;
	if(diagnostic_pixels != expected_bgra) {
		std::cerr << "Diagnostic readback did not preserve top-left order, BGRA, padding or alpha.\n";
	}
	std::array<uint8_t, 24> unchanged_diagnostic{};
	unchanged_diagnostic.fill(0xA5);
	RvReadbackRect invalid_readback = readback_rect;
	invalid_readback.x = 1;
	const RvResult invalid_readback_result = diagnostics.readback(store, source_handle,
		invalid_readback, RV_READBACK_RECT_BYTES, unchanged_diagnostic.data(),
		static_cast<uint32_t>(unchanged_diagnostic.size()), 12);
	passed = passed && invalid_readback_result == RV_RESULT_INVALID_ARGUMENT &&
		std::all_of(unchanged_diagnostic.begin(), unchanged_diagnostic.end(),
			[](uint8_t value) { return value == 0xA5; });
	invalid_readback = readback_rect;
	passed = passed && diagnostics.readback(store, source_handle, invalid_readback,
		RV_READBACK_RECT_BYTES, unchanged_diagnostic.data(),
		static_cast<uint32_t>(unchanged_diagnostic.size()), 7) == RV_RESULT_INVALID_ARGUMENT;
	const std::array<uint8_t, 16> overlay_pixels{{
		0, 255, 0, 128, 0, 0, 0, 0,
		0, 0, 0, 0, 0, 0, 0, 0
	}};
	const RvSurfaceHandle overlay_handle = 42;
	passed = passed && upload_rgba_surface(store, overlay_handle, 2, 2,
		std::vector<uint8_t>(overlay_pixels.begin(), overlay_pixels.end()),
		overlay_handle, RV_ALPHA_STRAIGHT);
	const std::array<RvSurfaceHandle, 2> overlays{{overlay_handle, 0}};
	passed = passed && output.render(frame, overlays, store, viewport) == RV_RESULT_OK;
	const GLenum overlay_error = ::glGetError();
	passed = passed && overlay_error == GL_NO_ERROR;
	bind_framebuffer(0x8D40, 0);
	std::array<uint8_t, 8 * 8 * 4> overlay_readback{};
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, overlay_readback.data());
	const size_t top_left_offset = static_cast<size_t>(7) * 8 * 4;
	const std::array<uint8_t, 4> expected_overlay_pixel{{127, 128, 0, 255}};
	passed = passed && std::equal(expected_overlay_pixel.begin(), expected_overlay_pixel.end(),
		overlay_readback.begin() + top_left_offset);
	if(!std::equal(expected_overlay_pixel.begin(), expected_overlay_pixel.end(),
		overlay_readback.begin() + top_left_offset)) {
		std::cerr << "Premultiplied HUD overlay did not composite over the committed frame.\n";
	}
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	store.destroy_surface(overlay_handle);
	frames.destroy();
	scaler.destroy();
	store.destroy_surface(source_handle);
	return passed;
}

bool test_completed_frame_context_pipeline(const GlFunctionTable& functions)
{
	using namespace revival::graphics;
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = 7001;
	config.texture_budget_bytes = 1024 * 1024;
	config.max_queued_bytes = 4096;
	config.max_queued_ops = 16;
	config.max_pending_frames = 2;
	config.detail_scale = RV_SCALE_2X;
	config.input_mode = RV_INPUT_MODE_COMPLETED_FRAMES;
	const HGLRC native_context = ::wglGetCurrentContext();
	ContextSession context(7001, config, std::this_thread::get_id(),
		reinterpret_cast<void*>(native_context), functions);
	RvFrameDesc descriptor = make_desc(2, 2, 1);
	const std::array<uint8_t, 16> pixels{{
		255, 0, 0, 255, 0, 255, 0, 255,
		0, 0, 255, 255, 255, 255, 255, 255
	}};
	bool passed = context.submit_completed_frame(&descriptor, RV_FRAME_DESC_BYTES,
		pixels.data(), static_cast<uint32_t>(pixels.size())) == RV_RESULT_OK;
	RvPumpStats pump{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	passed = passed && context.pump(0, &pump) == RV_RESULT_OK &&
		pump.frames_completed == 0 && pump.queued_bytes == pixels.size() &&
		context.present(RvViewport{RV_VIEWPORT_BYTES, RV_ABI_VERSION, 8, 8, 1, 1,
			RV_FILTER_NEAREST, RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT}) == RV_RESULT_OK;
	if(!passed) std::cerr << "Zero-budget frame pump changed or failed to retain the current image.\n";
	const RvViewport viewport{RV_VIEWPORT_BYTES, RV_ABI_VERSION, 8, 8, 1, 1,
		RV_FILTER_NEAREST, RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT};
	pump = RvPumpStats{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	const RvResult pump_result = context.pump(UINT64_C(1000000000), &pump);
	passed = passed && pump_result == RV_RESULT_OK && pump.frames_completed == 1 &&
		pump.operations_processed == 1 && pump.uploaded_bytes == pixels.size() &&
		pump.queued_bytes == 0 && context.present(viewport) == RV_RESULT_OK;
	std::array<char, 256> vendor{};
	std::array<char, 256> renderer{};
	std::array<char, 256> version{};
	passed = passed && context.device_strings(vendor.data(), static_cast<uint32_t>(vendor.size()),
		renderer.data(), static_cast<uint32_t>(renderer.size()),
		version.data(), static_cast<uint32_t>(version.size())) == RV_RESULT_OK &&
		vendor[0] != '\0' && renderer[0] != '\0' && version[0] != '\0';
	GLint previous_framebuffer = 0;
	::glGetIntegerv(0x8CA6, &previous_framebuffer);
	auto bind_framebuffer = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
		functions.get(GlProcedure::BindFramebuffer));
	bind_framebuffer(0x8D40, 0);
	std::array<uint8_t, 8 * 8 * 4> readback{};
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, readback.data());
	const std::array<uint8_t, 4> expected_red{{255, 0, 0, 255}};
	const size_t top_left_offset = static_cast<size_t>(7) * 8 * 4;
	passed = passed && std::equal(expected_red.begin(), expected_red.end(), readback.begin() + top_left_offset);
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	RvFrameDesc hud_desc = make_desc(4, 2, 1);
	hud_desc.alpha_mode = RV_ALPHA_PREMULTIPLIED;
	std::array<uint8_t, 32> hud_pixels{};
	hud_pixels[1] = 128;
	hud_pixels[3] = 128;
	const RvResult hud_submit = context.submit_overlay(0, &hud_desc, RV_FRAME_DESC_BYTES,
		hud_pixels.data(), static_cast<uint32_t>(hud_pixels.size()));
	if(hud_submit != RV_RESULT_OK) std::cerr << "Mismatched HUD submit returned " << hud_submit << ".\n";
	passed = passed && hud_submit == RV_RESULT_OK;
	pump = RvPumpStats{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	const RvResult hud_pump = context.pump(UINT64_C(1000000000), &pump);
	if(hud_pump != RV_RESULT_OK || pump.operations_processed != 1) {
		std::cerr << "Mismatched HUD pump returned " << hud_pump << " after "
			<< pump.operations_processed << " operations.\n";
	}
	passed = passed && hud_pump == RV_RESULT_OK &&
		pump.operations_processed == 1 && pump.uploaded_bytes == hud_pixels.size() &&
		context.present(viewport) == RV_RESULT_OK;
	bind_framebuffer(0x8D40, 0);
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, readback.data());
	const size_t fitted_hud_offset = static_cast<size_t>(8 - 1 - 2) * 8 * 4;
	const std::array<uint8_t, 4> expected_hud_pixel{{127, 128, 0, 255}};
	passed = passed && std::equal(expected_hud_pixel.begin(), expected_hud_pixel.end(),
		readback.begin() + fitted_hud_offset) &&
		std::equal(expected_red.begin(), expected_red.end(), readback.begin() + top_left_offset);
	if(!std::equal(expected_hud_pixel.begin(), expected_hud_pixel.end(),
		readback.begin() + fitted_hud_offset)) {
		std::cerr << "Mismatched HUD pixel was "
			<< static_cast<uint32_t>(readback[fitted_hud_offset]) << ","
			<< static_cast<uint32_t>(readback[fitted_hud_offset + 1]) << ","
			<< static_cast<uint32_t>(readback[fitted_hud_offset + 2]) << ","
			<< static_cast<uint32_t>(readback[fitted_hud_offset + 3]) << ".\n";
	}
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	RvFrameDesc clear_hud_desc = hud_desc;
	clear_hud_desc.physical_width = 0;
	clear_hud_desc.physical_height = 0;
	clear_hud_desc.physical_stride_bytes = 0;
	clear_hud_desc.logical_width = 0;
	clear_hud_desc.logical_height = 0;
	passed = passed && context.submit_overlay(0, &clear_hud_desc, RV_FRAME_DESC_BYTES,
		nullptr, 0) == RV_RESULT_OK;
	pump = RvPumpStats{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	passed = passed && context.pump(UINT64_C(1000000000), &pump) == RV_RESULT_OK &&
		pump.operations_processed == 1;
	passed = passed && context.clear_frame() == RV_RESULT_OK &&
		context.present(viewport) == RV_RESULT_OK;
	bind_framebuffer(0x8D40, 0);
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, readback.data());
	bool cleared_black = true;
	for(size_t pixel = 0; pixel < readback.size(); pixel += 4) {
		cleared_black = cleared_black && readback[pixel] == 0 && readback[pixel + 1] == 0 &&
			readback[pixel + 2] == 0 && readback[pixel + 3] == 255;
	}
	passed = passed && cleared_black;
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	context.begin_close();
	context.clear_graphics_resources();
	if(!passed) std::cerr << "Completed-frame context pump, device info, present or clear check failed.\n";
	return passed;
}

bool test_command_context_pipeline(const GlFunctionTable& functions)
{
	using namespace revival::graphics;
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = 7002;
	config.texture_budget_bytes = 1024 * 1024;
	config.max_queued_bytes = 4096;
	config.max_queued_ops = 32;
	config.max_pending_frames = 2;
	config.detail_scale = RV_SCALE_2X;
	config.input_mode = RV_INPUT_MODE_COMMANDS;
	const HGLRC native_context = ::wglGetCurrentContext();
	ContextSession context(7002, config, std::this_thread::get_id(),
		reinterpret_cast<void*>(native_context), functions);

	const std::vector<uint8_t> one_row(16, 255);
	const auto invalid = make_packet({
		{static_cast<uint16_t>(RV_OP_CREATE_SURFACE), create_payload(2, 2, 2, RV_SCALE_2X)},
		{static_cast<uint16_t>(RV_OP_UPLOAD_ROWS), upload_rows_payload(2, 0, one_row)}
	}, config.session_epoch, 1);
	bool passed = context.queue().submit_packet(invalid.data(),
		static_cast<uint32_t>(invalid.size())) == RV_RESULT_OK;
	RvPumpStats pump{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	passed = passed && context.pump(UINT64_C(1000000000), &pump) == RV_RESULT_INVALID_ARGUMENT;
	TextureSurfaceSnapshot missing{};
	passed = passed && context.texture_store().inspect(2, &missing) == RV_RESULT_STALE_HANDLE;

	RvFrameDesc descriptor = make_desc(4, 4, 0);
	descriptor.logical_width = 2;
	descriptor.logical_height = 2;
	descriptor.source_scale = RV_SCALE_2X;
	descriptor.desired_scale = RV_SCALE_2X;
	descriptor.physical_stride_bytes = 16;
	const std::vector<uint8_t> pixels(64, 255);
	std::vector<std::pair<uint16_t, std::vector<uint8_t>>> commands{
		{static_cast<uint16_t>(RV_OP_CREATE_SURFACE), create_payload(1, 2, 2, RV_SCALE_2X)},
		{static_cast<uint16_t>(RV_OP_BEGIN_UPLOAD), begin_upload_payload(1, descriptor, 64)}
	};
	for(uint32_t row = 0; row < 4; ++row) {
		commands.push_back({static_cast<uint16_t>(RV_OP_UPLOAD_ROWS), upload_rows_payload(1,
			row, std::vector<uint8_t>(pixels.begin() + row * 16, pixels.begin() + (row + 1) * 16))});
	}
	std::vector<uint8_t> end_payload;
	append_u64(end_payload, 1);
	commands.push_back({static_cast<uint16_t>(RV_OP_END_UPLOAD), end_payload});
	commands.push_back({static_cast<uint16_t>(RV_OP_FILL), fill_packet_payload(1)});
	commands.push_back({static_cast<uint16_t>(RV_OP_CREATE_SURFACE), create_payload(4, 2, 2, RV_SCALE_4X)});
	std::vector<uint8_t> scale_payload;
	append_u64(scale_payload, 1);
	append_u64(scale_payload, 4);
	append_u32(scale_payload, RV_SCALE_4X);
	append_u32(scale_payload, 0);
	commands.push_back({static_cast<uint16_t>(RV_OP_SCALE_SURFACE), scale_payload});
	commands.push_back({static_cast<uint16_t>(RV_OP_COMMIT_FRAME), commit_payload(1, 1)});
	const auto valid = make_packet(commands, config.session_epoch, 2);
	passed = passed && context.queue().submit_packet(valid.data(),
		static_cast<uint32_t>(valid.size())) == RV_RESULT_OK;
	pump = RvPumpStats{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	passed = passed && context.pump(0, &pump) == RV_RESULT_OK &&
		pump.operations_processed == 0 && pump.queued_bytes == valid.size();
	pump = RvPumpStats{};
	pump.struct_bytes = RV_PUMP_STATS_BYTES;
	pump.abi_version = RV_ABI_VERSION;
	passed = passed && context.pump(UINT64_C(1000000000), &pump) == RV_RESULT_OK &&
		pump.operations_processed == commands.size() && pump.frames_completed == 1 &&
		pump.uploaded_bytes == 64 && pump.queued_bytes == 0;
	TextureSurfaceSnapshot scaled_surface{};
	passed = passed && context.texture_store().inspect(4, &scaled_surface) == RV_RESULT_OK &&
		scaled_surface.initialized && scaled_surface.storage_scale == RV_SCALE_4X;
	const RvViewport viewport{RV_VIEWPORT_BYTES, RV_ABI_VERSION, 8, 8, 1, 1,
		RV_FILTER_NEAREST, RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT};
	passed = passed && context.present(viewport) == RV_RESULT_OK;
	GLint previous_framebuffer = 0;
	::glGetIntegerv(0x8CA6, &previous_framebuffer);
	auto bind_framebuffer = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
		functions.get(GlProcedure::BindFramebuffer));
	bind_framebuffer(0x8D40, 0);
	std::array<uint8_t, 8 * 8 * 4> readback{};
	::glReadPixels(0, 0, 8, 8, GL_RGBA, GL_UNSIGNED_BYTE, readback.data());
	const size_t top_left = static_cast<size_t>(7) * 8 * 4;
	const std::array<uint8_t, 4> red{{255, 0, 0, 255}};
	const std::array<uint8_t, 4> white{{255, 255, 255, 255}};
	passed = passed && ::glGetError() == GL_NO_ERROR &&
		std::equal(red.begin(), red.end(), readback.begin() + top_left) &&
		std::equal(white.begin(), white.end(), readback.end() - 4);
	bind_framebuffer(0x8D40, static_cast<GLuint>(previous_framebuffer));
	context.begin_close();
	context.clear_graphics_resources();
	if(!passed) std::cerr << "Ordered command packet upload, preflight, commit or output check failed.\n";
	return passed;
}

} // namespace

int main()
{
	const HINSTANCE instance = ::GetModuleHandleW(nullptr);
	WNDCLASSW window_class{};
	window_class.style = CS_OWNDC;
	window_class.lpfnWndProc = &hidden_window_proc;
	window_class.hInstance = instance;
	window_class.lpszClassName = kWindowClass;
	const ATOM registered = ::RegisterClassW(&window_class);
	if(registered == 0 && ::GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
		return skip("could not register a hidden WGL test window");
	}

	HWND window = ::CreateWindowExW(0, kWindowClass, L"", WS_POPUP,
		0, 0, 8, 8, nullptr, nullptr, instance, nullptr);
	if(window == nullptr) {
		if(registered != 0) ::UnregisterClassW(kWindowClass, instance);
		return skip("could not create a hidden WGL test window");
	}
	HDC dc = ::GetDC(window);
	PIXELFORMATDESCRIPTOR format{};
	format.nSize = sizeof(format);
	format.nVersion = 1;
	format.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL;
	format.iPixelType = PFD_TYPE_RGBA;
	format.cColorBits = 32;
	format.cAlphaBits = 8;
	format.iLayerType = PFD_MAIN_PLANE;
	const int pixel_format = ::ChoosePixelFormat(dc, &format);
	if(dc == nullptr || pixel_format == 0 || !::SetPixelFormat(dc, pixel_format, &format)) {
		if(dc != nullptr) ::ReleaseDC(window, dc);
		::DestroyWindow(window);
		if(registered != 0) ::UnregisterClassW(kWindowClass, instance);
		return skip("could not choose a WGL pixel format");
	}
	HGLRC context = ::wglCreateContext(dc);
	if(context == nullptr || !::wglMakeCurrent(dc, context)) {
		if(context != nullptr) ::wglDeleteContext(context);
		::ReleaseDC(window, dc);
		::DestroyWindow(window);
		if(registered != 0) ::UnregisterClassW(kWindowClass, instance);
		return skip("the current machine could not create a WGL context");
	}

	int result = 0;
	const auto resolver = revival::graphics::current_windows_gl_resolver();
	revival::graphics::GlFunctionTable functions{};
	const char* missing = nullptr;
	if(!revival::graphics::load_required_gl_functions(resolver, &functions, &missing)) {
		std::cout << "SKIP: OpenGL context lacks required procedure "
			<< (missing == nullptr ? "(resolver unavailable)" : missing) << '\n';
		result = 77;
	} else {
		revival::graphics::OpenGlTextureDevice device(functions);
		revival::graphics::TextureStore store(device, 1024 * 1024);
		auto bind_texture = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
			functions.get(revival::graphics::GlProcedure::BindTexture));
		auto bind_framebuffer = reinterpret_cast<void (APIENTRY*)(GLenum, GLuint)>(
			functions.get(revival::graphics::GlProcedure::BindFramebuffer));
		GLuint sentinel_texture = 0;
		GLuint sentinel_framebuffer = 0;
		::glGenTextures(1, &sentinel_texture);
		auto gen_framebuffers = reinterpret_cast<void (APIENTRY*)(GLsizei, GLuint*)>(
			functions.get(revival::graphics::GlProcedure::GenFramebuffers));
		gen_framebuffers(1, &sentinel_framebuffer);
		bind_texture(GL_TEXTURE_2D, sentinel_texture);
		bind_framebuffer(0x8D40, sentinel_framebuffer);
		auto pixel_store = reinterpret_cast<void (APIENTRY*)(GLenum, GLint)>(
			functions.get(revival::graphics::GlProcedure::PixelStorei));
		pixel_store(0x0CF5, 8);
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
		const std::array<uint8_t, 16> source{{
			200, 0, 0, 128, 0, 200, 0, 255,
			0, 0, 200, 128, 255, 255, 255, 0
		}};
		RvResult status = store.create_surface(1, 2, 2, RV_SCALE_ORIGINAL);
		GLint binding = 0;
		::glGetIntegerv(0x8069, &binding);
		if(binding != static_cast<GLint>(sentinel_texture)) status = RV_RESULT_INTERNAL_ERROR;
		::glGetIntegerv(0x8CA6, &binding);
		if(binding != static_cast<GLint>(sentinel_framebuffer)) status = RV_RESULT_INTERNAL_ERROR;
		::glGetIntegerv(0x0CF5, &binding);
		if(binding != 8) status = RV_RESULT_INTERNAL_ERROR;
		if(status == RV_RESULT_OK) status = store.begin_upload(1, desc, 16);
		if(status == RV_RESULT_OK) {
			status = store.upload_rows(1, 0, 1, 8, source.data(), 8);
		}
		if(status == RV_RESULT_OK) {
			status = store.upload_rows(1, 1, 1, 8, source.data() + 8, 8);
		}
		if(status == RV_RESULT_OK) status = store.end_upload(1);
		::glGetIntegerv(0x8069, &binding);
		if(binding != static_cast<GLint>(sentinel_texture)) status = RV_RESULT_INTERNAL_ERROR;
		::glGetIntegerv(0x8CA6, &binding);
		if(binding != static_cast<GLint>(sentinel_framebuffer)) status = RV_RESULT_INTERNAL_ERROR;
		::glGetIntegerv(0x0CF5, &binding);
		if(binding != 8) status = RV_RESULT_INTERNAL_ERROR;

		revival::graphics::TextureSurfaceSnapshot surface{};
		if(status == RV_RESULT_OK) status = store.inspect(1, &surface);
		if(status == RV_RESULT_OK) {
			bind_framebuffer(0x8D40, surface.active.framebuffer);
			std::array<uint8_t, 16> readback{};
			::glReadPixels(0, 0, 2, 2, GL_RGBA, GL_UNSIGNED_BYTE, readback.data());
			const GLenum error = ::glGetError();
			const std::array<uint8_t, 16> expected_bottom_up{{
				0, 0, 100, 128, 0, 0, 0, 0,
				100, 0, 0, 128, 0, 200, 0, 255
			}};
			if(error != GL_NO_ERROR || readback != expected_bottom_up) {
				std::cerr << "GL readback differed from the independent RGBA/alpha/orientation fixture.\n";
				result = 1;
			}
		}
		if(status == RV_RESULT_OK && result == 0) {
			OpenGlSurfaceDevice surface_renderer(device, functions);
			SurfaceOperations surface_operations(store, surface_renderer);
			GLint original_matrix_mode = 0;
			GLint original_framebuffer = 0;
			GLint original_texture = 0;
			::glGetIntegerv(0x0BA0, &original_matrix_mode);
			::glGetIntegerv(0x8CA6, &original_framebuffer);
			::glGetIntegerv(0x8069, &original_texture);
			const bool original_texturing = ::glIsEnabled(GL_TEXTURE_2D) == GL_TRUE;
			std::array<GLfloat, 16> original_projection{};
			std::array<GLfloat, 16> original_modelview{};
			std::array<GLfloat, 16> original_texture_matrix{};
			::glGetFloatv(0x0BA7, original_projection.data());
			::glGetFloatv(0x0BA6, original_modelview.data());
			::glGetFloatv(0x0BA8, original_texture_matrix.data());
			// Non-identity caller matrices make the fixture catch leaked host transforms.
			::glMatrixMode(0x1701);
			::glLoadIdentity();
			::glTranslatef(0.25f, -0.125f, 0.f);
			::glMatrixMode(0x1700);
			::glLoadIdentity();
			::glRotatef(13.f, 0.f, 0.f, 1.f);
			::glMatrixMode(0x1702);
			::glLoadIdentity();
			::glScalef(0.5f, 0.75f, 1.f);
			std::array<GLfloat, 16> expected_projection{};
			std::array<GLfloat, 16> expected_modelview{};
			std::array<GLfloat, 16> expected_texture_matrix{};
			::glGetFloatv(0x0BA7, expected_projection.data());
			::glGetFloatv(0x0BA6, expected_modelview.data());
			::glGetFloatv(0x0BA8, expected_texture_matrix.data());
			bool surface_checks_passed = test_gpu_surface_transforms(
				store, surface_operations, functions);
			if(!surface_checks_passed) {
				std::cerr << "GPU blit readback differed from the independent eight-transform fixture.\n";
			}
			if(surface_checks_passed && !test_gpu_clip_blend_and_self_copy(
				store, surface_operations, functions)) {
				std::cerr << "GPU clip, premultiplied source-over, or self-copy differed from expected pixels.\n";
				surface_checks_passed = false;
			}
			if(surface_checks_passed && !test_gpu_scale2x(store, surface_operations, functions)) {
				std::cerr << "GPU Scale2x readback differed from the CPU reference or fallback contract.\n";
				surface_checks_passed = false;
			}
			if(surface_checks_passed && !test_gpu_frame_presentation(
				store, surface_operations, functions)) {
				std::cerr << "GPU committed-frame presentation differed from the expected aspect-fit pixels or state.\n";
				surface_checks_passed = false;
			}
			if(surface_checks_passed && !test_completed_frame_context_pipeline(functions)) {
				surface_checks_passed = false;
			}
			if(surface_checks_passed && !test_command_context_pipeline(functions)) {
				surface_checks_passed = false;
			}
			GLint final_matrix_mode = 0;
			GLint final_framebuffer = 0;
			GLint final_texture = 0;
			::glGetIntegerv(0x0BA0, &final_matrix_mode);
			::glGetIntegerv(0x8CA6, &final_framebuffer);
			::glGetIntegerv(0x8069, &final_texture);
			std::array<GLfloat, 16> final_projection{};
			std::array<GLfloat, 16> final_modelview{};
			std::array<GLfloat, 16> final_texture_matrix{};
			::glGetFloatv(0x0BA7, final_projection.data());
			::glGetFloatv(0x0BA6, final_modelview.data());
			::glGetFloatv(0x0BA8, final_texture_matrix.data());
			const bool caller_state_preserved = final_matrix_mode == 0x1702 &&
				final_framebuffer == original_framebuffer && final_texture == original_texture &&
				(::glIsEnabled(GL_TEXTURE_2D) == GL_TRUE) == original_texturing &&
				final_projection == expected_projection && final_modelview == expected_modelview &&
				final_texture_matrix == expected_texture_matrix;
			::glMatrixMode(0x1701);
			::glLoadMatrixf(original_projection.data());
			::glMatrixMode(0x1700);
			::glLoadMatrixf(original_modelview.data());
			::glMatrixMode(0x1702);
			::glLoadMatrixf(original_texture_matrix.data());
			::glMatrixMode(static_cast<GLenum>(original_matrix_mode));
			if(!surface_checks_passed || !caller_state_preserved) {
				if(!caller_state_preserved) {
					std::cerr << "Caller state mismatch: matrix mode " << final_matrix_mode << "/0x1702, framebuffer "
						<< final_framebuffer << "/" << original_framebuffer << ", texture " << final_texture << "/"
						<< original_texture << ", texturing " << (::glIsEnabled(GL_TEXTURE_2D) == GL_TRUE)
						<< "/" << original_texturing << ", matrices "
						<< (final_projection == expected_projection) << "/"
						<< (final_modelview == expected_modelview) << "/"
						<< (final_texture_matrix == expected_texture_matrix) << ".\n";
				}
				result = 1;
			}
		}
		if(status != RV_RESULT_OK) {
			std::cerr << "Texture upload returned native result " << status << ".\n";
			result = 1;
		}
		store.clear();
		::glGetIntegerv(0x8CA6, &binding);
		const bool deleted_framebuffer_unbound = binding == 0;
		::glGetIntegerv(0x8069, &binding);
		const bool sentinel_texture_preserved = binding == static_cast<GLint>(sentinel_texture);
		bind_framebuffer(0x8D40, 0);
		bind_texture(GL_TEXTURE_2D, 0);
		pixel_store(0x0CF5, 4);
		::glDeleteTextures(1, &sentinel_texture);
		auto delete_framebuffers = reinterpret_cast<void (APIENTRY*)(GLsizei, const GLuint*)>(
			functions.get(revival::graphics::GlProcedure::DeleteFramebuffers));
		delete_framebuffers(1, &sentinel_framebuffer);
		if(!deleted_framebuffer_unbound || !sentinel_texture_preserved) {
			std::cerr << "Retiring a bound surface changed unrelated GL state or left a dead binding.\n";
			result = 1;
		}
		if(result == 0) {
			const auto renderer = reinterpret_cast<const char*>(::glGetString(GL_RENDERER));
			const auto version = reinterpret_cast<const char*>(::glGetString(GL_VERSION));
			std::cout << "Hidden WGL upload, ordered command execution, transforms, clip, blend, self-copy, Scale2x and state checks passed on "
				<< (renderer == nullptr ? "unknown renderer" : renderer) << " ("
				<< (version == nullptr ? "unknown version" : version) << ").\n";
		}
	}

	::wglMakeCurrent(nullptr, nullptr);
	::wglDeleteContext(context);
	::ReleaseDC(window, dc);
	::DestroyWindow(window);
	if(registered != 0) ::UnregisterClassW(kWindowClass, instance);
	return result;
}
