using System.Text.Json;
using System.Text;
using System.Text.Json.Nodes;
using System.Security.Cryptography;
using Revival.Mesen;

if (args.Length == 7 && args[0] == "--pack-self-test")
{
    try
    {
        CheckManagedHdPackStaging(args[1..]);
        CheckManagedHdPlaybackStatus();
        Console.WriteLine("Managed NES HD pack staging checks passed.");
        return 0;
    }
    catch (Exception error)
    {
        Console.Error.WriteLine(error.Message);
        return 1;
    }
}

if (args.Length != 2 || args[0] != "--self-test")
{
    Console.Error.WriteLine("Pass --self-test or --pack-self-test with Python-created fixtures.");
    return 2;
}

string fixturePath = Path.GetFullPath(args[1]);
byte[] original = File.ReadAllBytes(fixturePath);
string launchArgument = "--revival-launch=" + fixturePath;
try
{
    RevivalLaunchManifest manifest = RevivalLaunch.ReadArguments([launchArgument])
        ?? throw new InvalidDataException("Managed reader returned stock mode for a managed fixture.");
    Check(manifest.Controls.Count == 8, "Expected the complete NES controls map.");
    Check(manifest.RomPath.Contains("日本", StringComparison.Ordinal), "Unicode path was not preserved.");
    CheckGraphicsRuntime(manifest);
    CheckManagedNesControls();
    Check(RevivalLaunch.ReadArguments([]) is null, "Stock invocation should remain unmodified.");
    Rejects(() => RevivalLaunch.ReadArguments([launchArgument, launchArgument]));
    Rejects(() => RevivalLaunch.ReadArguments(["--revival-launch="]));
    Rejects(() => RevivalLaunch.ReadArguments(["--REVIVAL-LAUNCH=" + fixturePath]));
    Rejects(() => RevivalLaunch.ReadArguments([launchArgument, "--unexpected"]));

    var fields = JsonNode.Parse(original)!.AsObject();
    fields["unrecognized"] = true;
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["schema"] = 2;
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["controls"]!["A"] = "NOT_A_KEY";
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["controls"]!["B"] = "KEY_X";
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    string duplicateAction = Encoding.UTF8.GetString(original)
        .Replace("\"controls\":{\"A\":\"KEY_X\"",
            "\"controls\":{\"A\":\"KEY_X\",\"A\":\"KEY_X\"", StringComparison.Ordinal);
    RejectsFixture(fixturePath, duplicateAction, launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["session_id"] = "ffffffffffffffffffffffffffffffff";
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["game_id"] = new string('f', 64);
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["home"] = manifest.Home + "-redirected";
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["rom_sha256"] = new string('0', 64);
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    if (manifest.Mode == "original")
    {
        fields["mode"] = "enhanced";
    }
    else
    {
        fields["pack_manifest"] = null;
    }
    RejectsFixture(fixturePath, fields.ToJsonString(), launchArgument);

    fields = JsonNode.Parse(original)!.AsObject();
    fields["controls"] = new JsonObject();
    WriteFixture(fixturePath, fields.ToJsonString());
    RevivalLaunchManifest legacy = RevivalLaunch.ReadArguments([launchArgument])!;
    Check(legacy.Controls["A"] == "KEY_X" && legacy.Controls["SELECT"] == "SPACE",
        "Legacy empty controls must resolve to Default.");

    WriteFixtureBytes(fixturePath, Enumerable.Repeat((byte)' ', RevivalLaunch.MaximumManifestBytes + 1).ToArray());
    Rejects(() => RevivalLaunch.ReadArguments([launchArgument]));

    WriteFixture(fixturePath, Encoding.UTF8.GetString(original)
        .Replace("\"schema\":1", "\"schema\":1,\"schema\":1", StringComparison.Ordinal));
    Rejects(() => RevivalLaunch.ReadArguments([launchArgument]));

    WriteFixtureBytes(fixturePath, original);
    manifest.WriteStatus("starting");
    var status = JsonNode.Parse(File.ReadAllBytes(manifest.Status))!.AsObject();
    Check(status["phase"]!.GetValue<string>() == "starting", "Status phase was not written.");
    Check(status["session_id"]!.GetValue<string>() == manifest.SessionId,
        "Status session identity was not preserved.");
    Check(status["schema"]!.GetValue<int>() == 1 && !status.ContainsKey("reason"),
        "Starting status must preserve its schema and omit an empty reason.");
    manifest.WriteStatus("enhanced-hd-frame-detected", "pack=loaded frame=1024x960");
    status = JsonNode.Parse(File.ReadAllBytes(manifest.Status))!.AsObject();
    Check(status["phase"]!.GetValue<string>() == "enhanced-hd-frame-detected",
        "HD frame diagnostic phase was not persisted.");
    manifest.WriteStatus("enhanced-readiness-verified", "pack=loaded frame=1024x960 build=match");
    status = JsonNode.Parse(File.ReadAllBytes(manifest.Status))!.AsObject();
    Check(status["phase"]!.GetValue<string>() == "enhanced-readiness-verified",
        "Enhanced readiness handshake phase was not persisted.");
    manifest.WriteStatus("enhanced-frame-check-unverified", "pack=missing");
    status = JsonNode.Parse(File.ReadAllBytes(manifest.Status))!.AsObject();
    Check(status["phase"]!.GetValue<string>() == "enhanced-frame-check-unverified",
        "Unverified HD frame diagnostic phase was not persisted.");
    manifest.WriteStatus("error", "é\n" + new string('x', 256));
    status = JsonNode.Parse(File.ReadAllBytes(manifest.Status))!.AsObject();
    Check(status["phase"]!.GetValue<string>() == "error" &&
          status["reason"]!.GetValue<string>() == "é" + new string('x', 255),
        "Error status must preserve text and bound the cleaned reason.");

    Console.WriteLine("Managed NES launch reader checks passed.");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(error.Message);
    return 1;
}
finally
{
    WriteFixtureBytes(fixturePath, original);
}

static void Check(bool condition, string message)
{
    if (!condition)
    {
        throw new InvalidOperationException(message);
    }
}

static void CheckManagedHdPackStaging(string[] fixtures)
{
    RevivalLaunchManifest original = RevivalLaunch.ReadFile(fixtures[0]);
    string? originalResult = new ManagedHdPackStager().Stage(original);
    Check(originalResult is null, "Original mode must not stage an HD pack.");
    Check(!Directory.Exists(Path.Combine(original.Home, "HdPacks")),
        "Original mode created an HD pack folder.");

    RevivalLaunchManifest valid = RevivalLaunch.ReadFile(fixtures[1]);
    string staged = new ManagedHdPackStager().Stage(valid)
        ?? throw new InvalidOperationException("Enhanced mode did not publish its staged pack.");
    Check(staged == Path.Combine(valid.Home, "HdPacks", "game"),
        "HD pack destination did not use the ROM basename under the private home.");
    Check(File.Exists(Path.Combine(staged, "hires.txt")), "Validated definition was not staged.");
    Check(File.Exists(Path.Combine(staged, "page-000.png")), "Validated image page was not staged.");
    Check(!File.Exists(Path.Combine(staged, "manifest.json")),
        "The internal pack manifest must not be copied to Mesen's pack folder.");
    Check(Directory.GetFiles(staged).Length == 2, "Staging copied files outside the allowed pack contents.");
    byte[] stagedPage = File.ReadAllBytes(Path.Combine(staged, "page-000.png"));
    string stagedHash = Convert.ToHexString(SHA256.HashData(stagedPage));
    string sourceHash = Convert.ToHexString(SHA256.HashData(
        File.ReadAllBytes(Path.Combine(Path.GetDirectoryName(valid.PackManifest!)!, "page-000.png"))));
    Check(stagedHash == sourceHash, "Staged page bytes differ from the validated source page.");
    Rejects(() => new ManagedHdPackStager().Stage(valid));
    Check(File.ReadAllBytes(Path.Combine(staged, "page-000.png")).SequenceEqual(stagedPage),
        "An existing private pack was changed by a repeated stage attempt.");

    RejectPackFixture(fixtures[2], "invalid page reference");
    RejectPackFixture(fixtures[3], "mismatched scale");
    RejectPackFixture(fixtures[4], "missing page");

    RevivalLaunchManifest copyFailure = RevivalLaunch.ReadFile(fixtures[5]);
    int copies = 0;
    var brokenCopier = new ManagedHdPackStager((source, target, length) =>
    {
        copies++;
        if (copies == 2)
        {
            throw new IOException("Injected fixture copy failure.");
        }
        File.Copy(source, target);
    });
    Rejects(() => brokenCopier.Stage(copyFailure));
    Check(copies == 2, "The copy failure fixture did not reach the second file.");
    string hdRoot = Path.Combine(copyFailure.Home, "HdPacks");
    Check(!Directory.Exists(Path.Combine(hdRoot, "game")),
        "Failed copying published a partial HD pack.");
    Check(!Directory.Exists(hdRoot) || !Directory.EnumerateFileSystemEntries(hdRoot).Any(),
        "Failed copying left temporary staging files behind.");
}

static void CheckManagedHdPlaybackStatus()
{
    Check(System.Runtime.InteropServices.Marshal.SizeOf<ManagedHdNativeStatus>() == 640,
        "Managed HD readiness status layout does not match the native ABI.");
    var valid = new ManagedHdFrameStatus(1, 4, 1, 10, 8, 900, 720);
    string expectedBuildId = new string('a', 64);
    var validIdentity = new ManagedGraphicsIdentity(true, 1, expectedBuildId,
        "NVIDIA Corporation", "GeForce RTX 2050", "OpenGL 4.6");
    Check(ManagedHdPlaybackStatus.IsReady(valid, validIdentity, 4, expectedBuildId),
        "Cropped HD presentation with matching renderer identity was not accepted.");
    Check(ManagedHdPlaybackStatus.IsHdFrameReady(valid, 4),
        "HD frame evidence must remain independent from the renderer identity check.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid with { PackLoaded = 0 }, validIdentity, 4, expectedBuildId),
        "A missing native HD pack was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid with { PackScale = 2 }, validIdentity, 4, expectedBuildId),
        "A mismatched native HD scale was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid with { CommonRendererActive = 0 }, validIdentity, 4, expectedBuildId),
        "An inactive common renderer was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid with { PresentedFrames = 0 }, validIdentity, 4, expectedBuildId),
        "A frame that was not presented was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid with { Width = 256, Height = 240 }, validIdentity, 4, expectedBuildId),
        "Native-size frames were accepted as HD output.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid, validIdentity with { BuildId = new string('b', 64) },
        4, expectedBuildId), "A different common-renderer build was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid, validIdentity with { Available = false },
        4, expectedBuildId), "A missing native adapter identity was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid, validIdentity with { Renderer = "" },
        4, expectedBuildId), "An unidentified graphics adapter was accepted.");
    Check(!ManagedHdPlaybackStatus.IsReady(valid, validIdentity, 3, expectedBuildId),
        "An unsupported scale was accepted.");
}

