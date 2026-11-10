#include <jni.h>

#include <cstdint>

#include "revival_graphics.h"

#ifndef RV_EXPECTED_BUILD_ID_HEX
#error CMake must provide the expected common graphics build ID.
#endif

static_assert(sizeof(RV_EXPECTED_BUILD_ID_HEX) == 65, "expected build ID must be 64 hexadecimal bytes");

namespace {

constexpr uint32_t kKnownFeatures =
	RV_FEATURE_SURFACE_COMMANDS |
	RV_FEATURE_COMPLETED_FRAMES |
	RV_FEATURE_SCALE2X |
	RV_FEATURE_ACTUAL_DEVICE |
	RV_FEATURE_SURFACE_SCALE;

void throw_java(JNIEnv* env, const char* class_name, const char* message)
{
	jclass type = env->FindClass(class_name);
	if(type != nullptr) {
		env->ThrowNew(type, message);
		env->DeleteLocalRef(type);
	}
}

bool read_build_info(JNIEnv* env, RvBuildInfo* out)
{
	out->struct_bytes = RV_BUILD_INFO_BYTES;
	out->abi_version = RV_ABI_VERSION;
	const RvResult result = rv_get_build_info(out, RV_BUILD_INFO_BYTES);
	if(result != RV_RESULT_OK) {
		throw_java(env, "java/lang/IllegalStateException", "Common graphics ABI probe failed.");
		return false;
	}
	for(uint32_t index = 0; index < sizeof(out->build_id); ++index) {
		if(out->build_id[index] != RV_EXPECTED_BUILD_ID_HEX[index]) {
			throw_java(env, "java/lang/LinkageError", "JNI shim and common graphics DLL build IDs differ.");
			return false;
		}
	}
	return true;
}

void write_u32_le(jbyte* target, uint32_t value)
{
	for(uint32_t index = 0; index < 4; ++index) {
		target[index] = static_cast<jbyte>((value >> (index * 8)) & 0xffu);
	}
}

} // namespace

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphicsProbe_nativeAbiVersion(JNIEnv*, jclass)
{
	return static_cast<jint>(rv_get_abi_version());
}

extern "C" JNIEXPORT jlong JNICALL
Java_org_revival_graphics_NativeGraphicsProbe_nativeFeatureBits(JNIEnv* env, jclass)
{
	RvBuildInfo info{};
	if(!read_build_info(env, &info)) {
		return 0;
	}
	return static_cast<jlong>(info.feature_bits);
}

extern "C" JNIEXPORT void JNICALL
Java_org_revival_graphics_NativeGraphicsProbe_nativeRequireFeature(JNIEnv* env, jclass, jint feature)
{
	if(feature <= 0 || (static_cast<uint32_t>(feature) & ~kKnownFeatures) != 0) {
		throw_java(env, "java/lang/IllegalArgumentException", "Requested graphics feature code is invalid.");
		return;
	}

	RvBuildInfo info{};
	if(!read_build_info(env, &info)) {
		return;
	}
	const uint32_t requested = static_cast<uint32_t>(feature);
	if((info.feature_bits & requested) != requested) {
		throw_java(env, "java/lang/UnsupportedOperationException",
			"This ABI skeleton does not implement the requested graphics operation.");
	}
}

