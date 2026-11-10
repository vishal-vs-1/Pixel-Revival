"""Generate an opt-in MesenCE bridge checkout from the pinned source.

The script is intentionally source-anchored and writes to a separate output
tree.  The stock checkout is never edited, and a missing/changed anchor stops
the build instead of producing a partially patched emulator.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / "nes-bridge" / "mesen" / "upstream.json"


def _require(source: str, anchor: str) -> None:
    if anchor not in source:
        raise ValueError(f"Mesen bridge anchor missing: {anchor}")


def patch_factory(source: str) -> str:
    _require(source, '#include "InteropNotificationListeners.h"')
    _require(source, "IRenderingDevice* InitRenderer()")
    _require(source, "bool _softwareRenderer = false;")
    _require(source, 'DllExport void __stdcall InitializeEmu(')
    if "RevivalRenderingDevice.hpp" not in source:
        source = source.replace(
            '#include "InteropNotificationListeners.h"',
            '#include "InteropNotificationListeners.h"\n#ifdef REVIVAL_MESEN_BRIDGE\n#include "RevivalRenderingDevice.hpp"\n#endif',
            1,
        )
    if '#include "Core/NES/NesConsole.h"' not in source:
        source = source.replace(
            '#include "Core/Shared/Emulator.h"',
            '#include "Core/Shared/Emulator.h"\n#ifdef REVIVAL_MESEN_BRIDGE\n#include "Core/NES/NesConsole.h"\n#endif',
            1,
        )
    if "#include <cstring>" not in source:
        source = source.replace(
            '#include "InteropNotificationListeners.h"',
            '#include <cstring>\n#include "InteropNotificationListeners.h"',
            1,
        )
    if "static bool _revivalRenderer = false;" not in source:
        source = source.replace(
            "bool _softwareRenderer = false;",
            "bool _softwareRenderer = false;\n#ifdef REVIVAL_MESEN_BRIDGE\nstatic bool _revivalRenderer = false;\n#endif",
            1,
        )
    old_factory = "\t\treturn new revival::nes::RevivalRenderingDevice((HWND)_viewerHandle);"
    earlier_registration = (
        "new revival::nes::RevivalRenderingDevice(_emu->GetVideoRenderer(), (HWND)_viewerHandle);"
    )
    new_factory = (
        "\t\tauto* renderer = new revival::nes::RevivalRenderingDevice("
        "_emu.get(), (HWND)_viewerHandle);\n"
        "\t\t_emu->GetVideoRenderer()->RegisterRenderingDevice(renderer);\n"
        "\t\treturn renderer;"
    )
    if "if(_revivalRenderer)" not in source:
        source = source.replace(
            "{\n\tif(_softwareRenderer) {",
            "{\n#ifdef REVIVAL_MESEN_BRIDGE\n\tif(_revivalRenderer) {\n"
            + new_factory
            + "\n\t}\n#endif\n\tif(_softwareRenderer) {",
            1,
        )
    elif "RegisterRenderingDevice(renderer)" not in source:
        _require(source, old_factory)
        source = source.replace(old_factory, new_factory, 1)
    elif earlier_registration in source:
        source = source.replace(earlier_registration,
                                "new revival::nes::RevivalRenderingDevice(_emu.get(), (HWND)_viewerHandle);", 1)
    marker = '\tDllExport void __stdcall InitializeEmu('
    setter = '\tDllExport void __stdcall SetRevivalRenderer(bool enabled)\n\t{\n#ifdef REVIVAL_MESEN_BRIDGE\n\t\t_revivalRenderer = enabled;\n#else\n\t\t(void)enabled;\n#endif\n\t}\n\n'
    if "DllExport void __stdcall SetRevivalRenderer(bool enabled)" not in source:
        source = source.replace(marker, setter + marker, 1)
    status_anchor = '\tDllExport uint32_t __stdcall GetMesenVersion()'
    _require(source, status_anchor)
    if "GetRevivalNesHdPlaybackStatus" not in source:
        status_export = """#ifdef REVIVAL_MESEN_BRIDGE
	struct RevivalNesHdPlaybackStatus
	{
		uint32_t StructBytes;
		uint32_t AbiVersion;
		uint32_t PackLoaded;
		uint32_t PackScale;
		uint32_t CommonRendererActive;
		uint32_t FrameWidth;
		uint32_t FrameHeight;
		uint32_t Reserved;
		uint32_t IdentityAvailable;
		uint32_t GraphicsAbiVersion;
		uint64_t SubmittedFrames;
		uint64_t PresentedFrames;
		char GraphicsBuildId[65];
		char AdapterVendor[129];
		char AdapterRenderer[257];
		char AdapterVersion[129];
	};
	static_assert(sizeof(RevivalNesHdPlaybackStatus) == 640, "managed HD status ABI layout changed");

	DllExport void __stdcall GetRevivalNesHdPlaybackStatus(RevivalNesHdPlaybackStatus* status)
	{
		if(status == nullptr) return;
		*status = {};
		status->StructBytes = sizeof(RevivalNesHdPlaybackStatus);
		status->AbiVersion = 2;
		auto console = _emu->GetConsole();
		auto nes = std::dynamic_pointer_cast<NesConsole>(console);
		if(nes) {
			status->PackLoaded = nes->HasManagedHdPack() ? 1u : 0u;
			status->PackScale = nes->GetManagedHdPackScale();
		}
		auto renderer = std::dynamic_pointer_cast<revival::nes::RevivalRenderingDevice>(_emu->GetRenderer());
		if(renderer) {
			status->CommonRendererActive = renderer->active() ? 1u : 0u;
			renderer->GetFrameStatus(status->SubmittedFrames, status->PresentedFrames,
				status->FrameWidth, status->FrameHeight);
			revival::nes::RevivalRenderingDevice::GraphicsIdentity identity{};
			if(renderer->GetGraphicsIdentity(identity)) {
				status->IdentityAvailable = 1;
				status->GraphicsAbiVersion = identity.AbiVersion;
				std::memcpy(status->GraphicsBuildId, identity.BuildId, sizeof(status->GraphicsBuildId));
				std::memcpy(status->AdapterVendor, identity.Vendor, sizeof(status->AdapterVendor));
				std::memcpy(status->AdapterRenderer, identity.Renderer, sizeof(status->AdapterRenderer));
				std::memcpy(status->AdapterVersion, identity.Version, sizeof(status->AdapterVersion));
			}
		}
	}
#endif

"""
        source = source.replace(status_anchor, status_export + status_anchor, 1)
    if "StartRevivalNesCapture(" not in source:
        include = '#include "RevivalRenderingDevice.hpp"'
        source = source.replace(include, include + '\n#include "NesCaptureWriter.hpp"'
            '\n#include "Core/NES/BaseNesPpu.h"\n#include "Core/NES/NesDefaultVideoFilter.h"', 1)
        flag = "static bool _revivalRenderer = false;"
        source = source.replace(flag, flag + "\nstatic bool _revivalCaptureStarted = false;", 1)
        capture_export = """
#ifdef REVIVAL_MESEN_BRIDGE
	static revival::nes::NesCaptureSession& RevivalNesCapture()
	{
		static revival::nes::NesCaptureSession session(revival::nes::global_nes_tile_observer());
		return session;
	}

	DllExport bool __stdcall StartRevivalNesCapture(const wchar_t* directory,
		const char* gameId, const char* sessionId)
	{
		try {
			if(_revivalCaptureStarted || !_emu || !directory || !gameId || !sessionId) return false;
			auto nes = std::dynamic_pointer_cast<NesConsole>(_emu->GetConsole());
			if(!nes || !nes->HasManagedHdPack() || !nes->GetPpu()) return false;
			uint32_t colors[512]{};
			NesDefaultVideoFilter::GetFullPalette(colors, _emu->GetSettings()->GetNesConfig(),
				nes->GetPpu()->GetPpuModel());
			std::array<uint8_t, revival::nes::RV_NES_CAPTURE_PALETTE_BYTES> palette{};
			for(size_t index = 0; index < 64; ++index) {
				palette[index * 3] = static_cast<uint8_t>(colors[index] >> 16);
				palette[index * 3 + 1] = static_cast<uint8_t>(colors[index] >> 8);
				palette[index * 3 + 2] = static_cast<uint8_t>(colors[index]);
			}
			_revivalCaptureStarted = RevivalNesCapture().start(directory, gameId, sessionId, palette);
			return _revivalCaptureStarted;
		} catch(...) {
			return false;
		}
	}
