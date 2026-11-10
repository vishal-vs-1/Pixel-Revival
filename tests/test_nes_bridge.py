from pathlib import Path

from tools.patch_mesen import (
    patch_config_manager,
    patch_dependency_helper,
    patch_factory,
    patch_emu_api,
    patch_load_rom_helper,
    patch_main_window,
    patch_nes_config_view,
    patch_nes_config_view_model,
    patch_nes_console_header,
    patch_nes_console_cpp,
    patch_program,
    patch_project,
    patch_core_project,
    patch_hd_nes_ppu_cpp,
    patch_hd_nes_ppu_header,
    patch_stock_windows_renderer,
    patch_video_renderer_cpp,
    patch_video_renderer_header,
    generate,
)


ROOT = Path(__file__).resolve().parents[1]
MESEN = ROOT / "vendor" / "mesen-ce"


def test_mesen_factory_patch_is_opt_in_and_idempotent():
    source = (MESEN / "InteropDLL" / "EmuApiWrapper.cpp").read_text(encoding="utf-8")
    patched = patch_factory(source)
    assert "RevivalRenderingDevice.hpp" in patched
    assert "SetRevivalRenderer" in patched
    assert "_revivalRenderer" in patched
    assert "RegisterRenderingDevice(renderer)" in patched
    assert "GetRevivalNesHdPlaybackStatus" in patched
    assert "HasManagedHdPack" in patched
    release = patched.split("DllExport void __stdcall Release()", 1)[1]
    assert release.index("_emu->Stop(true)") < release.index("RevivalNesCapture().stop()") < release.index("_emu->Release()")
    assert patch_factory(patched) == patched


def test_mesen_project_patch_wires_the_common_dependency():
    project = (MESEN / "InteropDLL" / "InteropDLL.vcxproj").read_text(encoding="utf-8")
    patched = patch_project(project)
    assert "RevivalRenderingDevice.cpp" in patched
    assert "RevivalWglHost.cpp" in patched
    assert "NesTileObserver.cpp" in patched
    assert "NesCaptureWriter.cpp" in patched
    assert "NesCaptureWriter.hpp" in patched
    assert "NesInputMapper.cpp" in patched
    assert "NesInputMapper.hpp" in patched
    assert "REVIVAL_MESEN_BRIDGE" in patched
    assert "$(REVIVAL_GRAPHICS_LIB)" in patched
    assert patch_project(patched) == patched


def test_mesen_core_project_patch_exposes_the_observer_header():
    project = (MESEN / "Core" / "Core.vcxproj").read_text(encoding="utf-8")
    patched = patch_core_project(project)
    assert "$(ProjectDir)..\\..\\..\\nes-bridge\\core;" in patched
    assert patch_core_project(patched) == patched


def test_mesen_video_renderer_patch_handles_late_registration_on_owner_thread():
    header = (MESEN / "Core" / "Shared" / "Video" / "VideoRenderer.h").read_text(encoding="utf-8")
    source = (MESEN / "Core" / "Shared" / "Video" / "VideoRenderer.cpp").read_text(encoding="utf-8")
    patched_header = patch_video_renderer_header(header)
    patched_source = patch_video_renderer_cpp(source)
    assert "std::atomic<IRenderingDevice*> _renderer" in patched_header
    assert "IRenderingDevice* attached = nullptr;" in patched_source
    assert "OnRendererThreadStarted" in patched_source
    assert "_renderer.load(std::memory_order_acquire)" in patched_source
    assert '#include "NesRendererProbe.hpp"' in patched_source
    assert "if(probe.enabled()) probe.present(started);" in patched_source
    assert "probe.submit(started, uint64_t{frame.Width} * frame.Height * 4, pixels[center]);" in patched_source
    assert patch_video_renderer_header(patched_header) == patched_header
    assert patch_video_renderer_cpp(patched_source) == patched_source


