"""Patch video presentation, HD capture, and renderer lifecycle."""

from .anchors import _require


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


_PROBE_SUBMIT_SAMPLE = """	if(probe.enabled() && frame.FrameBuffer != nullptr && frame.Width && frame.Height) {
			const auto* pixels = static_cast<const uint32_t*>(frame.FrameBuffer);
			const auto center = static_cast<size_t>(frame.Height / 2) * frame.Width + frame.Width / 2;
			probe.submit(started, uint64_t{frame.Width} * frame.Height * 4, pixels[center]);
		}"""


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