#endif

"""
        release = '\tDllExport void __stdcall Release()'
        _require(source, release)
        source = source.replace(release, capture_export + release, 1)
        release_core = '\t\t\t_emu->Stop(true);\n\t\t\t_emu->Release();'
        _require(source, release_core)
        source = source.replace(release_core,
            '\t\t\t_emu->Stop(true);\n#ifdef REVIVAL_MESEN_BRIDGE\n'
            '\t\t\tif(_revivalCaptureStarted) {\n'
            '\t\t\t\tRevivalNesCapture().stop();\n'
            '\t\t\t\t_revivalCaptureStarted = false;\n\t\t\t}\n#endif\n'
            '\t\t\t_emu->Release();', 1)
    return source


def patch_nes_console_header(source: str) -> str:
    anchor = "\tNesConfig& GetNesConfig();"
    _require(source, anchor)
    declaration = anchor + "\n\tbool HasManagedHdPack();\n\tuint32_t GetManagedHdPackScale();"
    if "HasManagedHdPack()" not in source:
        source = source.replace(anchor, declaration, 1)
    return source


def patch_nes_console_cpp(source: str) -> str:
    anchor = "BaseVideoFilter* NesConsole::GetVideoFilter(bool getDefaultFilter)"
    _require(source, anchor)
    if "bool NesConsole::HasManagedHdPack()" not in source:
        methods = """bool NesConsole::HasManagedHdPack()
{
	return _hdData && _hdData->Scale > 0;
}

uint32_t NesConsole::GetManagedHdPackScale()
{
	return _hdData ? _hdData->Scale : 0;
}

"""
        source = source.replace(anchor, methods + anchor, 1)
    return source


def patch_project(project: str) -> str:
    anchors = [
        '<ClCompile Include="EmuApiWrapper.cpp" />',
        '<ClInclude Include="Common.h" />',
        '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>',
        '<PreprocessorDefinitions>WIN32;',
        '<AdditionalDependencies>dinput8.lib;',
    ]
    for anchor in anchors:
        if anchor == '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>':
            if anchor not in project and "nes-bridge\\core;" not in project:
                _require(project, anchor)
        elif anchor == '<PreprocessorDefinitions>WIN32;':
            if anchor not in project and "REVIVAL_MESEN_BRIDGE;WIN32;" not in project:
                _require(project, anchor)
        elif anchor == '<AdditionalDependencies>dinput8.lib;':
            if anchor not in project and "$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;" not in project:
                _require(project, anchor)
        else:
            _require(project, anchor)
    if "RevivalRenderingDevice.hpp" not in project:
        project = project.replace(
            '<ClInclude Include="Common.h" />',
            '<ClInclude Include="Common.h" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalRenderingDevice.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\windows\\RevivalWglHost.hpp" />',
            1,
        )
    elif "NesTileObserver.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />',
            1,
        )
    if "NesCaptureWriter.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />',
            1,
        )
    if "NesInputMapper.hpp" not in project:
        project = project.replace(
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />',
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.hpp" />\n'
            '    <ClInclude Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesInputMapper.hpp" />',
            1,
        )
    if "NesTileObserver.cpp" not in project:
        if "RevivalRenderingDevice.cpp" in project:
            project = project.replace(
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />',
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />',
                1,
            )
        else:
            project = project.replace(
                '<ClCompile Include="EmuApiWrapper.cpp" />',
                '<ClCompile Include="EmuApiWrapper.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalRenderingDevice.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\RevivalFrameContract.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserver.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />\n'
                '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\windows\\RevivalWglHost.cpp" />',
                1,
            )
    if "NesCaptureWriter.cpp" not in project:
        project = project.replace(
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />',
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesTileObserverMesen.cpp" />\n'
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />',
            1,
        )
    if "NesInputMapper.cpp" not in project:
        project = project.replace(
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />',
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesCaptureWriter.cpp" />\n'
            '    <ClCompile Include="$(ProjectDir)..\\..\\..\\nes-bridge\\core\\NesInputMapper.cpp" />',
            1,
        )
    if "nes-bridge\\core;" not in project:
        project = project.replace(
            '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>',
            '<AdditionalIncludeDirectories>$(ProjectDir)..\\..\\..\\nes-bridge\\core;$(ProjectDir)..\\..\\..\\nes-bridge\\windows;$(ProjectDir)..\\..\\..\\native-graphics\\include;%(AdditionalIncludeDirectories)\n      </AdditionalIncludeDirectories>',
            4,
        )
    if "REVIVAL_MESEN_BRIDGE;WIN32;" not in project:
        project = project.replace(
            '<PreprocessorDefinitions>WIN32;',
            '<PreprocessorDefinitions>REVIVAL_MESEN_BRIDGE;WIN32;',
        )
    if "$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;" not in project:
        project = project.replace(
            '<AdditionalDependencies>dinput8.lib;',
            '<AdditionalDependencies>$(REVIVAL_GRAPHICS_LIB);opengl32.lib;dinput8.lib;',
        )
    return project


def patch_core_project(project: str) -> str:
    anchor = '<AdditionalIncludeDirectories>\n      </AdditionalIncludeDirectories>'
    correct = "$(ProjectDir)..\\..\\..\\nes-bridge\\core;"
    wrong = "$(SolutionDir)..\\..\\..\\nes-bridge\\core;"
    if correct in project:
        if wrong in project:
            return project.replace(wrong, correct)
        return project
    if wrong in project:
        return project.replace(wrong, correct)
    _require(project, anchor)
    return project.replace(
        anchor,
        '<AdditionalIncludeDirectories>$(ProjectDir)..\\..\\..\\nes-bridge\\core;%(AdditionalIncludeDirectories)\n      </AdditionalIncludeDirectories>',
    )


def patch_hd_nes_ppu_header(source: str) -> str:
    anchor = '#include "NES/HdPacks/HdData.h"'
    _require(source, anchor)
    if "NesTileObserverMesen.hpp" not in source:
        source = source.replace(anchor, anchor + '\n#include "NesTileObserverMesen.hpp"', 1)
    if "rv_nes_observer_needs_raw_tiles()" not in source:
        source = source.replace(
            "if(_isChrRam) {",
            "if(_isChrRam || rv_nes_observer_needs_raw_tiles()) {",
        )
    return source


def patch_hd_nes_ppu_cpp(source: str) -> str:
    anchor = "\t_info = (_info == _screenInfo[0]) ? _screenInfo[1] : _screenInfo[0];"
    _require(source, anchor)
    if "rv_nes_observer_submit_screen(_info);" in source:
        return source
    return source.replace(
        anchor,
        "\trv_nes_observer_submit_screen(_info);\n\n" + anchor,
        1,
    )


def patch_video_renderer_header(source: str) -> str:
    anchor = "\tIRenderingDevice* _renderer = nullptr;"
    if "std::atomic<IRenderingDevice*> _renderer" in source:
        return source
    _require(source, anchor)
    if "#include <atomic>" not in source:
        source = source.replace("#include \"pch.h\"", "#include \"pch.h\"\n#include <atomic>", 1)
    return source.replace(anchor, "\tstd::atomic<IRenderingDevice*> _renderer{nullptr};", 1)


def patch_video_renderer_cpp(source: str) -> str:
    source = _patch_video_renderer_lifecycle(source)
    if '#include "NesRendererProbe.hpp"' in source:
        return source.replace(
            "if(probe.enabled()) probe.submit(started, uint64_t{frame.Width} * frame.Height * 4);",
            _PROBE_SUBMIT_SAMPLE, 1)
    include = '#include "Shared/Video/VideoRenderer.h"'
    destructor = "\tStopThread();\n}\n\nFrameInfo VideoRenderer::GetRendererSize()"
    render = "\t\t\t\tattached->Render(_emuHudSurface, _scriptHudSurface);"
    submit = "\t\trenderer->UpdateFrame(frame);"
    for anchor in (include, destructor, render, submit):
        _require(source, anchor)
    source = source.replace(include, include + '\n#ifdef _WIN32\n#include "NesRendererProbe.hpp"\n#endif', 1)
    source = source.replace(destructor, destructor.replace("\n}", """
