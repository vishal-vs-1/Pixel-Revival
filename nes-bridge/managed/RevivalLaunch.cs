using System;
using System.Collections.ObjectModel;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace Revival.Mesen;

public sealed class RevivalLaunchManifest
{
    internal RevivalLaunchManifest(
        string manifestPath,
        string gameId,
        string sessionId,
        string romPath,
        string romSha256,
        string home,
        string saves,
        string events,
        string status,
        string mode,
        string? packManifest,
        int packScale,
        Dictionary<string, string> controls,
        string graphicsManifest)
    {
        ManifestPath = manifestPath;
        GameId = gameId;
        SessionId = sessionId;
        RomPath = romPath;
        RomSha256 = romSha256;
        Home = home;
        Saves = saves;
        Events = events;
        Status = status;
        Mode = mode;
        PackManifest = packManifest;
        PackScale = packScale;
        Controls = new ReadOnlyDictionary<string, string>(
            new Dictionary<string, string>(controls, StringComparer.Ordinal));
        GraphicsManifest = graphicsManifest;
    }

    public string ManifestPath { get; }
    public string GameId { get; }
    public string SessionId { get; }
    public string RomPath { get; }
    public string RomSha256 { get; }
    public string Home { get; }
    public string Saves { get; }
    public string Events { get; }
    public string Status { get; }
    public string Mode { get; }
    public string? PackManifest { get; }
    public int PackScale { get; }
    public IReadOnlyDictionary<string, string> Controls { get; }
    public string GraphicsManifest { get; }

    public void WriteStatus(string phase, string? reason = null)
    {
        string[] phases = ["starting", "rom-loaded", "enhanced-hd-frame-detected", "enhanced-readiness-verified", "enhanced-frame-check-unverified", "error", "flushing-saves", "stopped"];
        if (!phases.Contains(phase, StringComparer.Ordinal))
        {
            throw new ArgumentException("Unsupported managed launch status phase.", nameof(phase));
        }

        // The published player disables reflection-based JSON serialization.
        using var statusBuffer = new MemoryStream();
        using (var writer = new Utf8JsonWriter(statusBuffer))
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WriteString("session_id", SessionId);
            writer.WriteString("phase", phase);
            if (reason is not null)
            {
                var cleanReason = new string(reason.Where(character => !char.IsControl(character))
                    .Take(256).ToArray());
                if (cleanReason.Length > 0)
                {
                    writer.WriteString("reason", cleanReason);
                }
            }
            writer.WriteEndObject();
        }

        if (statusBuffer.Length > 4096)
        {
            throw new InvalidDataException("Managed status exceeds 4 KiB.");
        }

        RevivalLaunch.EnsureNoLinkedSegments(Status);
        if (!Directory.Exists(Path.GetDirectoryName(Status)))
        {
            throw new InvalidDataException("Managed session folder is missing before status write.");
        }
        string temporary = Status + ".tmp-" + Guid.NewGuid().ToString("N");
        try
        {
            using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write,
                       FileShare.None, 4096, FileOptions.WriteThrough))
            {
                statusBuffer.WriteTo(stream);
                stream.Flush(flushToDisk: true);
            }
            File.Move(temporary, Status, overwrite: true);
        }
        finally
        {
            if (File.Exists(temporary))
            {
                File.Delete(temporary);
            }
        }
    }
}

public static class RevivalLaunch
{
    public const int MaximumManifestBytes = 65536;
    private const string ArgumentPrefix = "--revival-launch=";
    private const string ArgumentName = "--revival-launch";

    private static readonly string[] RequiredFields =
    [
        "schema", "game_id", "session_id", "rom_path", "rom_sha256", "home", "saves",
        "events", "status", "mode", "pack_manifest", "pack_scale", "controls",
        "graphics_manifest"
    ];

    private static readonly string[] Actions =
    ["UP", "DOWN", "LEFT", "RIGHT", "A", "B", "START", "SELECT"];

    private static readonly HashSet<string> SupportedKeys = BuildSupportedKeys();

    public static RevivalLaunchManifest? ReadArguments(IReadOnlyList<string> arguments)
    {
        ArgumentNullException.ThrowIfNull(arguments);
        var managed = arguments.Where(argument =>
            argument.StartsWith(ArgumentName, StringComparison.OrdinalIgnoreCase)).ToArray();
        if (managed.Length == 0)
        {
            return null;
        }

        if (managed.Length != 1 || arguments.Count != 1 ||
            !managed[0].StartsWith(ArgumentPrefix, StringComparison.Ordinal) ||
            managed[0].Length == ArgumentPrefix.Length)
        {
            throw new InvalidDataException("Managed NES launch requires exactly one valid manifest argument.");
        }

        if (!OperatingSystem.IsWindows())
        {
            throw new PlatformNotSupportedException("Managed Mesen launch is supported on Windows only.");
        }

        return ReadFile(managed[0][ArgumentPrefix.Length..]);
    }