def test_mesen_stock_probe_counts_completed_texture_copies():
    source = '\n'.join(('#include "Common.h"',
                        '\t_pDeviceContext->Unmap(_pTexture, 0);',
                        '\t\t_pDeviceContext->Unmap(hud.Texture, 0);'))
    patched = patch_stock_windows_renderer(source)
    assert patched.count("probe.enabled()") == 2
    assert "probe.stock_upload(uint64_t{rowPitch} * _emuFrameHeight, true);" in patched
    assert "probe.stock_upload(uint64_t{rowPitch} * hud.Height, false);" in patched
    assert patch_stock_windows_renderer(patched) == patched


def test_mesen_hd_ppu_patch_submits_passive_tile_metadata_and_copies_raw_tiles():
    header = (MESEN / "Core" / "NES" / "HdPacks" / "HdNesPpu.h").read_text(encoding="utf-8")
    source = (MESEN / "Core" / "NES" / "HdPacks" / "HdNesPpu.cpp").read_text(encoding="utf-8")
    patched_header = patch_hd_nes_ppu_header(header)
    patched_source = patch_hd_nes_ppu_cpp(source)
    assert "NesTileObserverMesen.hpp" in patched_header
    assert "rv_nes_observer_needs_raw_tiles()" in patched_header
    assert "rv_nes_observer_submit_screen(_info);" in patched_source
    assert patch_hd_nes_ppu_header(patched_header) == patched_header
    assert patch_hd_nes_ppu_cpp(patched_source) == patched_source


def test_mesen_managed_opt_in_is_guarded_and_uses_native_bool_marshalling():
    emu_api = (MESEN / "UI" / "Interop" / "EmuApi.cs").read_text(encoding="utf-8")
    main_window = (MESEN / "UI" / "Windows" / "MainWindow.axaml.cs").read_text(encoding="utf-8")
    patched_api = patch_emu_api(emu_api)
    patched_window = patch_main_window(main_window)
    assert "SetRevivalRenderer([MarshalAs(UnmanagedType.I1)] bool enabled)" in patched_api
    assert "SetRevivalNesObserver([MarshalAs(UnmanagedType.I1)] bool enabled)" in patched_api
    assert "GetRevivalNesObserverStats(out RevivalNesObserverStats stats)" in patched_api
    assert "GetRevivalNesHdPlaybackStatus(out Revival.Mesen.ManagedHdNativeStatus status)" in patched_api
    assert "public struct RevivalNesObserverStats" in patched_api
    assert "out Revival.Mesen.ManagedHdNativeStatus status" in patched_api
    assert "CharSet.Unicode, ExactSpelling = true" in patched_api
    assert "StartRevivalNesCapture(" in patched_api
    assert 'if(launch.Mode == "enhanced") {\n\t\t\t\t\tif(!_managedCloseRequested) TryStartManagedNesCapture(launch);' in patched_window
    assert 'GetEnvironmentVariable("J2ME_REVIVAL_NES_CAPTURE"), "1"' in patched_window
    assert "J2ME_REVIVAL_MESEN_RENDERER" in patched_window
    assert "J2ME_REVIVAL_MESEN_OBSERVER" in patched_window
    assert "ManagedHdPlaybackStatus.IsReady(snapshot, identity, launch.PackScale, expectedBuildId)" in patched_window
    assert "enhanced-readiness-verified" in patched_window
    assert "_isClosing || _managedCloseRequested" in patched_window
    assert "[revival-observer] frames=" in patched_window
    assert "EmuApi.SetRevivalRenderer(true)" in patched_window
    assert patch_emu_api(patched_api) == patched_api
    assert patch_main_window(patched_window) == patched_window


def test_mesen_hd_loader_status_patch_is_read_only_and_idempotent():
    header = (MESEN / "Core" / "NES" / "NesConsole.h").read_text(encoding="utf-8")
    source = (MESEN / "Core" / "NES" / "NesConsole.cpp").read_text(encoding="utf-8")
    patched_header = patch_nes_console_header(header)
    patched_source = patch_nes_console_cpp(source)
    assert "bool HasManagedHdPack();" in patched_header
    assert "return _hdData && _hdData->Scale > 0;" in patched_source
    assert patch_nes_console_header(patched_header) == patched_header
    assert patch_nes_console_cpp(patched_source) == patched_source