#ifdef _WIN32
	revival::nes::renderer_probe().report();
#endif
}""", 1), 1)
    source = source.replace(render, """#ifdef _WIN32
				auto& probe = revival::nes::renderer_probe();
				const auto started = probe.enabled() ? revival::nes::NesRendererProbe::Clock::now() : revival::nes::NesRendererProbe::Tick{};
#endif
""" + render + """
#ifdef _WIN32
				if(probe.enabled()) probe.present(started);
#endif""", 1)
    return source.replace(submit, """#ifdef _WIN32
		auto& probe = revival::nes::renderer_probe();
		const auto started = probe.enabled() ? revival::nes::NesRendererProbe::Clock::now() : revival::nes::NesRendererProbe::Tick{};
#endif
""" + submit + """
#ifdef _WIN32
""" + _PROBE_SUBMIT_SAMPLE + """
#endif""", 1)


_PROBE_SUBMIT_SAMPLE = """	if(probe.enabled() && frame.FrameBuffer != nullptr && frame.Width && frame.Height) {
			const auto* pixels = static_cast<const uint32_t*>(frame.FrameBuffer);
			const auto center = static_cast<size_t>(frame.Height / 2) * frame.Width + frame.Width / 2;
			probe.submit(started, uint64_t{frame.Width} * frame.Height * 4, pixels[center]);
		}"""


def patch_stock_windows_renderer(source: str) -> str:
    include = '#include "../../../nes-bridge/core/NesRendererProbe.hpp"'
    if include in source:
        return source
    for anchor, addition in (
        ('#include "Common.h"', include),
        ("\t_pDeviceContext->Unmap(_pTexture, 0);",
         "\tif(auto& probe = revival::nes::renderer_probe(); probe.enabled()) {\n"
         "\t\tprobe.stock_upload(uint64_t{rowPitch} * _emuFrameHeight, true);\n\t}"),
        ("\t\t_pDeviceContext->Unmap(hud.Texture, 0);",
         "\t\tif(auto& probe = revival::nes::renderer_probe(); probe.enabled()) {\n"
         "\t\t\tprobe.stock_upload(uint64_t{rowPitch} * hud.Height, false);\n\t\t}"),
    ):
        _require(source, anchor)
        source = source.replace(anchor, anchor + "\n" + addition, 1)
    return source


def _patch_video_renderer_lifecycle(source: str) -> str:
    _require(source, "void VideoRenderer::RenderThread()")
    _require(source, "void VideoRenderer::RegisterRenderingDevice(IRenderingDevice* renderer)")
    if "IRenderingDevice* attached = nullptr;" in source:
        return source

    start = source.index("void VideoRenderer::RenderThread()")
    end = source.index("\nFrameInfo VideoRenderer::GetEmuHudSize", start)
    render_thread = """void VideoRenderer::RenderThread()
{
\tIRenderingDevice* attached = nullptr;

\tTimer lastFrameTimer;
\tbool needClearHud = false;
\twhile(!_stopFlag.load()) {
\t\t// Registration can happen after this thread starts; attach and detach on
\t\t// this owner thread so WGL and device callbacks stay serialized.
\t\tIRenderingDevice* current = _renderer.load(std::memory_order_acquire);
\t\tif(current != attached) {
\t\t\tif(attached != nullptr) {
\t\t\t\tattached->OnRendererThreadStopped();
\t\t\t}
\t\t\tattached = current;
\t\t\tif(attached != nullptr) {
\t\t\t\tattached->OnRendererThreadStarted();
\t\t\t}
\t\t}

\t\t//Wait until a frame is ready, or until 32ms have passed (to allow HUD to update at ~30fps when paused)
\t\tbool forceRender = !_waitForRender.Wait(32);
\t\tif(attached != nullptr) {
\t\t\tFrameInfo size = _emu->GetVideoDecoder()->GetBaseFrameInfo(true);
\t\t\t_scriptHudSurface.UpdateSize(size.Width * _scriptHudScale, size.Height * _scriptHudScale);

\t\t\tsize = GetEmuHudSize(size);
\t\t\tif(_emuHudSurface.UpdateSize(size.Width, size.Height)) {
\t\t\t\t_rendererHud->ClearScreen();
\t\t\t}

\t\t\tRenderedFrame frame;
\t\t\t{
\t\t\t\tauto lock = _frameLock.AcquireSafe();
\t\t\t\tframe = _lastFrame;
\t\t\t}

\t\t\tif(needClearHud) {
\t\t\t\t_emuHudSurface.Clear();
\t\t\t\t_rendererHud->ClearScreen();
\t\t\t}

\t\t\t_inputHud->DrawControllers(size, frame.InputData);

\t\t\t{
\t\t\t\tauto lock = _hudLock.AcquireSafe();
\t\t\t\t_systemHud->Draw(_rendererHud.get(), size.Width, size.Height);
\t\t\t}

\t\t\tbool showDebugInfo = _emu->GetSettings()->GetPreferences().ShowDebugInfo;
\t\t\tif(showDebugInfo) {
\t\t\t\tdouble lastFrameTime = lastFrameTimer.GetElapsedMS();
\t\t\t\tlastFrameTimer.Reset();
\t\t\t\t_emu->GetDebugStats()->UpdateStats(_emu, true, lastFrameTime);
\t\t\t\t_emu->GetDebugStats()->DisplayStats(_emu, _rendererHud.get());
\t\t\t\tneedClearHud = true;
\t\t\t}

\t\t\t_emuHudSurface.IsDirty = _rendererHud->Draw(_emuHudSurface.Buffer, size, {}, 0, {}, !needClearHud);
\t\t\t_scriptHudSurface.IsDirty = DrawScriptHud(frame);

\t\t\tneedClearHud = showDebugInfo;

\t\t\tif(forceRender || _needRedraw || _emuHudSurface.IsDirty || _scriptHudSurface.IsDirty) {
\t\t\t\t_needRedraw = false;
\t\t\t\tattached->Render(_emuHudSurface, _scriptHudSurface);
\t\t\t}
\t\t}
\t}

\tif(attached != nullptr) {
\t\tattached->OnRendererThreadStopped();
\t}
}
"""
    source = source[:start] + render_thread + source[end:]

    old_methods = """void VideoRenderer::UpdateFrame(RenderedFrame& frame)
{
\t{
\t\tauto lock = _hudLock.AcquireSafe();
\t\t_systemHud->UpdateHud();
\t}

\tProcessAviRecording(frame);

\t{
\t\tauto lock = _frameLock.AcquireSafe();
\t\t_lastFrame = frame;
\t}

\tif(_renderer) {
\t\t_renderer->UpdateFrame(frame);
\t\t_needRedraw = true;
\t\t_waitForRender.Signal();
\t}
}

void VideoRenderer::ClearFrame()
{
\tif(_renderer) {
\t\t_renderer->ClearFrame();
\t}
}

void VideoRenderer::RegisterRenderingDevice(IRenderingDevice* renderer)
{
\t_renderer = renderer;
\tStartThread();
}

void VideoRenderer::UnregisterRenderingDevice(IRenderingDevice* renderer)
{
\tif(_renderer == renderer) {
\t\tStopThread();
\t\t_renderer = nullptr;
\t}
}
"""
    new_methods = """void VideoRenderer::UpdateFrame(RenderedFrame& frame)
{
\t{
\t\tauto lock = _hudLock.AcquireSafe();
\t\t_systemHud->UpdateHud();
\t}

\tProcessAviRecording(frame);

\t{
\t\tauto lock = _frameLock.AcquireSafe();
\t\t_lastFrame = frame;
\t}

\tIRenderingDevice* renderer = _renderer.load(std::memory_order_acquire);
\tif(renderer != nullptr) {
\t\trenderer->UpdateFrame(frame);
\t\t_needRedraw = true;
\t\t_waitForRender.Signal();
\t}
}

void VideoRenderer::ClearFrame()
{
\tIRenderingDevice* renderer = _renderer.load(std::memory_order_acquire);
\tif(renderer != nullptr) {
\t\trenderer->ClearFrame();
\t}
}

void VideoRenderer::RegisterRenderingDevice(IRenderingDevice* renderer)
{
\t_renderer.store(renderer, std::memory_order_release);
\tStartThread();
\t_waitForRender.Signal();
}

void VideoRenderer::UnregisterRenderingDevice(IRenderingDevice* renderer)
{
\tif(_renderer.load(std::memory_order_acquire) == renderer) {
\t\tStopThread();
\t\t_renderer.store(nullptr, std::memory_order_release);
\t}
}
"""
    if old_methods not in source:
        raise ValueError("Mesen VideoRenderer callback block changed")
    return source.replace(old_methods, new_methods, 1)


