"""Read managed launch options before loading the game."""

from .anchors import _require


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