static void RejectPackFixture(string path, string description)
{
    RevivalLaunchManifest launch = RevivalLaunch.ReadFile(path);
    Rejects(() => new ManagedHdPackStager().Stage(launch));
    string hdRoot = Path.Combine(launch.Home, "HdPacks");
    Check(!Directory.Exists(Path.Combine(hdRoot, "game")),
        $"Rejected {description} fixture published an HD pack.");
    Check(!Directory.Exists(hdRoot) || !Directory.EnumerateFileSystemEntries(hdRoot).Any(),
        $"Rejected {description} fixture left staging files behind.");
}

static void Rejects(Action action)
{
    try
    {
        action();
    }
    catch (Exception error) when (error is InvalidDataException or JsonException or IOException)
    {
        return;
    }
    throw new InvalidOperationException("Expected malformed managed launch data to be rejected.");
}

static void RejectsFixture(string path, string json, string launchArgument)
{
    WriteFixture(path, json);
    Rejects(() => RevivalLaunch.ReadArguments([launchArgument]));
}

static void WriteFixture(string path, string json) =>
    File.WriteAllText(path, json, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));

static void WriteFixtureBytes(string path, byte[] bytes) => File.WriteAllBytes(path, bytes);

static void CheckGraphicsRuntime(RevivalLaunchManifest manifest)
{
    string runtimeCore = Path.Combine(AppContext.BaseDirectory, "MesenCore.dll");
    string runtimeSkia = Path.Combine(AppContext.BaseDirectory, "libSkiaSharp.dll");
    string runtimeHarfBuzz = Path.Combine(AppContext.BaseDirectory, "libHarfBuzzSharp.dll");
    string[] stagedLibraries = [runtimeCore, runtimeSkia, runtimeHarfBuzz];
    string graphicsManifest = manifest.GraphicsManifest;
    string sourceGraphics = Path.Combine(Path.GetDirectoryName(graphicsManifest)!, "revival_graphics.dll");
    string commonFolder = Path.Combine(Directory.GetParent(Path.GetDirectoryName(graphicsManifest)!)!.FullName, "common");
    string abiBuildIdPath = Path.Combine(commonFolder, "abi-build-id.txt");
    if (stagedLibraries.Any(File.Exists))
    {
        throw new InvalidOperationException("Managed runtime fixture output already contains native binaries.");
    }

    bool hadManifest = File.Exists(graphicsManifest);
    byte[] oldManifest = hadManifest ? File.ReadAllBytes(graphicsManifest) : [];
    byte[] oldGraphics = File.ReadAllBytes(sourceGraphics);
    bool hadAbiBuildId = File.Exists(abiBuildIdPath);
    byte[] oldAbiBuildId = hadAbiBuildId ? File.ReadAllBytes(abiBuildIdPath) : [];
    try
    {
        string buildId = Convert.ToHexString(SHA256.HashData(oldGraphics)).ToLowerInvariant();
        Directory.CreateDirectory(commonFolder);
        string expectedAbiBuildId = new string('c', 64);
        File.WriteAllText(abiBuildIdPath, expectedAbiBuildId + "\n", Encoding.ASCII);
        File.WriteAllBytes(runtimeCore, "synthetic core"u8.ToArray());
        File.WriteAllBytes(runtimeSkia, "synthetic skia"u8.ToArray());
        File.WriteAllBytes(runtimeHarfBuzz, "synthetic harfbuzz"u8.ToArray());
        File.WriteAllText(graphicsManifest, $$"""
            {"schema":1,"protocol":"common-v1","abi_version":1,"build_id":"{{buildId}}","dll":"revival_graphics.dll"}
            """);

        ManagedLaunchStartup.VerifyRuntime(manifest);
        Check(ManagedLaunchStartup.ReadExpectedAbiBuildId(graphicsManifest) == expectedAbiBuildId,
            "Expected common renderer ABI build identity was not read.");
        File.WriteAllText(abiBuildIdPath, new string('C', 64) + "\n", Encoding.ASCII);
        Rejects(() => ManagedLaunchStartup.ReadExpectedAbiBuildId(graphicsManifest));
        File.WriteAllText(abiBuildIdPath, expectedAbiBuildId + "\n", Encoding.ASCII);
        File.WriteAllBytes(sourceGraphics, "stale graphics"u8.ToArray());
        Rejects(() => ManagedLaunchStartup.VerifyRuntime(manifest));
    }
    finally
    {
        foreach (string library in stagedLibraries)
        {
            File.Delete(library);
        }
        File.WriteAllBytes(sourceGraphics, oldGraphics);
        if (hadManifest)
        {
            File.WriteAllBytes(graphicsManifest, oldManifest);
        }
        else
        {
            File.Delete(graphicsManifest);
        }
        if (hadAbiBuildId)
        {
            File.WriteAllBytes(abiBuildIdPath, oldAbiBuildId);
        }
        else
        {
            File.Delete(abiBuildIdPath);
        }
    }
}