def patch_emu_api(source: str) -> str:
    anchor = '[DllImport(DllPath)] public static extern void InitDll();'
    _require(source, anchor)
    declaration = '[DllImport(DllPath)] public static extern void SetRevivalRenderer([MarshalAs(UnmanagedType.I1)] bool enabled);'
    observer_declaration = '[DllImport(DllPath)] public static extern void SetRevivalNesObserver([MarshalAs(UnmanagedType.I1)] bool enabled);'
    stats_struct = """\t\t[StructLayout(LayoutKind.Sequential)]
\t\tpublic struct RevivalNesObserverStats
\t\t{
\t\t\tpublic UInt64 Frames;
\t\t\tpublic UInt64 Seen;
\t\t\tpublic UInt64 Unique;
\t\t\tpublic UInt64 Admitted;
\t\t\tpublic UInt64 Dropped;
\t\t\tpublic UInt32 Queued;
\t\t}
"""
    stats_declaration = '[DllImport(DllPath)] public static extern void GetRevivalNesObserverStats(out RevivalNesObserverStats stats);'
    hd_declaration = '[DllImport(DllPath)] public static extern void GetRevivalNesHdPlaybackStatus(out Revival.Mesen.ManagedHdNativeStatus status);'
    capture_declaration = ('[DllImport(DllPath, CharSet = CharSet.Unicode, ExactSpelling = true)] '
                           '[return: MarshalAs(UnmanagedType.I1)]\n\t\tpublic static extern bool StartRevivalNesCapture('
                           'string directory, [MarshalAs(UnmanagedType.LPStr)] string gameId, '
                           '[MarshalAs(UnmanagedType.LPStr)] string sessionId);')
    if declaration not in source:
        source = source.replace(anchor, anchor + "\n\n\t\t" + declaration, 1)
    if observer_declaration not in source:
        source = source.replace(declaration, declaration + "\n\n\t\t" + observer_declaration, 1)
    if stats_struct not in source:
        source = source.replace(observer_declaration, observer_declaration + "\n\n" + stats_struct, 1)
    if stats_declaration not in source:
        source = source.replace(stats_struct, stats_struct + "\n\t\t" + stats_declaration, 1)
    if hd_declaration not in source:
        source = source.replace(stats_declaration, stats_declaration + "\n\t\t" + hd_declaration, 1)
    if capture_declaration not in source:
        source = source.replace(hd_declaration, hd_declaration + "\n\t\t" + capture_declaration, 1)
    return source


def patch_program(source: str) -> str:
    source = source.replace("using Mesen.Utilities;", "using Mesen.Utilities;\nusing Revival.Mesen;", 1) \
        if "using Revival.Mesen;" not in source else source
    field_anchor = "\t\tpublic static string[] CommandLineArgs { get; private set; } = Array.Empty<string>();"
    _require(source, field_anchor)
    managed_property = field_anchor + "\n\t\tpublic static RevivalLaunchManifest? ManagedLaunch { get; private set; }"
    if "public static RevivalLaunchManifest? ManagedLaunch" not in source:
        source = source.replace(field_anchor, managed_property, 1)

    main_anchor = "\t\tpublic static int Main(string[] args)\n\t\t{\n"
    _require(source, main_anchor)
    if "RevivalLaunch.ReadArguments(args)" not in source:
        managed_parse = """\t\t\tRevivalLaunchManifest? managedLaunch;
			try {
				managedLaunch = RevivalLaunch.ReadArguments(args);
			} catch(Exception error) {
				Console.Error.WriteLine("[revival-managed] launch rejected: " + error.Message);
				return 2;
			}
			Program.ManagedLaunch = managedLaunch;

"""
        source = source.replace(main_anchor, main_anchor + managed_parse, 1)

    resolver_anchor = "\t\t\tif(!System.Diagnostics.Debugger.IsAttached) {"
    if resolver_anchor in source:
        source = source.replace(
            resolver_anchor,
            "\t\t\tif(Program.ManagedLaunch != null || !System.Diagnostics.Debugger.IsAttached) {",
            1,
        )
    elif "if(Program.ManagedLaunch != null || !System.Diagnostics.Debugger.IsAttached) {" not in source:
        _require(source, resolver_anchor)

    update_anchor = "\t\t\tif(args.Length >= 4 && args[0] == \"--update\") {"
    _require(source, update_anchor)
    if "[revival-managed] startup failed:" not in source:
        branch = """\t\t\tif(Program.ManagedLaunch is RevivalLaunchManifest launch) {
				try {
					launch.WriteStatus("starting");
					if(launch.Mode != "original" && !(launch.Mode == "enhanced" &&
						string.Equals(Environment.GetEnvironmentVariable("J2ME_REVIVAL_ENABLE_NES_ENHANCED"), "1", StringComparison.Ordinal))) {
						throw new InvalidDataException("Enhanced NES playback is experimental and requires J2ME_REVIVAL_ENABLE_NES_ENHANCED=1.");
					}
					if(launch.Mode == "enhanced" && !OperatingSystem.IsWindows()) {
						throw new InvalidDataException("Experimental Enhanced NES playback currently requires the Windows shared renderer.");
					}
					ConfigManager.SetManagedSession(launch);
					ManagedLaunchStartup.VerifyRuntime(launch);
					Environment.CurrentDirectory = ConfigManager.HomeFolder;
					DependencyHelper.ExtractNativeDependencies(ConfigManager.HomeFolder, includeMesenCore: false);
					Program.CommandLineArgs = Array.Empty<string>();
					BuildAvaloniaApp().StartWithClassicDesktopLifetime(Array.Empty<string>(), ShutdownMode.OnMainWindowClose);
					return 0;
				} catch(Exception error) {
					Console.Error.WriteLine("[revival-managed] startup failed: " + error.Message);
					try { launch.WriteStatus("error", error.Message); } catch { }
					return 3;
				}
			}

"""
        source = source.replace(update_anchor, branch + update_anchor, 1)

    load_anchor = "\t\t\t\treturn NativeLibrary.Load(Path.Combine(ConfigManager.HomeFolder, libraryName));"
    if load_anchor in source:
        source = source.replace(
            load_anchor,
            "\t\t\t\tstring runtimeRoot = Program.ManagedLaunch != null\n"
            "\t\t\t\t\t? AppContext.BaseDirectory : ConfigManager.HomeFolder;\n"
            "\t\t\t\treturn NativeLibrary.Load(Path.Combine(runtimeRoot, libraryName));",
            1,
        )
    elif "string runtimeRoot = Program.ManagedLaunch != null" not in source:
        _require(source, load_anchor)
    return source


