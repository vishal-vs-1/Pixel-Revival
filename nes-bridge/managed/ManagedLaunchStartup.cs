using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text.Json;

namespace Revival.Mesen;

/// <summary>Checks that the managed frontend is paired with the common graphics runtime it was launched for.</summary>
public static class ManagedLaunchStartup
{
    private const int MaximumGraphicsManifestBytes = 65536;
    private const long MaximumNativeLibraryBytes = 536870912;

    public static void VerifyRuntime(RevivalLaunchManifest launch)
    {
        ArgumentNullException.ThrowIfNull(launch);
        string corePath = Path.Combine(AppContext.BaseDirectory, "MesenCore.dll");
        string[] requiredLibraries =
        [
            corePath,
            Path.Combine(AppContext.BaseDirectory, "libSkiaSharp.dll"),
            Path.Combine(AppContext.BaseDirectory, "libHarfBuzzSharp.dll")
        ];
        foreach (string library in requiredLibraries)
        {
            RevivalLaunch.EnsureNoLinkedSegments(library);
            if (!File.Exists(library) || new FileInfo(library).Length > MaximumNativeLibraryBytes)
            {
                throw new InvalidDataException("Managed Mesen runtime is missing or has an oversized native library.");
            }
        }
        RevivalLaunch.EnsureNoLinkedSegments(launch.GraphicsManifest);
        if (!File.Exists(launch.GraphicsManifest))
        {
            throw new InvalidDataException("Managed Mesen runtime is missing a required native library or manifest.");
        }

        byte[] raw;
        using (var stream = new FileStream(launch.GraphicsManifest, FileMode.Open, FileAccess.Read,
                   FileShare.Read, 4096, FileOptions.SequentialScan))
        {
            if (stream.Length > MaximumGraphicsManifestBytes)
            {
                throw new InvalidDataException("Common graphics manifest exceeds 64 KiB.");
            }
            raw = new byte[(int)stream.Length];
            stream.ReadExactly(raw);
        }

        using JsonDocument document = JsonDocument.Parse(raw, new JsonDocumentOptions { MaxDepth = 4 });
        JsonElement root = document.RootElement;
        if (root.ValueKind != JsonValueKind.Object)
        {
            throw new InvalidDataException("Common graphics manifest must be an object.");
        }
        var fields = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        foreach (JsonProperty property in root.EnumerateObject())
        {
            if (!fields.TryAdd(property.Name, property.Value))
            {
                throw new InvalidDataException("Common graphics manifest has duplicate fields.");
            }
        }
        if (fields.Count != 5 || !fields.ContainsKey("schema") || !fields.ContainsKey("protocol") ||
            !fields.ContainsKey("abi_version") || !fields.ContainsKey("build_id") || !fields.ContainsKey("dll") ||
            fields["schema"].GetInt32() != 1 || fields["protocol"].GetString() != "common-v1" ||
            fields["abi_version"].GetInt32() != 1)
        {
            throw new InvalidDataException("Common graphics manifest schema or protocol is unsupported.");
        }

        string buildId = fields["build_id"].GetString() ?? "";
        if (buildId.Length != 64 || buildId.Any(character => !Uri.IsHexDigit(character)) ||
            buildId != buildId.ToLowerInvariant())
        {
            throw new InvalidDataException("Common graphics build ID is invalid.");
        }
        string graphicsValue = fields["dll"].GetString() ?? "";
        string manifestDirectory = Path.GetDirectoryName(launch.GraphicsManifest)
            ?? throw new InvalidDataException("Common graphics manifest has no parent directory.");
        string runtimeRoot = Directory.GetParent(manifestDirectory)?.FullName ?? manifestDirectory;
        string sourceGraphics = Path.IsPathFullyQualified(graphicsValue)
            ? Path.GetFullPath(graphicsValue)
            : Path.GetFullPath(Path.Combine(manifestDirectory, graphicsValue));
        string relativeGraphics = Path.GetRelativePath(runtimeRoot, sourceGraphics);
        if (string.IsNullOrEmpty(graphicsValue) || Path.IsPathRooted(relativeGraphics) ||
            relativeGraphics == ".." || relativeGraphics.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal) ||
            relativeGraphics.StartsWith(".." + Path.AltDirectorySeparatorChar, StringComparison.Ordinal))
        {
            throw new InvalidDataException("Common graphics DLL path must stay inside the portable runtime folder.");
        }
        RevivalLaunch.EnsureNoLinkedSegments(sourceGraphics);
        if (!File.Exists(sourceGraphics))
        {
            throw new InvalidDataException("Common graphics DLL named by its manifest is missing.");
        }
        if (new FileInfo(sourceGraphics).Length > MaximumNativeLibraryBytes)
        {
            throw new InvalidDataException("Common graphics DLL exceeds the managed runtime size limit.");
        }

        string sourceHash = Hash(sourceGraphics);
        if (!string.Equals(sourceHash, buildId, StringComparison.Ordinal))
        {
            throw new InvalidDataException("Common graphics DLL does not match the launch manifest.");
        }

        Console.WriteLine($"[revival-managed] MesenCore sha256={Hash(corePath)}");
        Console.WriteLine($"[revival-managed] graphics sha256={sourceHash} abi=1 protocol=common-v1");
    }

    public static string ReadExpectedAbiBuildId(string graphicsManifestPath)
    {
        if (string.IsNullOrWhiteSpace(graphicsManifestPath) ||
            !Path.IsPathFullyQualified(graphicsManifestPath))
        {
            throw new InvalidDataException("Graphics manifest path must be absolute.");
        }

        string manifest = Path.GetFullPath(graphicsManifestPath);
        RevivalLaunch.EnsureNoLinkedSegments(manifest);
        string manifestDirectory = Path.GetDirectoryName(manifest)
            ?? throw new InvalidDataException("Graphics manifest has no parent directory.");
        string runtimeRoot = Directory.GetParent(manifestDirectory)?.FullName
            ?? throw new InvalidDataException("Graphics runtime has no parent directory.");
        string identityPath = Path.Combine(runtimeRoot, "common", "abi-build-id.txt");
        RevivalLaunch.EnsureNoLinkedSegments(identityPath);
        if (!File.Exists(identityPath))
        {
            throw new InvalidDataException("Common graphics ABI build identity is missing.");
        }

        using var identityStream = new FileStream(identityPath, FileMode.Open, FileAccess.Read,
            FileShare.Read, 66, FileOptions.SequentialScan);
        if (identityStream.Length is < 64 or > 66)
        {
            throw new InvalidDataException("Common graphics ABI build identity has an invalid size.");
        }
        byte[] bytes = new byte[(int)identityStream.Length];
        identityStream.ReadExactly(bytes);
        string buildId = System.Text.Encoding.ASCII.GetString(bytes, 0, 64);
        if (buildId.Any(character => !(character is >= '0' and <= '9' or >= 'a' and <= 'f')) ||
            bytes.Length == 65 && bytes[64] != (byte)'\n' ||
            bytes.Length == 66 && (bytes[64] != (byte)'\r' || bytes[65] != (byte)'\n'))
        {
            throw new InvalidDataException("Common graphics ABI build identity is malformed.");
        }
        return buildId;
    }

    private static string Hash(string path)
    {
        using FileStream stream = File.OpenRead(path);
        return Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
    }
}
