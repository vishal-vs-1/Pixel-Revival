cmake_minimum_required(VERSION 3.27)
project(revival_nes_bridge LANGUAGES CXX)

if(NOT WIN32)
	message(FATAL_ERROR "The Mesen bridge is currently Windows-only.")
endif()

set(REVIVAL_GRAPHICS_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/../native-graphics/include" CACHE PATH "Common graphics ABI include directory")
set(MESEN_SOURCE_ROOT "" CACHE PATH "Pinned MesenCE source checkout for the optional bridge target")

add_library(revival_nes_bridge_contract STATIC
	core/RevivalFrameContract.cpp
	core/NesTileObserver.cpp
	core/NesCaptureWriter.cpp
	core/NesInputMapper.cpp)
target_include_directories(revival_nes_bridge_contract PUBLIC
	"${CMAKE_CURRENT_SOURCE_DIR}/core" "${REVIVAL_GRAPHICS_INCLUDE}")
target_compile_features(revival_nes_bridge_contract PRIVATE cxx_std_17)

if(MESEN_SOURCE_ROOT)
	add_library(revival_nes_bridge STATIC
		core/RevivalRenderingDevice.cpp
		core/RevivalFrameContract.cpp
		core/NesTileObserver.cpp
		core/NesTileObserverMesen.cpp
		windows/RevivalWglHost.cpp)
	target_include_directories(revival_nes_bridge PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/core"
		"${CMAKE_CURRENT_SOURCE_DIR}/windows"
		"${REVIVAL_GRAPHICS_INCLUDE}"
		"${MESEN_SOURCE_ROOT}/Core"
		"${MESEN_SOURCE_ROOT}")
	target_compile_definitions(revival_nes_bridge PRIVATE REVIVAL_MESEN_BRIDGE)
	target_compile_features(revival_nes_bridge PRIVATE cxx_std_17)
	target_link_libraries(revival_nes_bridge PRIVATE opengl32 user32 gdi32)
endif()

include(CTest)
if(BUILD_TESTING)
	add_executable(revival_nes_bridge_contract_checks tests/frame_contract_checks.cpp)
	target_include_directories(revival_nes_bridge_contract_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/core" "${REVIVAL_GRAPHICS_INCLUDE}")
	target_compile_features(revival_nes_bridge_contract_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_nes_bridge_contract_checks PRIVATE revival_nes_bridge_contract)
	add_test(NAME revival_nes_bridge_contract_checks COMMAND revival_nes_bridge_contract_checks)
	add_executable(revival_nes_tile_observer_checks tests/nes_tile_observer_checks.cpp)
	target_include_directories(revival_nes_tile_observer_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/core")
	target_compile_features(revival_nes_tile_observer_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_nes_tile_observer_checks PRIVATE revival_nes_bridge_contract)
	add_test(NAME revival_nes_tile_observer_checks COMMAND revival_nes_tile_observer_checks)
	add_executable(revival_nes_capture_writer_checks tests/nes_capture_writer_checks.cpp)
	target_include_directories(revival_nes_capture_writer_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/core")
	target_compile_features(revival_nes_capture_writer_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_nes_capture_writer_checks PRIVATE revival_nes_bridge_contract)
	add_test(NAME revival_nes_capture_writer_checks COMMAND revival_nes_capture_writer_checks)
	add_executable(revival_nes_input_mapper_checks tests/nes_input_mapper_checks.cpp)
	target_include_directories(revival_nes_input_mapper_checks PRIVATE
		"${CMAKE_CURRENT_SOURCE_DIR}/core")
	target_compile_features(revival_nes_input_mapper_checks PRIVATE cxx_std_17)
	target_link_libraries(revival_nes_input_mapper_checks PRIVATE revival_nes_bridge_contract)
	add_test(NAME revival_nes_input_mapper_checks COMMAND revival_nes_input_mapper_checks)
endif()