def patch_config_manager(source: str) -> str:
    if "using Revival.Mesen;" not in source:
        _require(source, "using Mesen.Utilities;")
        source = source.replace("using Mesen.Utilities;", "using Mesen.Utilities;\nusing Revival.Mesen;", 1)
    anchor = "\t\tprivate static string? _homeFolder = null;"
    _require(source, anchor)
    if "private static RevivalLaunchManifest? _managedSession;" not in source:
        source = source.replace(anchor, anchor + "\n\t\tprivate static RevivalLaunchManifest? _managedSession;", 1)

    reset_anchor = "\t\tpublic static void ResetHomeFolder()\n\t\t{"
    _require(source, reset_anchor)
    if "public static void SetManagedSession(" not in source:
        managed_methods = """\t\tpublic static void SetManagedSession(RevivalLaunchManifest session)
		{
			if(_managedSession != null) {
				throw new InvalidOperationException("A managed session is already active.");
			}
			RevivalLaunch.EnsureNoLinkedSegments(session.Home);
			RevivalLaunch.EnsureNoLinkedSegments(session.Saves);
			Directory.CreateDirectory(session.Home);
			Directory.CreateDirectory(session.Saves);
			string battery = Path.Combine(session.Saves, "battery");
			string states = Path.Combine(session.Saves, "states");
			RevivalLaunch.EnsureNoLinkedSegments(battery);
			RevivalLaunch.EnsureNoLinkedSegments(states);
			Directory.CreateDirectory(battery);
			Directory.CreateDirectory(states);
			RevivalLaunch.EnsureNoLinkedSegments(session.Home);
			RevivalLaunch.EnsureNoLinkedSegments(battery);
			RevivalLaunch.EnsureNoLinkedSegments(states);
			_managedSession = session;
			_homeFolder = session.Home;
			DisableSaveSettings = true;
		}

		public static void ApplyManagedSessionConfig()
		{
			if(_managedSession == null) {
				throw new InvalidOperationException("No managed session is active.");
			}
			string battery = Path.Combine(_managedSession.Saves, "battery");
			string states = Path.Combine(_managedSession.Saves, "states");
			Config.Preferences.SaveDataFolder = battery;
			Config.Preferences.OverrideSaveDataFolder = true;
			Config.Preferences.SaveStateFolder = states;
			Config.Preferences.OverrideSaveStateFolder = true;
			string? stagedHdPack = new ManagedHdPackStager().Stage(_managedSession);
			Config.Nes.EnableHdPacks = stagedHdPack != null;
			Console.WriteLine(stagedHdPack == null
				? "[revival-managed] HD packs disabled for Original playback"
				: "[revival-managed] validated HD pack staged for this session");
		}

"""
        source = source.replace(reset_anchor, managed_methods + reset_anchor, 1)

    save_anchor = "\t\tpublic static string SaveFolder { get { return GetFolder(DefaultSaveDataFolder, Config.Preferences.SaveDataFolder, Config.Preferences.OverrideSaveDataFolder); } }"
    state_anchor = "\t\tpublic static string SaveStateFolder { get { return GetFolder(DefaultSaveStateFolder, Config.Preferences.SaveStateFolder, Config.Preferences.OverrideSaveStateFolder); } }"
    if save_anchor in source:
        source = source.replace(
            save_anchor,
            "\t\tpublic static string SaveFolder { get { return _managedSession != null ? Path.Combine(_managedSession.Saves, \"battery\") : GetFolder(DefaultSaveDataFolder, Config.Preferences.SaveDataFolder, Config.Preferences.OverrideSaveDataFolder); } }",
            1,
        )
    elif "_managedSession != null ? Path.Combine(_managedSession.Saves, \"battery\")" not in source:
        _require(source, save_anchor)
    if state_anchor in source:
        source = source.replace(
            state_anchor,
            "\t\tpublic static string SaveStateFolder { get { return _managedSession != null ? Path.Combine(_managedSession.Saves, \"states\") : GetFolder(DefaultSaveStateFolder, Config.Preferences.SaveStateFolder, Config.Preferences.OverrideSaveStateFolder); } }",
            1,
        )
    elif "_managedSession != null ? Path.Combine(_managedSession.Saves, \"states\")" not in source:
        _require(source, state_anchor)
    return source


def patch_dependency_helper(source: str) -> str:
    signature = "public static void ExtractNativeDependencies(string dest)"
    if signature in source:
        source = source.replace(signature, "public static void ExtractNativeDependencies(string dest, bool includeMesenCore = true)", 1)
    _require(source, "public static void ExtractNativeDependencies(string dest, bool includeMesenCore = true)")
    debug_start = source.find("#if DEBUG\n\t\t\ttry {")
    if debug_start >= 0:
        debug_end = source.find("\n#endif", debug_start)
        if debug_end < 0:
            raise ValueError("Mesen dependency extraction debug block changed")
        debug_body = source[debug_start + len("#if DEBUG\n"):debug_end]
        source = source[:debug_start] + "#if DEBUG\n\t\t\tif(includeMesenCore) {\n" + debug_body + "\n\t\t\t}\n#endif" + source[debug_end + len("\n#endif"):]
    entry_anchor = "\t\t\t\tif(entry.FullName.StartsWith(\"Internal\")) {"
    _require(source, entry_anchor)
    if "if(!includeMesenCore && string.Equals(Path.GetFileName(entry.FullName)" not in source:
        source = source.replace(
            entry_anchor,
            "\t\t\t\tif(!includeMesenCore && string.Equals(Path.GetFileName(entry.FullName), \"MesenCore.dll\", StringComparison.OrdinalIgnoreCase)) {\n\t\t\t\t\tcontinue;\n\t\t\t\t}\n\t\t\t\t" + entry_anchor.lstrip(),
            1,
        )
    return source


def patch_load_rom_helper(source: str) -> str:
    anchors = {
        "\t\tpublic static async void LoadRom(ResourcePath romPath, ResourcePath? patchPath = null)\n\t\t{":
            "\t\t\tif(Mesen.Program.ManagedLaunch != null) { return; }\n",
        "\t\tpublic static void LoadRecentGame(string filename, bool forceLoadState)\n\t\t{":
            "\t\t\tif(Mesen.Program.ManagedLaunch != null) { return; }\n",
        "\t\tpublic static async void LoadPatchFile(string patchFile)\n\t\t{":
            "\t\t\tif(Mesen.Program.ManagedLaunch != null) { return; }\n",
    }
    for anchor, addition in anchors.items():
        _require(source, anchor)
        if addition.strip() not in source:
            source = source.replace(anchor, anchor + "\n" + addition.rstrip(), 1)
    rom_else = "\t\t\t\t} else {\n\t\t\t\t\tLoadRom(filename);\n\t\t\t\t}"
    if "else if(Mesen.Program.ManagedLaunch != null)" in source:
        pass
    elif rom_else in source:
        source = source.replace(
            rom_else,
            "\t\t\t\t} else if(Mesen.Program.ManagedLaunch != null) {\n"
            "\t\t\t\t\treturn;\n"
            "\t\t\t\t} else {\n"
            "\t\t\t\t\tLoadRom(filename);\n"
            "\t\t\t\t}",
            1,
        )
    else:
        _require(source, rom_else)
    return source


def patch_nes_config_view_model(source: str) -> str:
    anchor = "\t\tpublic NesInputConfigViewModel Input { get; private set; }"
    _require(source, anchor)
    if "CanEditControllerMappings" not in source:
        source = source.replace(
            anchor,
            anchor + "\n\t\tpublic bool CanEditControllerMappings => global::Mesen.Program.ManagedLaunch == null;",
            1,
        )
    return source


def patch_nes_config_view(source: str) -> str:
    anchor = '<TabItem Header="{l:Translate tpgInput}">'
    managed_anchor = '<TabItem Header="{l:Translate tpgInput}" IsVisible="{Binding CanEditControllerMappings}">'
    if managed_anchor not in source:
        _require(source, anchor)
        source = source.replace(anchor, managed_anchor, 1)
    return source