def test_mesen_renderer_status_patch_reports_native_identity_and_frame_evidence():
    factory = (MESEN / "InteropDLL" / "EmuApiWrapper.cpp").read_text(encoding="utf-8")
    patched = patch_factory(factory)
    assert "GetRevivalNesHdPlaybackStatus" in patched
    assert "GraphicsBuildId[65]" in patched
    assert "AdapterVendor[129]" in patched
    assert "AdapterRenderer[257]" in patched
    assert "GetGraphicsIdentity(identity)" in patched
    assert "status->AbiVersion = 2" in patched
    assert patch_factory(patched) == patched


def test_managed_startup_isolated_from_stock_launch_and_rom_switching():
    ui = MESEN / "UI"
    program = patch_program((ui / "Program.cs").read_text(encoding="utf-8"))
    config = patch_config_manager((ui / "Config" / "ConfigManager.cs").read_text(encoding="utf-8"))
    dependencies = patch_dependency_helper((ui / "Utilities" / "DependencyHelper.cs").read_text(encoding="utf-8"))
    rom_helper = patch_load_rom_helper((ui / "Utilities" / "LoadRomHelper.cs").read_text(encoding="utf-8"))
    window = patch_main_window((ui / "Windows" / "MainWindow.axaml.cs").read_text(encoding="utf-8"))

    assert program.index("RevivalLaunch.ReadArguments(args)") < program.index("ConfigManager.HomeFolder")
    assert "StartWithClassicDesktopLifetime(Array.Empty<string>()" in program
    assert "string runtimeRoot = Program.ManagedLaunch != null" in program
    assert "ExtractNativeDependencies(ConfigManager.HomeFolder, includeMesenCore: false)" in program
    assert "DisableSaveSettings = true" in config
    assert "Path.Combine(_managedSession.Saves, \"battery\")" in config
    assert "Path.Combine(_managedSession.Saves, \"states\")" in config
    assert "new ManagedHdPackStager().Stage(_managedSession)" in config
    assert "Config.Nes.EnableHdPacks = stagedHdPack != null" in config
    assert 'J2ME_REVIVAL_ENABLE_NES_ENHANCED' in program
    assert "if(!includeMesenCore && string.Equals(Path.GetFileName(entry.FullName)" in dependencies
    assert "if(Mesen.Program.ManagedLaunch != null) { return; }" in rom_helper
    assert "else if(Mesen.Program.ManagedLaunch != null)" in rom_helper
    assert "EmuApi.LoadRom(launch.RomPath, String.Empty)" in window
    managed = window[window.index("private void InitializeManagedLaunch("):
                     window.index("private void Instance_ArgumentsReceived(")]
    assert 'string.Equals(Environment.GetEnvironmentVariable("J2ME_REVIVAL_MESEN_RENDERER"), "1", StringComparison.Ordinal)' in managed
    assert managed.index("EmuApi.SetRevivalRenderer(useCommonRenderer)") < managed.index("EmuApi.InitializeEmu(")
    assert "EmuApi.SetRevivalRenderer(true)" not in managed
    assert "[revival-managed] ROM loaded" in managed
    assert "launch.WriteStatus(\"rom-loaded\")" in window
    assert "launch.WriteStatus(\"flushing-saves\")" in window
    assert "launch.WriteStatus(\"stopped\")" in window
    assert "try { EmuApi.Stop(); } catch(Exception error)" in window
    assert "[revival-managed] stop failed:" in window
    assert "[revival-managed] release failed:" in window
    assert window.index("[revival-managed] stop failed:") < window.index("EmuApi.Release();")

    for patcher, original in (
        (patch_program, (ui / "Program.cs").read_text(encoding="utf-8")),
        (patch_config_manager, (ui / "Config" / "ConfigManager.cs").read_text(encoding="utf-8")),
        (patch_dependency_helper, (ui / "Utilities" / "DependencyHelper.cs").read_text(encoding="utf-8")),
        (patch_load_rom_helper, (ui / "Utilities" / "LoadRomHelper.cs").read_text(encoding="utf-8")),
        (patch_main_window, (ui / "Windows" / "MainWindow.axaml.cs").read_text(encoding="utf-8")),
    ):
        assert patcher(patcher(original)) == patcher(original)


