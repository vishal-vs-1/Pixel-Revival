cmake_minimum_required(VERSION 3.27)

project(revival_graphics VERSION 1.0 LANGUAGES C CXX)

if(NOT WIN32)
	message(FATAL_ERROR "The native graphics ABI and WGL context layer are currently defined for Windows only.")
endif()
if(NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
	message(FATAL_ERROR "The native graphics ABI requires a 64-bit toolchain.")
endif()

set(JAVA_HOME "" CACHE PATH "JDK root used for JNI headers and the probe")
if(NOT JAVA_HOME AND DEFINED ENV{JAVA_HOME})
	set(JAVA_HOME "$ENV{JAVA_HOME}")
endif()
if(NOT EXISTS "${JAVA_HOME}/include/jni.h")
	message(FATAL_ERROR "Set JAVA_HOME to a JDK containing include/jni.h.")
endif()
if(NOT EXISTS "${JAVA_HOME}/include/win32/jni_md.h")
	message(FATAL_ERROR "The selected JDK does not provide Windows JNI headers.")
endif()

set(RV_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/include/revival_graphics.h")
set(RV_PROTOCOL "${CMAKE_CURRENT_SOURCE_DIR}/protocol/graphics-v1.md")
set(RV_ABI_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/abi_skeleton.cpp")
set(RV_LAYOUT_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/abi_layout_checks.cpp")
set(RV_PACKET_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/packet.cpp")
set(RV_QUEUE_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/command_queue.cpp")
set(RV_GL_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/gl_functions.cpp")
set(RV_CONTEXT_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/context.cpp")
set(RV_CONTEXT_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/context.hpp")
set(RV_QUEUE_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/command_queue.hpp")
set(RV_PACKET_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/packet.hpp")
set(RV_COMMAND_PROTOCOL_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/command_protocol.cpp")
set(RV_COMMAND_PROTOCOL_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/command_protocol.hpp")
set(RV_COMMAND_CONSUMER_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/command_consumer.cpp")
set(RV_COMMAND_CONSUMER_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/command_consumer.hpp")
set(RV_GL_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/gl_functions.hpp")
set(RV_FRAME_DESC_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/frame_desc.cpp")
set(RV_FRAME_DESC_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/frame_desc.hpp")
set(RV_PIXEL_TRANSFER_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/pixel_transfer.cpp")
set(RV_PIXEL_TRANSFER_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/pixel_transfer.hpp")
set(RV_TEXTURE_DEVICE_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/texture_device.cpp")
set(RV_TEXTURE_DEVICE_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/texture_device.hpp")
set(RV_TEXTURE_STORE_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/texture_store.cpp")
set(RV_TEXTURE_STORE_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/texture_store.hpp")
set(RV_SURFACE_PROTOCOL_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_protocol.cpp")
set(RV_SURFACE_PROTOCOL_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_protocol.hpp")
set(RV_SURFACE_OPS_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_ops.cpp")
set(RV_SURFACE_OPS_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_ops.hpp")
set(RV_SURFACE_DEVICE_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_device.cpp")
set(RV_SURFACE_DEVICE_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/surface_device.hpp")
set(RV_SCALE2X_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/scale2x.cpp")
set(RV_SCALE2X_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/scale2x.hpp")
set(RV_SCALE2X_SHADER "${CMAKE_CURRENT_SOURCE_DIR}/shaders/scale2x_source.hpp")
set(RV_COMPLETED_FRAME_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/completed_frame.cpp")
set(RV_COMPLETED_FRAME_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/completed_frame.hpp")
set(RV_FRAME_PRESENTER_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/frame_presenter.cpp")
set(RV_FRAME_PRESENTER_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/frame_presenter.hpp")
set(RV_FRAME_OUTPUT_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/frame_output_renderer.cpp")
set(RV_DIAGNOSTICS_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/diagnostics.cpp")
set(RV_DIAGNOSTICS_HEADER "${CMAKE_CURRENT_SOURCE_DIR}/src/diagnostics.hpp")
set(RV_JNI_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/jni/revival_graphics_jni.cpp")
set(RV_DEPS_FILE "${CMAKE_CURRENT_SOURCE_DIR}/dependencies.json")
set(RV_CMAKE_FILE "${CMAKE_CURRENT_SOURCE_DIR}/CMakeLists.txt")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
	"${RV_HEADER}" "${RV_PROTOCOL}" "${RV_ABI_SOURCE}" "${RV_LAYOUT_SOURCE}"
	"${RV_PACKET_SOURCE}" "${RV_QUEUE_SOURCE}" "${RV_GL_SOURCE}" "${RV_CONTEXT_SOURCE}"
	"${RV_CONTEXT_HEADER}" "${RV_QUEUE_HEADER}" "${RV_PACKET_HEADER}"
	"${RV_COMMAND_PROTOCOL_SOURCE}" "${RV_COMMAND_PROTOCOL_HEADER}"
	"${RV_COMMAND_CONSUMER_SOURCE}" "${RV_COMMAND_CONSUMER_HEADER}" "${RV_GL_HEADER}"
	"${RV_FRAME_DESC_SOURCE}" "${RV_FRAME_DESC_HEADER}"
	"${RV_PIXEL_TRANSFER_SOURCE}" "${RV_PIXEL_TRANSFER_HEADER}"
	"${RV_TEXTURE_DEVICE_SOURCE}" "${RV_TEXTURE_DEVICE_HEADER}"
	"${RV_TEXTURE_STORE_SOURCE}" "${RV_TEXTURE_STORE_HEADER}"
	"${RV_SURFACE_PROTOCOL_SOURCE}" "${RV_SURFACE_PROTOCOL_HEADER}"
	"${RV_SURFACE_OPS_SOURCE}" "${RV_SURFACE_OPS_HEADER}"
	"${RV_SURFACE_DEVICE_SOURCE}" "${RV_SURFACE_DEVICE_HEADER}"
	"${RV_SCALE2X_SOURCE}" "${RV_SCALE2X_HEADER}" "${RV_SCALE2X_SHADER}"
	"${RV_COMPLETED_FRAME_SOURCE}" "${RV_COMPLETED_FRAME_HEADER}"
	"${RV_FRAME_PRESENTER_SOURCE}" "${RV_FRAME_PRESENTER_HEADER}" "${RV_FRAME_OUTPUT_SOURCE}"
	"${RV_DIAGNOSTICS_SOURCE}" "${RV_DIAGNOSTICS_HEADER}" "${RV_JNI_SOURCE}"
	"${RV_DEPS_FILE}" "${RV_CMAKE_FILE}")
file(SHA256 "${RV_HEADER}" RV_HEADER_SHA256)
file(SHA256 "${RV_PROTOCOL}" RV_PROTOCOL_SHA256)
file(SHA256 "${RV_ABI_SOURCE}" RV_ABI_SOURCE_SHA256)
file(SHA256 "${RV_LAYOUT_SOURCE}" RV_LAYOUT_SOURCE_SHA256)
file(SHA256 "${RV_PACKET_SOURCE}" RV_PACKET_SOURCE_SHA256)
file(SHA256 "${RV_QUEUE_SOURCE}" RV_QUEUE_SOURCE_SHA256)
file(SHA256 "${RV_GL_SOURCE}" RV_GL_SOURCE_SHA256)
file(SHA256 "${RV_CONTEXT_SOURCE}" RV_CONTEXT_SOURCE_SHA256)
file(SHA256 "${RV_CONTEXT_HEADER}" RV_CONTEXT_HEADER_SHA256)
file(SHA256 "${RV_QUEUE_HEADER}" RV_QUEUE_HEADER_SHA256)
file(SHA256 "${RV_PACKET_HEADER}" RV_PACKET_HEADER_SHA256)
file(SHA256 "${RV_COMMAND_PROTOCOL_SOURCE}" RV_COMMAND_PROTOCOL_SOURCE_SHA256)
file(SHA256 "${RV_COMMAND_PROTOCOL_HEADER}" RV_COMMAND_PROTOCOL_HEADER_SHA256)
file(SHA256 "${RV_COMMAND_CONSUMER_SOURCE}" RV_COMMAND_CONSUMER_SOURCE_SHA256)
file(SHA256 "${RV_COMMAND_CONSUMER_HEADER}" RV_COMMAND_CONSUMER_HEADER_SHA256)
file(SHA256 "${RV_GL_HEADER}" RV_GL_HEADER_SHA256)
file(SHA256 "${RV_FRAME_DESC_SOURCE}" RV_FRAME_DESC_SOURCE_SHA256)
file(SHA256 "${RV_FRAME_DESC_HEADER}" RV_FRAME_DESC_HEADER_SHA256)
file(SHA256 "${RV_PIXEL_TRANSFER_SOURCE}" RV_PIXEL_TRANSFER_SOURCE_SHA256)
file(SHA256 "${RV_PIXEL_TRANSFER_HEADER}" RV_PIXEL_TRANSFER_HEADER_SHA256)
file(SHA256 "${RV_TEXTURE_DEVICE_SOURCE}" RV_TEXTURE_DEVICE_SOURCE_SHA256)
file(SHA256 "${RV_TEXTURE_DEVICE_HEADER}" RV_TEXTURE_DEVICE_HEADER_SHA256)
file(SHA256 "${RV_TEXTURE_STORE_SOURCE}" RV_TEXTURE_STORE_SOURCE_SHA256)
file(SHA256 "${RV_TEXTURE_STORE_HEADER}" RV_TEXTURE_STORE_HEADER_SHA256)
file(SHA256 "${RV_SURFACE_PROTOCOL_SOURCE}" RV_SURFACE_PROTOCOL_SOURCE_SHA256)
file(SHA256 "${RV_SURFACE_PROTOCOL_HEADER}" RV_SURFACE_PROTOCOL_HEADER_SHA256)
file(SHA256 "${RV_SURFACE_OPS_SOURCE}" RV_SURFACE_OPS_SOURCE_SHA256)
file(SHA256 "${RV_SURFACE_OPS_HEADER}" RV_SURFACE_OPS_HEADER_SHA256)
file(SHA256 "${RV_SURFACE_DEVICE_SOURCE}" RV_SURFACE_DEVICE_SOURCE_SHA256)
file(SHA256 "${RV_SURFACE_DEVICE_HEADER}" RV_SURFACE_DEVICE_HEADER_SHA256)
file(SHA256 "${RV_SCALE2X_SOURCE}" RV_SCALE2X_SOURCE_SHA256)
file(SHA256 "${RV_SCALE2X_HEADER}" RV_SCALE2X_HEADER_SHA256)
file(SHA256 "${RV_SCALE2X_SHADER}" RV_SCALE2X_SHADER_SHA256)
file(SHA256 "${RV_COMPLETED_FRAME_SOURCE}" RV_COMPLETED_FRAME_SOURCE_SHA256)
file(SHA256 "${RV_COMPLETED_FRAME_HEADER}" RV_COMPLETED_FRAME_HEADER_SHA256)
file(SHA256 "${RV_FRAME_PRESENTER_SOURCE}" RV_FRAME_PRESENTER_SOURCE_SHA256)
file(SHA256 "${RV_FRAME_PRESENTER_HEADER}" RV_FRAME_PRESENTER_HEADER_SHA256)
file(SHA256 "${RV_FRAME_OUTPUT_SOURCE}" RV_FRAME_OUTPUT_SOURCE_SHA256)
file(SHA256 "${RV_DIAGNOSTICS_SOURCE}" RV_DIAGNOSTICS_SOURCE_SHA256)
file(SHA256 "${RV_DIAGNOSTICS_HEADER}" RV_DIAGNOSTICS_HEADER_SHA256)
file(SHA256 "${RV_JNI_SOURCE}" RV_JNI_SOURCE_SHA256)
file(SHA256 "${RV_DEPS_FILE}" RV_DEPS_SHA256)
file(SHA256 "${RV_CMAKE_FILE}" RV_CMAKE_SHA256)
string(SHA256 RV_BUILD_ID_HEX
	"${RV_HEADER_SHA256}|${RV_PROTOCOL_SHA256}|${RV_ABI_SOURCE_SHA256}|${RV_LAYOUT_SOURCE_SHA256}|${RV_PACKET_SOURCE_SHA256}|${RV_QUEUE_SOURCE_SHA256}|${RV_GL_SOURCE_SHA256}|${RV_CONTEXT_SOURCE_SHA256}|${RV_CONTEXT_HEADER_SHA256}|${RV_QUEUE_HEADER_SHA256}|${RV_PACKET_HEADER_SHA256}|${RV_COMMAND_PROTOCOL_SOURCE_SHA256}|${RV_COMMAND_PROTOCOL_HEADER_SHA256}|${RV_COMMAND_CONSUMER_SOURCE_SHA256}|${RV_COMMAND_CONSUMER_HEADER_SHA256}|${RV_GL_HEADER_SHA256}|${RV_FRAME_DESC_SOURCE_SHA256}|${RV_FRAME_DESC_HEADER_SHA256}|${RV_PIXEL_TRANSFER_SOURCE_SHA256}|${RV_PIXEL_TRANSFER_HEADER_SHA256}|${RV_TEXTURE_DEVICE_SOURCE_SHA256}|${RV_TEXTURE_DEVICE_HEADER_SHA256}|${RV_TEXTURE_STORE_SOURCE_SHA256}|${RV_TEXTURE_STORE_HEADER_SHA256}|${RV_SURFACE_PROTOCOL_SOURCE_SHA256}|${RV_SURFACE_PROTOCOL_HEADER_SHA256}|${RV_SURFACE_OPS_SOURCE_SHA256}|${RV_SURFACE_OPS_HEADER_SHA256}|${RV_SURFACE_DEVICE_SOURCE_SHA256}|${RV_SURFACE_DEVICE_HEADER_SHA256}|${RV_SCALE2X_SOURCE_SHA256}|${RV_SCALE2X_HEADER_SHA256}|${RV_SCALE2X_SHADER_SHA256}|${RV_COMPLETED_FRAME_SOURCE_SHA256}|${RV_COMPLETED_FRAME_HEADER_SHA256}|${RV_FRAME_PRESENTER_SOURCE_SHA256}|${RV_FRAME_PRESENTER_HEADER_SHA256}|${RV_FRAME_OUTPUT_SOURCE_SHA256}|${RV_DIAGNOSTICS_SOURCE_SHA256}|${RV_DIAGNOSTICS_HEADER_SHA256}|${RV_JNI_SOURCE_SHA256}|${RV_DEPS_SHA256}|${RV_CMAKE_SHA256}|${CMAKE_CXX_COMPILER_ID}|${CMAKE_CXX_COMPILER_VERSION}|${CMAKE_SIZEOF_VOID_P}")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/abi-build-id.txt" "${RV_BUILD_ID_HEX}\n")

add_library(revival_graphics SHARED
	src/abi_header_c_check.c
	src/abi_skeleton.cpp
	src/abi_layout_checks.cpp
	src/packet.cpp
	src/command_protocol.cpp
	src/command_consumer.cpp
	src/command_queue.cpp
	src/gl_functions.cpp
	src/context.cpp
	src/frame_desc.cpp
	src/pixel_transfer.cpp
	src/texture_device.cpp
	src/texture_store.cpp
	src/surface_protocol.cpp
	src/surface_ops.cpp
	src/surface_device.cpp
	src/scale2x.cpp
	src/completed_frame.cpp
	src/frame_presenter.cpp
	src/frame_output_renderer.cpp
	src/diagnostics.cpp
)
target_include_directories(revival_graphics PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_compile_features(revival_graphics PRIVATE c_std_11 cxx_std_17)
target_link_libraries(revival_graphics PRIVATE opengl32)
target_compile_definitions(revival_graphics PRIVATE
	REVIVAL_GRAPHICS_BUILD
	RV_BUILD_ID_HEX="${RV_BUILD_ID_HEX}"
)
set_target_properties(revival_graphics PROPERTIES
	OUTPUT_NAME "revival_graphics"
	RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/bin"
)

add_library(revival_graphics_jni SHARED jni/revival_graphics_jni.cpp)
target_include_directories(revival_graphics_jni PRIVATE
	"${CMAKE_CURRENT_SOURCE_DIR}/include"
	"${JAVA_HOME}/include"
	"${JAVA_HOME}/include/win32"
)
target_compile_features(revival_graphics_jni PRIVATE cxx_std_17)
target_compile_definitions(revival_graphics_jni PRIVATE
	RV_EXPECTED_BUILD_ID_HEX="${RV_BUILD_ID_HEX}"
)
target_link_libraries(revival_graphics_jni PRIVATE revival_graphics)
set_target_properties(revival_graphics_jni PROPERTIES
	OUTPUT_NAME "revival_graphics_jni"
	RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/bin"
)

include(CTest)
if(BUILD_TESTING)
	add_executable(revival_graphics_contract_checks
		tests/contract_checks.cpp
		src/packet.cpp
		src/command_protocol.cpp
		src/command_consumer.cpp
		src/command_queue.cpp
		src/gl_functions.cpp
		src/context.cpp
		src/frame_desc.cpp
		src/pixel_transfer.cpp
		src/texture_device.cpp
		src/texture_store.cpp
		src/surface_protocol.cpp
		src/surface_ops.cpp
		src/surface_device.cpp
		src/scale2x.cpp
		src/completed_frame.cpp
		src/frame_presenter.cpp
		src/frame_output_renderer.cpp
		src/diagnostics.cpp
	)
	target_include_directories(revival_graphics_contract_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/include"
		"${CMAKE_CURRENT_SOURCE_DIR}/src")
	target_compile_features(revival_graphics_contract_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_graphics_contract_checks PRIVATE opengl32)
	add_test(NAME revival_graphics_contract_checks COMMAND revival_graphics_contract_checks)
	add_executable(revival_graphics_abi_checks tests/abi_checks.cpp)
	target_include_directories(revival_graphics_abi_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/include")
	target_compile_features(revival_graphics_abi_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_graphics_abi_checks PRIVATE revival_graphics)
	set_target_properties(revival_graphics_abi_checks PROPERTIES
		RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/bin")
	add_test(NAME revival_graphics_abi_checks COMMAND revival_graphics_abi_checks)
endif()

option(RV_BUILD_GL_HARNESS "Build the opt-in hidden-window OpenGL upload check" OFF)
if(BUILD_TESTING AND RV_BUILD_GL_HARNESS)
	add_executable(revival_graphics_gl_harness
		tests/gl_harness.cpp
		src/gl_functions.cpp
		src/packet.cpp
		src/command_protocol.cpp
		src/command_consumer.cpp
		src/command_queue.cpp
		src/context.cpp
		src/frame_desc.cpp
		src/pixel_transfer.cpp
		src/texture_device.cpp
		src/texture_store.cpp
		src/surface_protocol.cpp
		src/surface_ops.cpp
		src/surface_device.cpp
		src/scale2x.cpp
		src/completed_frame.cpp
		src/frame_presenter.cpp
		src/frame_output_renderer.cpp
		src/diagnostics.cpp
	)
	target_include_directories(revival_graphics_gl_harness PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/include"
		"${CMAKE_CURRENT_SOURCE_DIR}/src")
	target_compile_features(revival_graphics_gl_harness PRIVATE cxx_std_17)
	target_link_libraries(revival_graphics_gl_harness PRIVATE opengl32 user32 gdi32)
	set_target_properties(revival_graphics_gl_harness PROPERTIES
		RUNTIME_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/bin")
	add_test(NAME revival_graphics_gl_harness COMMAND revival_graphics_gl_harness)
	set_tests_properties(revival_graphics_gl_harness PROPERTIES SKIP_RETURN_CODE 77)
endif()

if(MSVC)
	foreach(target revival_graphics revival_graphics_jni revival_graphics_contract_checks revival_graphics_abi_checks revival_graphics_gl_harness)
		if(TARGET ${target})
		target_compile_options(${target} PRIVATE /W4 /permissive- /Zc:__cplusplus)
		endif()
	endforeach()
endif()