def patch_main_window(source: str) -> str:
    if "using Revival.Mesen;" not in source:
        _require(source, "using Mesen.Views;")
        source = source.replace("using Mesen.Views;", "using Mesen.Views;\nusing Revival.Mesen;", 1)
    field_anchor = "\t\tprivate bool _isClosing = false;"
    _require(source, field_anchor)
    if "_managedCoreStarted;" not in source:
        source = source.replace(
            field_anchor,
            field_anchor + "\n\t\tprivate volatile bool _managedCoreStarted;\n\t\tprivate volatile bool _managedStartupFinished;\n\t\tprivate volatile bool _managedCloseRequested;\n\t\tprivate bool _managedLaunchFailed;\n\t\tprivate string? _managedErrorReason;\n\t\tprivate DispatcherTimer? _managedHdPlaybackTimer;\n\t\tprivate int _managedHdPlaybackPolls;\n\t\tprivate bool _managedHdFrameReported;",
            1,
        )
    closing_anchor = "\t\t\tbase.OnClosing(e);"
    _require(source, closing_anchor)
    if "_managedStartupFinished" not in source[source.index("protected override void OnClosing"):source.index("private bool CloseEmu")]:
        source = source.replace(
            closing_anchor,
            closing_anchor + "\n\t\t\tif(Program.ManagedLaunch != null && !_managedStartupFinished) {\n"
            "\t\t\t\te.Cancel = true;\n\t\t\t\t_managedCloseRequested = true;\n\t\t\t\treturn;\n\t\t\t}",
            1,
        )

    marker = '\t\t\t\tEmuApi.InitializeEmu(\n'
    _require(source, marker)
    if "[revival-observer] frames=" not in source:
        _require(source, "\t\t\tEmuApi.Release();")
        stats_log = """\t\t\tif(OperatingSystem.IsWindows() && string.Equals(Environment.GetEnvironmentVariable(\"J2ME_REVIVAL_MESEN_OBSERVER\"), \"1\", StringComparison.Ordinal)) {
\t\t\t\tEmuApi.RevivalNesObserverStats stats;
\t\t\t\tEmuApi.GetRevivalNesObserverStats(out stats);
\t\t\t\tConsole.WriteLine($\"[revival-observer] frames={stats.Frames} seen={stats.Seen} unique={stats.Unique} admitted={stats.Admitted} dropped={stats.Dropped} queued={stats.Queued}\");
\t\t\t}

"""
        source = source.replace("\t\t\tEmuApi.Release();", stats_log + "\t\t\tEmuApi.Release();", 1)
    if "J2ME_REVIVAL_MESEN_OBSERVER" not in source:
        observer_opt_in = """\t\t\t\tif(OperatingSystem.IsWindows() && string.Equals(Environment.GetEnvironmentVariable(\"J2ME_REVIVAL_MESEN_OBSERVER\"), \"1\", StringComparison.Ordinal)) {
\t\t\t\t\tEmuApi.SetRevivalNesObserver(true);
\t\t\t\t}

"""
        if "J2ME_REVIVAL_MESEN_RENDERER" in source:
            renderer_block = "\t\t\t\t\tEmuApi.SetRevivalRenderer(true);\n\t\t\t\t}\n\n"
            _require(source, renderer_block)
            source = source.replace(renderer_block, renderer_block + observer_opt_in, 1)
        else:
            opt_in = """\t\t\t\t// The bridge is opt-in so stock Mesen and normal Revival launches keep their renderer.
\t\t\t\tif(OperatingSystem.IsWindows() && string.Equals(Environment.GetEnvironmentVariable(\"J2ME_REVIVAL_MESEN_RENDERER\"), \"1\", StringComparison.Ordinal)) {
\t\t\t\t\tEmuApi.SetRevivalRenderer(true);
\t\t\t\t}
""" + observer_opt_in
            source = source.replace(marker, opt_in + marker, 1)

    opened_anchor = "\t\t\tTask.Run(() => {\n\t\t\t\tCommandLineHelper cmdLine = new CommandLineHelper(Program.CommandLineArgs, true);"
    _require(source, opened_anchor)
    if "InitializeManagedLaunch(managedLaunch)" not in source:
        managed_branch = """\t\t\tif(Program.ManagedLaunch is RevivalLaunchManifest managedLaunch) {
\t\t\t\tTask.Run(() => InitializeManagedLaunch(managedLaunch));
\t\t\t\treturn;
\t\t\t}

"""
        source = source.replace(opened_anchor, managed_branch + opened_anchor, 1)

    instance_anchor = "\t\tprivate void Instance_ArgumentsReceived(object? sender, ArgumentsReceivedEventArgs e)"
    _require(source, instance_anchor)
    if "private void InitializeManagedLaunch(RevivalLaunchManifest launch)" not in source:
        method = """\t\tprivate void InitializeManagedLaunch(RevivalLaunchManifest launch)
\t\t{
\t\t\ttry {
\t\t\t\t// Original playback uses Mesen's renderer while the common viewer is experimental.
\t\t\t\tbool useCommonRenderer = launch.Mode == "enhanced" || string.Equals(Environment.GetEnvironmentVariable("J2ME_REVIVAL_MESEN_RENDERER"), "1", StringComparison.Ordinal);
\t\t\t\tEmuApi.SetRevivalRenderer(useCommonRenderer);
\t\t\t\tConsole.WriteLine(useCommonRenderer ? "[revival-managed] renderer: common (opt-in)" : "[revival-managed] renderer: Mesen");
\t\t\t\t_managedCoreStarted = true;
\t\t\t\tEmuApi.InitializeEmu(
\t\t\t\t\tConfigManager.HomeFolder,
\t\t\t\t\tTryGetPlatformHandle()?.Handle ?? IntPtr.Zero,
\t\t\t\t\t_renderer.Handle,
\t\t\t\t\t_usesSoftwareRenderer,
\t\t\t\t\tfalse,
\t\t\t\t\tfalse,
\t\t\t\t\tfalse
\t\t\t\t);

\t\t\t\tConfigManager.Config.RemoveObsoleteConfig();
\t\t\t\tConfigManager.Config.InitializeDefaults();
\t\t\t\tConfigManager.Config.UpgradeConfig();
\t\t\t\tConfigManager.ApplyManagedSessionConfig();

\t\t\t\t_listener = new NotificationListener();
\t\t\t\t_listener.OnNotification += OnNotification;
\t\t\t\t_model.Init(this);
\t\t\t\tApplyManagedControlMappings(launch);
\t\t\t\tConfigManager.Config.ApplyConfig();
\t\t\t\tConfigApi.CheckShaderSupport();

\t\t\t\tif(!EmuApi.LoadRom(launch.RomPath, String.Empty)) {
\t\t\t\t\tthrow new InvalidOperationException(\"Mesen rejected the validated NES ROM.\");
\t\t\t\t}
\t\t\t\tlaunch.WriteStatus(\"rom-loaded\");
\t\t\t\tConsole.WriteLine("[revival-managed] ROM loaded");
\t\t\t\tif(launch.Mode == "enhanced") {
\t\t\t\t\tif(!_managedCloseRequested) TryStartManagedNesCapture(launch);
\t\t\t\t\tDispatcher.UIThread.Post(() => StartManagedHdPlaybackStatus(launch));
\t\t\t\t}
\t\t\t} catch(Exception error) {
\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t_managedErrorReason = error.Message;
\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] game startup failed: \" + error.Message);
\t\t\t\ttry { launch.WriteStatus(\"error\", error.Message); } catch { }
\t\t\t\tDispatcher.UIThread.Post(() => {
\t\t\t\t\t_needCloseValidation = false;
\t\t\t\t\tClose();
\t\t\t\t});
\t\t\t}
\t\t}

"""
        source = source.replace(instance_anchor, method + instance_anchor, 1)

    if "private void ApplyManagedControlMappings(RevivalLaunchManifest launch)" not in source:
        mapping_method = """\t\tprivate void ApplyManagedControlMappings(RevivalLaunchManifest launch)
		{
			// Resolve every physical key before changing the controller configuration.
			NesControlCodes keys = RevivalNesControls.Resolve(launch.Controls, InputApi.GetKeyCode);
			var keyboard = new NesKeyMapping() {
				Up = keys.Up,
				Down = keys.Down,
				Left = keys.Left,
				Right = keys.Right,
				A = keys.A,
				B = keys.B,
				Start = keys.Start,
				Select = keys.Select
			};
			var xbox = new NesKeyMapping();
			xbox.SetDefaultKeys(ControllerType.NesController, KeyPresetType.XboxP1);
			var ps4 = new NesKeyMapping();
			if(ConfigManager.Config.DefaultKeyMappings.HasFlag(DefaultKeyMappingType.Ps4)) {
				ps4.SetDefaultKeys(ControllerType.NesController, KeyPresetType.Ps4P1);
			}
			var unused = new NesKeyMapping();

			NesControllerConfig port1 = ConfigManager.Config.Nes.Port1;
			port1.Mapping1 = keyboard;
			port1.Mapping2 = xbox;
			port1.Mapping3 = ps4;
			port1.Mapping4 = unused;
			Console.WriteLine("[revival-managed] applied NES keyboard profile and native gamepad presets");
		}

"""
        source = source.replace(instance_anchor, mapping_method + instance_anchor, 1)

    if "private static void TryStartManagedNesCapture" not in source:
        capture_method = """\t\tprivate static void TryStartManagedNesCapture(RevivalLaunchManifest launch)
\t\t{
\t\t\tif(!string.Equals(Environment.GetEnvironmentVariable("J2ME_REVIVAL_NES_CAPTURE"), "1", StringComparison.Ordinal)) return;
\t\t\ttry {
\t\t\t\t// The validated launch home sits three levels below the game's runtime inbox.
\t\t\t\tstring inbox = System.IO.Path.GetFullPath(System.IO.Path.Combine(launch.Home, "..", "..", "..", "nes-capture"));
\t\t\t\tRevivalLaunch.EnsureNoLinkedSegments(inbox);
\t\t\t\tif(EmuApi.StartRevivalNesCapture(inbox, launch.GameId, launch.SessionId)) {
\t\t\t\t\tConsole.WriteLine("[revival-capture] enabled");
\t\t\t\t} else {
\t\t\t\t\tConsole.Error.WriteLine("[revival-capture] unavailable: no accepted HD pack or writer startup failed");
\t\t\t\t}
\t\t\t} catch(Exception error) {
\t\t\t\tConsole.Error.WriteLine("[revival-capture] unavailable: " + error.Message);
\t\t\t}
\t\t}

"""
        source = source.replace(instance_anchor, capture_method + instance_anchor, 1)

    if "private void StartManagedHdPlaybackStatus(RevivalLaunchManifest launch)" not in source:
        status_method = """\t\tprivate void StartManagedHdPlaybackStatus(RevivalLaunchManifest launch)
\t\t{
\t\t\tif(_isClosing || _managedCloseRequested) return;
\t\t\tstring expectedBuildId;
\t\t\ttry {
\t\t\t\texpectedBuildId = ManagedLaunchStartup.ReadExpectedAbiBuildId(launch.GraphicsManifest);
\t\t\t} catch(Exception error) {
\t\t\t\tConsole.Error.WriteLine("[revival-hd] expected graphics identity unavailable: " + error.Message);
\t\t\t\ttry { launch.WriteStatus("enhanced-frame-check-unverified", error.Message); } catch { }
\t\t\t\treturn;
\t\t\t}
\t\t\t_managedHdPlaybackPolls = 0;
\t\t\t_managedHdFrameReported = false;
\t\t\t_managedHdPlaybackTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(250) };
\t\t\t_managedHdPlaybackTimer.Tick += (_, _) => {
\t\t\t\tif(_isClosing || _managedCloseRequested) {
\t\t\t\t\t_managedHdPlaybackTimer?.Stop();
\t\t\t\t\t_managedHdPlaybackTimer = null;
\t\t\t\t\treturn;
\t\t\t\t}
\t\t\t\ttry {
\t\t\t\t\tManagedHdNativeStatus native;
\t\t\t\t\tEmuApi.GetRevivalNesHdPlaybackStatus(out native);
\t\t\t\t\tvar snapshot = new ManagedHdFrameStatus(native.PackLoaded, native.PackScale,
\t\t\t\t\t\tnative.CommonRendererActive, native.SubmittedFrames, native.PresentedFrames,
\t\t\t\t\t\tnative.FrameWidth, native.FrameHeight);
\t\t\t\t\tvar identity = new ManagedGraphicsIdentity(native.IdentityAvailable == 1,
\t\t\t\t\t\tnative.GraphicsAbiVersion, native.GraphicsBuildId, native.AdapterVendor,
\t\t\t\t\t\tnative.AdapterRenderer, native.AdapterVersion);
\t\t\t\t\tif(native.StructBytes != 640 || native.AbiVersion != 2) {
\t\t\t\t\t\tthrow new InvalidDataException($"Unsupported native readiness ABI {native.AbiVersion}/{native.StructBytes}.");
\t\t\t\t\t}
\t\t\t\t\tif(ManagedHdPlaybackStatus.IsHdFrameReady(snapshot, launch.PackScale) &&
\t\t\t\t\t\t!_managedHdFrameReported) {
\t\t\t\t\t\tstring frameDetails = ManagedHdPlaybackStatus.Describe(snapshot);
\t\t\t\t\t\tConsole.WriteLine("[revival-hd] HD frame detected: " + frameDetails);
\t\t\t\t\t\tlaunch.WriteStatus("enhanced-hd-frame-detected", frameDetails);
\t\t\t\t\t\t_managedHdFrameReported = true;
\t\t\t\t\t}
\t\t\t\t\tif(ManagedHdPlaybackStatus.IsReady(snapshot, identity, launch.PackScale, expectedBuildId)) {
\t\t\t\t\t\tstring details = ManagedHdPlaybackStatus.Describe(snapshot, identity, expectedBuildId);
\t\t\t\t\t\tConsole.WriteLine("[revival-hd] readiness verified: " + details);
\t\t\t\t\t\tlaunch.WriteStatus("enhanced-readiness-verified", details);
\t\t\t\t\t\t_managedHdPlaybackTimer.Stop();
\t\t\t\t\t\t_managedHdPlaybackTimer = null;
\t\t\t\t\t\treturn;
\t\t\t\t\t}
\t\t\t\t\t_managedHdPlaybackPolls++;
\t\t\t\t\tif(_managedHdPlaybackPolls >= 80) {
\t\t\t\t\t\tstring details = ManagedHdPlaybackStatus.Describe(snapshot, identity, expectedBuildId);
\t\t\t\t\t\tConsole.Error.WriteLine("[revival-hd] readiness unverified: " + details);
\t\t\t\t\t\tlaunch.WriteStatus("enhanced-frame-check-unverified", details);
\t\t\t\t\t\t_managedHdPlaybackTimer.Stop();
\t\t\t\t\t\t_managedHdPlaybackTimer = null;
\t\t\t\t\t}
\t\t\t\t} catch(Exception error) {
\t\t\t\t\tConsole.Error.WriteLine("[revival-hd] readiness query failed: " + error.Message);
\t\t\t\t\ttry { launch.WriteStatus("enhanced-frame-check-unverified", error.Message); } catch { }
\t\t\t\t\t_managedHdPlaybackTimer?.Stop();
\t\t\t\t\t_managedHdPlaybackTimer = null;
\t\t\t\t}
\t\t\t};
\t\t\t_managedHdPlaybackTimer.Start();
\t\t}

"""
        source = source.replace(instance_anchor, status_method + instance_anchor, 1)

    close_anchor = "\t\t\t_timerBackgroundFlag.Stop();\n\t\t\tEmuApi.Stop();\n\t\t\t_listener?.Dispose();"
    if "launch.WriteStatus(\"flushing-saves\")" not in source:
        _require(source, close_anchor)
        close_branch = """\t\t\t_timerBackgroundFlag.Stop();
\t\t\tif(Program.ManagedLaunch is RevivalLaunchManifest launch) {
\t\t\t\tbool released = !_managedCoreStarted;
\t\t\t\tif(_managedCoreStarted) {
\t\t\t\t\ttry {
\t\t\t\t\t\tlaunch.WriteStatus(\"flushing-saves\");
\t\t\t\t\t} catch(Exception statusError) {
\t\t\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t\t\t_managedErrorReason = statusError.Message;
\t\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] flush status failed: \" + statusError.Message);
\t\t\t\t\t}
\t\t\t\t\ttry { EmuApi.Stop(); } catch(Exception error) {
\t\t\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t\t\t_managedErrorReason = error.Message;
\t\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] stop failed: \" + error.Message);
\t\t\t\t\t}
\t\t\t\t\ttry { _listener?.Dispose(); } catch(Exception error) {
\t\t\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t\t\t_managedErrorReason = error.Message;
\t\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] listener disposal failed: \" + error.Message);
\t\t\t\t\t}
\t\t\t\t\ttry {
\t\t\t\t\t\tEmuApi.Release();
\t\t\t\t\t\t_managedCoreStarted = false;
\t\t\t\t\t\treleased = true;
\t\t\t\t\t} catch(Exception error) {
\t\t\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t\t\t_managedErrorReason = error.Message;
\t\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] release failed: \" + error.Message);
\t\t\t\t\t}
\t\t\t\t} else {
\t\t\t\t\ttry { _listener?.Dispose(); } catch(Exception error) {
\t\t\t\t\t\t_managedLaunchFailed = true;
\t\t\t\t\t\t_managedErrorReason = error.Message;
\t\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] listener disposal failed: \" + error.Message);
\t\t\t\t\t}
\t\t\t\t}
\t\t\t\ttry {
\t\t\t\t\tif(released && !_managedLaunchFailed) {
\t\t\t\t\t\tlaunch.WriteStatus(\"stopped\");
\t\t\t\t\t} else {
\t\t\t\t\t\tlaunch.WriteStatus(\"error\", _managedErrorReason ?? \"Managed session ended before a clean stop.\");
\t\t\t\t\t}
\t\t\t\t} catch(Exception error) {
\t\t\t\t\tConsole.Error.WriteLine(\"[revival-managed] shutdown status failed: \" + error.Message);
\t\t\t\t}
\t\t\t} else {
\t\t\t\tEmuApi.Stop();
\t\t\t\t_listener?.Dispose();
"""
        close_branch = close_branch.replace(
            "\t\t\t_timerBackgroundFlag.Stop();",
            "\t\t\t_timerBackgroundFlag.Stop();\n\t\t\t_managedHdPlaybackTimer?.Stop();\n\t\t\t_managedHdPlaybackTimer = null;",
            1,
        )
        source = source.replace(close_anchor, close_branch, 1)
        release_anchor = "\t\t\tEmuApi.Release();\n\t\t\tConfigManager.Config.MainWindow.SaveWindowSettings(this);"
        _require(source, release_anchor)
        source = source.replace(
            release_anchor,
            "\t\t\t\tEmuApi.Release();\n\t\t\t}\n\t\t\tConfigManager.Config.MainWindow.SaveWindowSettings(this);",
            1,
        )

    close_early_anchor = "\t\t\tif(!force && ApplicationHelper.GetOpenedWindows().Count > 1) {\n\t\t\t\treturn false;\n\t\t\t}\n\n\t\t\t_timerBackgroundFlag.Stop();"
    if "_managedCloseRequested = true;\n\t\t\t\treturn false;" not in source:
        _require(source, close_early_anchor)
        source = source.replace(
            close_early_anchor,
            close_early_anchor.replace("\n\n\t\t\t_timerBackgroundFlag.Stop();", "\n\n\t\t\tif(Program.ManagedLaunch != null && !_managedStartupFinished) {\n\t\t\t\t_managedCloseRequested = true;\n\t\t\t\treturn false;\n\t\t\t}\n\n\t\t\t_timerBackgroundFlag.Stop();"),
            1,
        )

    startup_end = """\t\t\t\tDispatcher.UIThread.Post(() => {
					_needCloseValidation = false;
					Close();
				});
			}
		}

"""
    if startup_end in source:
        source = source.replace(
            startup_end,
            """\t\t\t}
			finally {
				_managedStartupFinished = true;
				if(_managedLaunchFailed || _managedCloseRequested) {
					Dispatcher.UIThread.Post(() => {
						_needCloseValidation = false;
						Close();
					});
				}
			}
		}

""",
            1,
        )

    return source