def test_managed_nes_controls_apply_atomically_and_protect_profile():
    ui = MESEN / "UI"
    window = patch_main_window((ui / "Windows" / "MainWindow.axaml.cs").read_text(encoding="utf-8"))
    view_model = patch_nes_config_view_model((ui / "ViewModels" / "NesConfigViewModel.cs").read_text(encoding="utf-8"))
    view = patch_nes_config_view((ui / "Views" / "NesConfigView.axaml").read_text(encoding="utf-8"))

    launch_method = window.index("private void InitializeManagedLaunch(RevivalLaunchManifest launch)")
    initialize = window.index("EmuApi.InitializeEmu(", launch_method)
    apply_config = window.index("ApplyManagedControlMappings(launch);", initialize)
    native_apply = window.index("ConfigManager.Config.ApplyConfig();", apply_config)
    rom_load = window.index("EmuApi.LoadRom(launch.RomPath, String.Empty)")
    mapping_method = window.index("private void ApplyManagedControlMappings(RevivalLaunchManifest launch)")
    resolve = window.index("RevivalNesControls.Resolve(launch.Controls, InputApi.GetKeyCode)", mapping_method)
    assert initialize < apply_config < native_apply < rom_load
    assert mapping_method < resolve
    for field in ("Up", "Down", "Left", "Right", "A", "B", "Start", "Select"):
        assert f"{field} = keys.{field}" in window
    keyboard_start = window.index("var keyboard = new NesKeyMapping()")
    keyboard_end = window.index("var xbox = new NesKeyMapping()", keyboard_start)
    keyboard_initializer = window[keyboard_start:keyboard_end]
    assert "Turbo" not in keyboard_initializer and "GenericKey1" not in keyboard_initializer
    assert "new NesKeyMapping()" in window
    assert "xbox.SetDefaultKeys(ControllerType.NesController, KeyPresetType.XboxP1)" in window
    assert "DefaultKeyMappingType.Ps4" in window
    assert "ps4.SetDefaultKeys(ControllerType.NesController, KeyPresetType.Ps4P1)" in window
    assert "port1.Mapping1 = keyboard" in window
    assert "port1.Mapping2 = xbox" in window
    assert "port1.Mapping3 = ps4" in window
    assert "port1.Mapping4 = unused" in window
    assert "port1.Type =" not in window
    assert "InputApi.ResetKeyState();" in window
    assert "CanEditControllerMappings => global::Mesen.Program.ManagedLaunch == null;" in view_model
    assert 'IsVisible="{Binding CanEditControllerMappings}"' in view
    nes_config = (ui / "Config" / "NesConfig.cs").read_text(encoding="utf-8")
    refresh_start = nes_config.index("public void UpdateInputFromCoreConfig()")
    refresh_body = nes_config[refresh_start:nes_config.index("\n\t\t}", refresh_start) + 5]
    assert "Port1.Type = cfg.Port1.Type;" in refresh_body
    assert "Port1.Mapping" not in refresh_body and "InitializeDefaults" not in refresh_body

    assert patch_main_window(window) == window
    assert patch_nes_config_view_model(view_model) == view_model
    assert patch_nes_config_view(view) == view


def test_pinned_generation_includes_managed_startup_sources(tmp_path):
    generate(MESEN, tmp_path)
    for relative in (
        "UI/Program.cs",
        "UI/Config/ConfigManager.cs",
        "UI/Utilities/DependencyHelper.cs",
        "UI/Utilities/LoadRomHelper.cs",
        "UI/Revival/RevivalLaunch.cs",
        "UI/Revival/ManagedLaunchStartup.cs",
        "UI/Revival/RevivalNesControls.cs",
        "UI/Revival/ManagedHdPackValidator.cs",
        "UI/Revival/ManagedHdPackStager.cs",
        "UI/Revival/ManagedHdPlaybackStatus.cs",
        "Core/NES/NesConsole.h",
        "Core/NES/NesConsole.cpp",
        "UI/ViewModels/NesConfigViewModel.cs",
        "UI/Views/NesConfigView.axaml",
    ):
        assert (tmp_path / relative).is_file()