static void CheckManagedNesControls()
{
    var tokens = new Dictionary<string, string>(StringComparer.Ordinal);
    foreach (char letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ")
    {
        tokens.Add("KEY_" + letter, letter.ToString());
    }
    foreach (char digit in "0123456789")
    {
        tokens.Add("DIGIT_" + digit, digit.ToString());
        tokens.Add("NUMPAD_" + digit, "Numpad " + digit);
    }
    tokens.Add("ARROW_UP", "Up Arrow");
    tokens.Add("ARROW_DOWN", "Down Arrow");
    tokens.Add("ARROW_LEFT", "Left Arrow");
    tokens.Add("ARROW_RIGHT", "Right Arrow");
    tokens.Add("NUMPAD_ENTER", "Numpad Enter");
    tokens.Add("ENTER", "Enter");
    tokens.Add("SPACE", "Space");
    foreach ((string token, string name) in tokens)
    {
        Check(RevivalNesControls.ToNativeKeyName(token) == name,
            $"Physical token {token} did not map to Mesen's key name.");
    }
    RejectsMapping(() => RevivalNesControls.ToNativeKeyName("KEY_ESC"));

    var bindings = new Dictionary<string, string>(StringComparer.Ordinal)
    {
        ["UP"] = "ARROW_UP", ["DOWN"] = "ARROW_DOWN",
        ["LEFT"] = "ARROW_LEFT", ["RIGHT"] = "ARROW_RIGHT",
        ["A"] = "KEY_X", ["B"] = "KEY_Z",
        ["START"] = "ENTER", ["SELECT"] = "SPACE"
    };
    var nativeCodes = new Dictionary<string, ushort>(StringComparer.Ordinal);
    ushort nextCode = 1;
    foreach (string name in tokens.Values.Distinct(StringComparer.Ordinal))
    {
        nativeCodes.Add(name, nextCode++);
    }
    nativeCodes["Esc"] = 60000;
    nativeCodes["F11"] = 60001;
    ushort Resolve(string name) => nativeCodes.TryGetValue(name, out ushort code) ? code : (ushort)0;

    NesControlCodes resolved = RevivalNesControls.Resolve(bindings, Resolve);
    Check(resolved.Up == Resolve("Up Arrow") && resolved.Down == Resolve("Down Arrow") &&
        resolved.Left == Resolve("Left Arrow") && resolved.Right == Resolve("Right Arrow") &&
        resolved.A == Resolve("X") && resolved.B == Resolve("Z") &&
        resolved.Start == Resolve("Enter") && resolved.Select == Resolve("Space"),
        "Resolved action codes did not preserve the launch snapshot.");

    RejectsMapping(() => RevivalNesControls.Resolve(bindings, _ => 0));
    RejectsMapping(() => RevivalNesControls.Resolve(bindings, name =>
        name is "Esc" ? (ushort)60000 : name is "F11" ? (ushort)60001 : (ushort)7));
    RejectsMapping(() => RevivalNesControls.Resolve(bindings, name =>
        name is "Esc" ? (ushort)60000 : name is "F11" ? (ushort)60001 : name == "X" ? (ushort)60000 : Resolve(name)));
    RejectsMapping(() => RevivalNesControls.Resolve(bindings, name =>
        name is "Esc" ? (ushort)60000 : name is "F11" ? (ushort)60001 : name == "X" ? (ushort)60001 : Resolve(name)));
    var incomplete = new Dictionary<string, string>(bindings, StringComparer.Ordinal);
    incomplete.Remove("SELECT");
    RejectsMapping(() => RevivalNesControls.Resolve(incomplete, Resolve));
}

static void RejectsMapping(Action action)
{
    try
    {
        action();
    }
    catch (InvalidDataException)
    {
        return;
    }
    throw new InvalidOperationException("Expected invalid NES key mapping to be rejected.");
}