def generate(source_root: Path, output_root: Path) -> None:
    metadata = json.loads(UPSTREAM.read_text(encoding="utf-8"))
    factory_path = source_root / metadata["factory_source"]
    project_path = source_root / metadata["interop_project"]
    core_project_path = source_root / metadata["core_project"]
    video_header_path = source_root / metadata["video_renderer_header"]
    video_cpp_path = source_root / metadata["video_renderer_source"]
    stock_renderer_path = source_root / metadata["stock_windows_renderer_source"]
    hd_ppu_header_path = source_root / metadata["hd_nes_ppu_header"]
    hd_ppu_cpp_path = source_root / metadata["hd_nes_ppu_source"]
    nes_console_header_path = source_root / metadata["nes_console_header"]
    nes_console_cpp_path = source_root / metadata["nes_console_source"]
    nes_config_view_model_path = source_root / metadata["managed_nes_config_view_model"]
    nes_config_view_path = source_root / metadata["managed_nes_config_view"]
    emu_api_path = source_root / metadata["managed_emu_api"]
    main_window_path = source_root / metadata["managed_main_window"]
    program_path = source_root / metadata["managed_program"]
    config_manager_path = source_root / metadata["managed_config_manager"]
    dependency_helper_path = source_root / metadata["managed_dependency_helper"]
    load_rom_helper_path = source_root / metadata["managed_load_rom_helper"]
    factory = factory_path.read_text(encoding="utf-8")
    project = project_path.read_text(encoding="utf-8")
    core_project = core_project_path.read_text(encoding="utf-8")
    video_header = video_header_path.read_text(encoding="utf-8")
    video_cpp = video_cpp_path.read_text(encoding="utf-8")
    hd_ppu_header = hd_ppu_header_path.read_text(encoding="utf-8")
    hd_ppu_cpp = hd_ppu_cpp_path.read_text(encoding="utf-8")
    nes_console_header = nes_console_header_path.read_text(encoding="utf-8")
    nes_console_cpp = nes_console_cpp_path.read_text(encoding="utf-8")
    nes_config_view_model = nes_config_view_model_path.read_text(encoding="utf-8")
    nes_config_view = nes_config_view_path.read_text(encoding="utf-8")
    emu_api = emu_api_path.read_text(encoding="utf-8")
    main_window = main_window_path.read_text(encoding="utf-8")
    program = program_path.read_text(encoding="utf-8")
    config_manager = config_manager_path.read_text(encoding="utf-8")
    dependency_helper = dependency_helper_path.read_text(encoding="utf-8")
    load_rom_helper = load_rom_helper_path.read_text(encoding="utf-8")
    for anchor in metadata["anchors"]:
        _require(factory if anchor in factory else project, anchor)
    patched_factory = patch_factory(factory)
    patched_project = patch_project(project)
    patched_core_project = patch_core_project(core_project)
    patched_video_header = patch_video_renderer_header(video_header)
    patched_video_cpp = patch_video_renderer_cpp(video_cpp)
    patched_hd_ppu_header = patch_hd_nes_ppu_header(hd_ppu_header)
    patched_hd_ppu_cpp = patch_hd_nes_ppu_cpp(hd_ppu_cpp)
    patched_nes_console_header = patch_nes_console_header(nes_console_header)
    patched_nes_console_cpp = patch_nes_console_cpp(nes_console_cpp)
    patched_nes_config_view_model = patch_nes_config_view_model(nes_config_view_model)
    patched_nes_config_view = patch_nes_config_view(nes_config_view)
    patched_emu_api = patch_emu_api(emu_api)
    patched_main_window = patch_main_window(main_window)
    patched_program = patch_program(program)
    patched_config_manager = patch_config_manager(config_manager)
    patched_dependency_helper = patch_dependency_helper(dependency_helper)
    patched_load_rom_helper = patch_load_rom_helper(load_rom_helper)
    (output_root / metadata["factory_source"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["interop_project"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["factory_source"]).write_text(patched_factory, encoding="utf-8")
    (output_root / metadata["interop_project"]).write_text(patched_project, encoding="utf-8")
    (output_root / metadata["core_project"]).parent.mkdir(parents=True, exist_ok=True)
    (output_root / metadata["core_project"]).write_text(patched_core_project, encoding="utf-8")
    for relative, content in (
        (metadata["video_renderer_header"], patched_video_header),
        (metadata["video_renderer_source"], patched_video_cpp),
        (metadata["stock_windows_renderer_source"],
         patch_stock_windows_renderer(stock_renderer_path.read_text(encoding="utf-8"))),
        (metadata["hd_nes_ppu_header"], patched_hd_ppu_header),
        (metadata["hd_nes_ppu_source"], patched_hd_ppu_cpp),
        (metadata["nes_console_header"], patched_nes_console_header),
        (metadata["nes_console_source"], patched_nes_console_cpp),
        (metadata["managed_emu_api"], patched_emu_api),
        (metadata["managed_main_window"], patched_main_window),
        (metadata["managed_program"], patched_program),
        (metadata["managed_config_manager"], patched_config_manager),
        (metadata["managed_dependency_helper"], patched_dependency_helper),
        (metadata["managed_load_rom_helper"], patched_load_rom_helper),
        (metadata["managed_nes_config_view_model"], patched_nes_config_view_model),
        (metadata["managed_nes_config_view"], patched_nes_config_view),
    ):
        target = output_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

    for name in ("managed_launch_reader", "managed_launch_startup", "managed_nes_controls",
                 "managed_hd_pack_validator", "managed_hd_pack_stager", "managed_hd_playback_status"):
        relative = metadata[name]
        target = output_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        source = ROOT / "nes-bridge" / "managed" / Path(relative).name
        target.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    args = parser.parse_args()
    generate(args.source_root.resolve(), args.output_root.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
