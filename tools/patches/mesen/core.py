"""Integrate the NES bridge with the native core and interop API."""

from .anchors import _require


def patch_factory(source: str, *, inprocess: bool = False) -> str:
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
    if inprocess and "SetRevivalSaveFolder(" not in source:
        anchor = '\tDllExport void __stdcall SetRevivalRenderer(bool enabled)'
        _require(source, anchor)
        exports = """\tDllExport bool __stdcall SetRevivalSaveFolder(const char* saveFolder)
\t{
#ifdef REVIVAL_MESEN_BRIDGE
\t\tif(!_emu || saveFolder == nullptr || saveFolder[0] == '\\0') return false;
\t\tFolderUtilities::SetFolderOverrides(saveFolder, "", "", "");
\t\treturn true;
#else
\t\t(void)saveFolder;
\t\treturn false;
#endif
\t}

\tDllExport bool __stdcall SetRevivalNesControls(const uint16_t* keys, uint32_t count)
\t{
#ifdef REVIVAL_MESEN_BRIDGE
\t\tif(!_emu || keys == nullptr || count != 8) return false;
\t\tfor(uint32_t index = 0; index < count; ++index) {
\t\t\tif(keys[index] == 0) return false;
\t\t\tfor(uint32_t previous = 0; previous < index; ++previous) {
\t\t\t\tif(keys[previous] == keys[index]) return false;
\t\t\t}
\t\t}
\t\tNesConfig config = _emu->GetSettings()->GetNesConfig();
\t\tconfig.Port1.Type = ControllerType::NesController;
\t\tconfig.AutoConfigureInput = false;
\t\tauto& mapping = config.Port1.Keys.Mapping1;
\t\tmapping.Up = keys[0];
\t\tmapping.Down = keys[1];
\t\tmapping.Left = keys[2];
\t\tmapping.Right = keys[3];
\t\tmapping.A = keys[4];
\t\tmapping.B = keys[5];
\t\tmapping.Start = keys[6];
\t\tmapping.Select = keys[7];
\t\t_emu->GetSettings()->SetNesConfig(config);
\t\treturn true;
#else
\t\t(void)keys;
\t\t(void)count;
\t\treturn false;
#endif
\t}

\tDllExport void __stdcall StopRevivalNesCapture()
\t{
#ifdef REVIVAL_MESEN_BRIDGE
\t\tif(_revivalCaptureStarted) {
\t\t\tRevivalNesCapture().stop();
\t\t\t_revivalCaptureStarted = false;
\t\t}
#endif
\t}

"""
        forward = "#ifdef REVIVAL_MESEN_BRIDGE\nstatic revival::nes::NesCaptureSession& RevivalNesCapture();\n#endif\n\n"
        source = source.replace(anchor, forward + exports + anchor, 1)
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