    public static RevivalLaunchManifest ReadFile(string manifestPath)
    {
        if (!OperatingSystem.IsWindows())
        {
            throw new PlatformNotSupportedException("Managed Mesen launch is supported on Windows only.");
        }
        if (string.IsNullOrWhiteSpace(manifestPath) || !Path.IsPathFullyQualified(manifestPath))
        {
            throw new InvalidDataException("Managed launch manifest path must be absolute.");
        }

        string fullManifestPath = FullPath(manifestPath, "manifest path");
        EnsureNoLinkedSegments(fullManifestPath);
        byte[] json = ReadBounded(fullManifestPath);
        using JsonDocument document = JsonDocument.Parse(json, new JsonDocumentOptions { MaxDepth = 8 });
        JsonElement root = document.RootElement;
        var fields = ReadExactObject(root, RequiredFields, "launch manifest");

        if (ReadInt32(fields["schema"], "schema") != 1)
        {
            throw new InvalidDataException("Unsupported managed launch schema.");
        }

        string gameId = ReadString(fields["game_id"], "game_id", 64);
        string sessionId = ReadString(fields["session_id"], "session_id", 32);
        if (!IsHex(gameId, 64) || !IsHex(sessionId, 32))
        {
            throw new InvalidDataException("Managed launch identity is invalid.");
        }

        string romPath = ReadAbsolutePath(fields["rom_path"], "rom_path");
        string romSha256 = ReadString(fields["rom_sha256"], "rom_sha256", 64);
        if (!IsHex(romSha256, 64))
        {
            throw new InvalidDataException("Managed ROM hash is invalid.");
        }

        string home = ReadAbsolutePath(fields["home"], "home");
        string saves = ReadAbsolutePath(fields["saves"], "saves");
        string events = ReadAbsolutePath(fields["events"], "events");
        string status = ReadAbsolutePath(fields["status"], "status");
        string mode = ReadString(fields["mode"], "mode", 16);
        string? packManifest = fields["pack_manifest"].ValueKind == JsonValueKind.Null
            ? null : ReadAbsolutePath(fields["pack_manifest"], "pack_manifest");
        int packScale = ReadInt32(fields["pack_scale"], "pack_scale");
        if (packScale is not (2 or 4))
        {
            throw new InvalidDataException("Managed pack scale must be 2 or 4.");
        }
        string graphicsManifest = ReadAbsolutePath(fields["graphics_manifest"], "graphics_manifest");
        Dictionary<string, string> controls = ReadControls(fields["controls"]);

        string normalizedRom = FullPath(romPath, "rom_path");
        string normalizedHome = FullPath(home, "home");
        string normalizedSaves = FullPath(saves, "saves");
        string normalizedEvents = FullPath(events, "events");
        string normalizedStatus = FullPath(status, "status");
        string normalizedGraphics = FullPath(graphicsManifest, "graphics_manifest");
        string? normalizedPack = packManifest is null ? null : FullPath(packManifest, "pack_manifest");

        string gameFolder = Path.GetDirectoryName(normalizedRom)
            ?? throw new InvalidDataException("Managed ROM path has no parent.");
        string libraryFolder = Path.GetDirectoryName(gameFolder)
            ?? throw new InvalidDataException("Managed ROM path has no library parent.");
        string nesRoot = Path.GetDirectoryName(libraryFolder)
            ?? throw new InvalidDataException("Managed ROM path has no NES root.");
        string expectedRom = Path.Combine(nesRoot, "library", gameId, "game.nes");
        if (!SamePath(normalizedRom, expectedRom) ||
            !SamePath(gameFolder, Path.Combine(nesRoot, "library", gameId)))
        {
            throw new InvalidDataException("Managed ROM is outside its namespaced library location.");
        }

        string workspace = Path.Combine(nesRoot, "rendered-games", gameId);
        string sessionRoot = Path.Combine(workspace, ".runtime", "sessions", sessionId);
        string expectedHome = Path.Combine(sessionRoot, "home");
        string expectedEvents = Path.Combine(sessionRoot, "events");
        string expectedStatus = Path.Combine(sessionRoot, "status.json");
        string expectedSaves = Path.Combine(workspace, "saves", "mesen");
        string expectedManifest = Path.Combine(sessionRoot, "launch.json");
        if (!SamePath(fullManifestPath, expectedManifest) || !SamePath(normalizedHome, expectedHome) ||
            !SamePath(normalizedEvents, expectedEvents) || !SamePath(normalizedStatus, expectedStatus) ||
            !SamePath(normalizedSaves, expectedSaves))
        {
            throw new InvalidDataException("Managed session paths do not match their owned workspace.");
        }

        if (mode == "original")
        {
            if (normalizedPack is not null)
            {
                throw new InvalidDataException("Original mode cannot name an HD pack.");
            }
        }
        else if (mode == "enhanced")
        {
            if (normalizedPack is null)
            {
                throw new InvalidDataException("Enhanced mode requires an HD pack manifest.");
            }
            string packFolder = Path.GetDirectoryName(normalizedPack)
                ?? throw new InvalidDataException("HD pack path has no generation folder.");
            string generation = Path.GetFileName(packFolder);
            if (!IsHex(generation, 32) ||
                !SamePath(normalizedPack, Path.Combine(workspace, "nes", "packs", generation,
                    "manifest.json")))
            {
                throw new InvalidDataException("Enhanced HD pack is outside its immutable generation.");
            }
        }
        else
        {
            throw new InvalidDataException("Managed launch mode must be original or enhanced.");
        }

        foreach (string ownedPath in new[]
                 {
                     fullManifestPath, normalizedRom, workspace, sessionRoot, normalizedHome,
                     normalizedEvents, normalizedStatus, normalizedSaves
                 })
        {
            EnsureNoLinkedSegments(ownedPath);
        }
        RequireFile(fullManifestPath, "launch manifest");
        RequireFile(normalizedRom, "NES ROM");
        RequireDirectory(workspace, "game workspace");
        RequireDirectory(sessionRoot, "session folder");
        RequireDirectory(normalizedHome, "private Mesen home");
        RequireDirectory(normalizedEvents, "session events folder");
        RequireDirectory(normalizedSaves, "persistent Mesen saves folder");
        if (normalizedPack is not null)
        {
            EnsureNoLinkedSegments(normalizedPack);
            RequireFile(normalizedPack, "HD pack manifest");
        }

        VerifyRomIdentity(normalizedRom, romSha256, gameId);
        return new RevivalLaunchManifest(
            fullManifestPath, gameId, sessionId, normalizedRom, romSha256,
            normalizedHome, normalizedSaves, normalizedEvents, normalizedStatus,
            mode, normalizedPack, packScale, controls, normalizedGraphics);
    }