extern "C" JNIEXPORT jbyteArray JNICALL
Java_org_revival_graphics_NativeGraphics_nativeBuildInfo(JNIEnv* env, jclass)
{
	RvBuildInfo info{};
	if(!read_build_info(env, &info)) {
		return nullptr;
	}
	jbyte bytes[RV_BUILD_INFO_BYTES]{};
	write_u32_le(bytes, info.struct_bytes);
	write_u32_le(bytes + 4, info.abi_version);
	for(uint32_t index = 0; index < sizeof(info.build_id); ++index) {
		bytes[8 + index] = static_cast<jbyte>(info.build_id[index]);
	}
	write_u32_le(bytes + 72, info.feature_bits);
	jbyteArray result = env->NewByteArray(static_cast<jsize>(sizeof(bytes)));
	if(result == nullptr) return nullptr;
	env->SetByteArrayRegion(result, 0, static_cast<jsize>(sizeof(bytes)), bytes);
	return result;
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativeSubmitPacket(
	JNIEnv* env, jclass, jlong context, jobject packet, jint offset, jint length)
{
	if(context == 0 || packet == nullptr || offset < 0 || length < 32) {
		return static_cast<jint>(RV_RESULT_INVALID_ARGUMENT);
	}
	const jlong capacity = env->GetDirectBufferCapacity(packet);
	void* address = env->GetDirectBufferAddress(packet);
	if(capacity < 0 || address == nullptr || static_cast<jlong>(offset) > capacity ||
		static_cast<jlong>(length) > capacity - static_cast<jlong>(offset)) {
		return static_cast<jint>(RV_RESULT_INVALID_ARGUMENT);
	}
	const auto* bytes = static_cast<const uint8_t*>(address) + offset;
	return static_cast<jint>(rv_submit(static_cast<RvContextHandle>(context), bytes,
		static_cast<uint32_t>(length)));
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_org_revival_graphics_NativeGraphics_nativeAttach(
	JNIEnv* env, jclass, jlong epoch, jlong texture_budget, jint queued_bytes,
	jint queued_ops, jint pending_frames, jint detail_scale, jint input_mode)
{
	RvAttachConfig config{};
	config.struct_bytes = RV_ATTACH_CONFIG_BYTES;
	config.abi_version = RV_ABI_VERSION;
	config.session_epoch = static_cast<uint64_t>(epoch);
	config.texture_budget_bytes = static_cast<uint64_t>(texture_budget);
	config.max_queued_bytes = static_cast<uint32_t>(queued_bytes);
	config.max_queued_ops = static_cast<uint32_t>(queued_ops);
	config.max_pending_frames = static_cast<uint32_t>(pending_frames);
	config.detail_scale = static_cast<uint32_t>(detail_scale);
	config.input_mode = static_cast<uint32_t>(input_mode);
	RvContextHandle context = 0;
	const RvResult result = rv_attach_current_context(&config, RV_ATTACH_CONFIG_BYTES, &context);
	const jlong values[2] = {static_cast<jlong>(result), static_cast<jlong>(context)};
	jlongArray output = env->NewLongArray(2);
	if(output != nullptr) env->SetLongArrayRegion(output, 0, 2, values);
	return output;
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_org_revival_graphics_NativeGraphics_nativePump(JNIEnv* env, jclass,
	jlong context, jlong budget_nanos)
{
	RvPumpStats stats{};
	stats.struct_bytes = RV_PUMP_STATS_BYTES;
	stats.abi_version = RV_ABI_VERSION;
	const RvResult result = rv_pump(static_cast<RvContextHandle>(context),
		static_cast<uint64_t>(budget_nanos), &stats);
	const jlong values[7] = {static_cast<jlong>(result),
		static_cast<jlong>(stats.operations_processed), static_cast<jlong>(stats.frames_completed),
		static_cast<jlong>(stats.queued_bytes), static_cast<jlong>(stats.queued_ops),
		static_cast<jlong>(stats.uploaded_bytes), static_cast<jlong>(stats.elapsed_ns)};
	jlongArray output = env->NewLongArray(7);
	if(output != nullptr) env->SetLongArrayRegion(output, 0, 7, values);
	return output;
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativePresent(JNIEnv*, jclass, jlong context,
	jint width, jint height, jint aspect_width, jint aspect_height, jint filter)
{
	RvViewport viewport{};
	viewport.struct_bytes = RV_VIEWPORT_BYTES;
	viewport.abi_version = RV_ABI_VERSION;
	viewport.framebuffer_width = static_cast<uint32_t>(width);
	viewport.framebuffer_height = static_cast<uint32_t>(height);
	viewport.aspect_num = static_cast<uint32_t>(aspect_width);
	viewport.aspect_den = static_cast<uint32_t>(aspect_height);
	viewport.filter = static_cast<uint32_t>(filter);
	viewport.fit = RV_FIT_DESKTOP_HEIGHT_PRESERVE_ASPECT;
	return static_cast<jint>(rv_present(static_cast<RvContextHandle>(context),
		&viewport, RV_VIEWPORT_BYTES));
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativeClearFrame(JNIEnv*, jclass, jlong context)
{
	return static_cast<jint>(rv_clear_frame(static_cast<RvContextHandle>(context)));
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativeReadback(JNIEnv* env, jclass, jlong context,
	jlong surface, jint x, jint y, jint width, jint height, jint output_format,
	jint output_alpha_mode, jobject destination)
{
	if(context == 0 || surface == 0 || x < 0 || y < 0 || width <= 0 || height <= 0 || destination == nullptr) {
		return static_cast<jint>(RV_RESULT_INVALID_ARGUMENT);
	}
	const uint64_t bytes = static_cast<uint64_t>(width) * static_cast<uint64_t>(height) * 4u;
	const jlong capacity = env->GetDirectBufferCapacity(destination);
	void* address = env->GetDirectBufferAddress(destination);
	if(bytes > UINT32_MAX || capacity < static_cast<jlong>(bytes) || address == nullptr) {
		return static_cast<jint>(RV_RESULT_INVALID_ARGUMENT);
	}
	RvReadbackRect rect{};
	rect.struct_bytes = RV_READBACK_RECT_BYTES;
	rect.abi_version = RV_ABI_VERSION;
	rect.x = static_cast<uint32_t>(x);
	rect.y = static_cast<uint32_t>(y);
	rect.width = static_cast<uint32_t>(width);
	rect.height = static_cast<uint32_t>(height);
	rect.output_format = static_cast<uint32_t>(output_format);
	rect.output_alpha_mode = static_cast<uint32_t>(output_alpha_mode);
	return static_cast<jint>(rv_readback_for_check(static_cast<RvContextHandle>(context),
		static_cast<RvSurfaceHandle>(surface), &rect, RV_READBACK_RECT_BYTES,
		static_cast<uint8_t*>(address), static_cast<uint32_t>(bytes),
		static_cast<uint32_t>(width) * 4u));
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativeBeginClose(JNIEnv*, jclass, jlong context)
{
	return static_cast<jint>(rv_begin_close(static_cast<RvContextHandle>(context)));
}

extern "C" JNIEXPORT jint JNICALL
Java_org_revival_graphics_NativeGraphics_nativeDestroyOnOwner(JNIEnv*, jclass, jlong context)
{
	return static_cast<jint>(rv_destroy_on_owner(static_cast<RvContextHandle>(context)));
}
