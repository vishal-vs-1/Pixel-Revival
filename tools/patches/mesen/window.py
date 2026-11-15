"""Bind managed-session window, focus, and shutdown behavior."""

from .anchors import _require


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