    private static byte[] ReadBounded(string path)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read,
            4096, FileOptions.SequentialScan);
        byte[] buffer = new byte[MaximumManifestBytes + 1];
        int total = 0;
        while (total < buffer.Length)
        {
            int read = stream.Read(buffer, total, buffer.Length - total);
            if (read == 0)
            {
                break;
            }
            total += read;
        }
        if (total > MaximumManifestBytes)
        {
            throw new InvalidDataException("Managed launch manifest exceeds 64 KiB.");
        }
        Array.Resize(ref buffer, total);
        return buffer;
    }

    private static Dictionary<string, JsonElement> ReadExactObject(
        JsonElement element, IEnumerable<string> expectedFields, string description)
    {
        if (element.ValueKind != JsonValueKind.Object)
        {
            throw new InvalidDataException($"Managed {description} must be an object.");
        }
        var expected = new HashSet<string>(expectedFields, StringComparer.Ordinal);
        var fields = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        foreach (JsonProperty property in element.EnumerateObject())
        {
            if (!expected.Contains(property.Name))
            {
                throw new InvalidDataException($"Unknown managed {description} field: {property.Name}.");
            }
            if (!fields.TryAdd(property.Name, property.Value))
            {
                throw new InvalidDataException($"Duplicate managed {description} field: {property.Name}.");
            }
        }
        if (fields.Count != expected.Count)
        {
            throw new InvalidDataException($"Managed {description} is missing a required field.");
        }
        return fields;
    }

    private static Dictionary<string, string> ReadControls(JsonElement element)
    {
        if (element.ValueKind == JsonValueKind.Object && !element.EnumerateObject().Any())
        {
            return new Dictionary<string, string>(StringComparer.Ordinal)
            {
                ["UP"] = "ARROW_UP", ["DOWN"] = "ARROW_DOWN",
                ["LEFT"] = "ARROW_LEFT", ["RIGHT"] = "ARROW_RIGHT",
                ["A"] = "KEY_X", ["B"] = "KEY_Z",
                ["START"] = "ENTER", ["SELECT"] = "SPACE"
            };
        }
        var fields = ReadExactObject(element, Actions, "controls");
        var controls = new Dictionary<string, string>(StringComparer.Ordinal);
        var usedKeys = new HashSet<string>(StringComparer.Ordinal);
        foreach (string action in Actions)
        {
            string key = ReadString(fields[action], $"controls.{action}", 32);
            if (!SupportedKeys.Contains(key) || !usedKeys.Add(key))
            {
                throw new InvalidDataException($"NES control token for {action} is invalid or duplicated.");
            }
            controls.Add(action, key);
        }
        return controls;
    }

    private static HashSet<string> BuildSupportedKeys()
    {
        var keys = new HashSet<string>(StringComparer.Ordinal)
        {
            "ARROW_UP", "ARROW_DOWN", "ARROW_LEFT", "ARROW_RIGHT", "ENTER", "NUMPAD_ENTER", "SPACE"
        };
        foreach (char letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        {
            keys.Add("KEY_" + letter);
        }
        foreach (char digit in "0123456789")
        {
            keys.Add("DIGIT_" + digit);
            keys.Add("NUMPAD_" + digit);
        }
        return keys;
    }

    private static string ReadString(JsonElement element, string field, int maximumLength)
    {
        if (element.ValueKind != JsonValueKind.String)
        {
            throw new InvalidDataException($"Managed field {field} must be text.");
        }
        string value = element.GetString()!;
        if (value.Length == 0 || value.Length > maximumLength || value.Any(char.IsControl))
        {
            throw new InvalidDataException($"Managed field {field} has an invalid length or character.");
        }
        return value;
    }

    private static int ReadInt32(JsonElement element, string field)
    {
        if (element.ValueKind != JsonValueKind.Number || !element.TryGetInt32(out int value))
        {
            throw new InvalidDataException($"Managed field {field} must be a 32-bit integer.");
        }
        return value;
    }

    private static string ReadAbsolutePath(JsonElement element, string field)
    {
        string value = ReadString(element, field, 4096);
        if (!Path.IsPathFullyQualified(value))
        {
            throw new InvalidDataException($"Managed field {field} must be an absolute path.");
        }
        return value;
    }

    private static string FullPath(string path, string field)
    {
        try
        {
            return Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));
        }
        catch (Exception error) when (error is ArgumentException or IOException or NotSupportedException)
        {
            throw new InvalidDataException($"Managed field {field} is not a usable path.", error);
        }
    }

    private static bool IsHex(string value, int length) =>
        value.Length == length && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');

    private static bool SamePath(string left, string right) =>
        string.Equals(FullPath(left, "path"), FullPath(right, "path"), StringComparison.OrdinalIgnoreCase);

    internal static void EnsureNoLinkedSegments(string path)
    {
        string full = FullPath(path, "owned path");
        string root = Path.GetPathRoot(full)
            ?? throw new InvalidDataException("Managed path has no filesystem root.");
        string current = root;
        foreach (string part in full[root.Length..].Split(
                     [Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar],
                     StringSplitOptions.RemoveEmptyEntries))
        {
            current = Path.Combine(current, part);
            if (File.Exists(current) || Directory.Exists(current))
            {
                if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                {
                    throw new InvalidDataException("Managed paths cannot traverse links or junctions.");
                }
            }
        }
    }

    private static void RequireFile(string path, string description)
    {
        if (!File.Exists(path))
        {
            throw new InvalidDataException($"Managed {description} is missing.");
        }
    }

    private static void RequireDirectory(string path, string description)
    {
        if (!Directory.Exists(path))
        {
            throw new InvalidDataException($"Managed {description} is missing.");
        }
    }

    private static void VerifyRomIdentity(string romPath, string expectedSha256, string expectedGameId)
    {
        using var stream = new FileStream(romPath, FileMode.Open, FileAccess.Read, FileShare.Read,
            1024 * 1024, FileOptions.SequentialScan);
        if (stream.Length > 64L * 1024 * 1024)
        {
            throw new InvalidDataException("Managed NES ROM exceeds 64 MiB.");
        }
        byte[] digest = SHA256.HashData(stream);
        string actualSha256 = Convert.ToHexString(digest).ToLowerInvariant();
        if (!string.Equals(actualSha256, expectedSha256, StringComparison.Ordinal))
        {
            throw new InvalidDataException("Managed NES ROM hash does not match its launch manifest.");
        }

        using var namespaced = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        namespaced.AppendData(Encoding.UTF8.GetBytes("nes\0"));
        namespaced.AppendData(digest);
        string actualGameId = Convert.ToHexString(namespaced.GetHashAndReset()).ToLowerInvariant();
        if (!string.Equals(actualGameId, expectedGameId, StringComparison.Ordinal))
        {
            throw new InvalidDataException("Managed NES ROM does not match its namespaced game ID.");
        }
    }
}
