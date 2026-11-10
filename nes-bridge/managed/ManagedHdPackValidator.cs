using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.Json;

namespace Revival.Mesen;

internal sealed record ManagedHdPackFile(string Name, string SourcePath, long Length);

internal sealed record ManagedHdPackTile(int Page, string SelectorTag, int X, int Y);

internal sealed record ManagedHdPackPage(string Name, int Width, int Height);

internal sealed record ValidatedManagedHdPack(
    string Generation,
    string RomName,
    string Home,
    IReadOnlyList<ManagedHdPackFile> Files);

/// <summary>Validates the generated pack format before it reaches Mesen's native loader.</summary>
internal static class ManagedHdPackValidator
{
    internal const long MaximumPackBytes = 64L * 1024 * 1024;

    private static readonly string[] ManifestFields =
    ["schema", "recipe_key", "pack_scale", "bindings", "generation", "pages", "tiles"];

    private static readonly string[] BindingFields = ["binding_id", "selector", "source_id"];
    private static readonly string[] TileFields =
    ["binding_id", "selector", "source_id", "page", "x", "y", "size"];

    internal static ValidatedManagedHdPack Validate(RevivalLaunchManifest launch)
    {
        ArgumentNullException.ThrowIfNull(launch);
        if (launch.Mode != "enhanced" || launch.PackManifest is null)
        {
            throw new InvalidDataException("HD pack validation requires an Enhanced launch with a pack.");
        }
        if (launch.PackScale is not (2 or 4))
        {
            throw new InvalidDataException("HD pack launch scale must be 2 or 4.");
        }

        string workspace = GetWorkspace(launch);
        string generation = Path.GetFileName(Path.GetDirectoryName(launch.PackManifest)
            ?? throw new InvalidDataException("HD pack manifest has no generation directory."));
        if (!IsLowerHex(generation, 32))
        {
            throw new InvalidDataException("HD pack generation folder name is invalid.");
        }

        string expectedPackDirectory = Path.Combine(workspace, "nes", "packs", generation);
        string expectedManifest = Path.Combine(expectedPackDirectory, "manifest.json");
        if (!SamePath(launch.PackManifest, expectedManifest))
        {
            throw new InvalidDataException("HD pack manifest is outside its selected immutable generation.");
        }

        RequireOwnedDirectory(workspace, workspace, "game workspace");
        RequireOwnedDirectory(expectedPackDirectory, Path.Combine(workspace, "nes", "packs"),
            "HD pack generation");
        RequireOwnedFile(expectedManifest, expectedPackDirectory, "HD pack manifest");
        long manifestLength = GetLength(expectedManifest, "HD pack manifest");
        byte[] manifestBytes = ReadBounded(expectedManifest, manifestLength, "HD pack manifest");

        using JsonDocument document = JsonDocument.Parse(manifestBytes,
            new JsonDocumentOptions { MaxDepth = 8, CommentHandling = JsonCommentHandling.Disallow });
        Dictionary<string, JsonElement> fields = ReadObject(document.RootElement, ManifestFields,
            "HD pack manifest");
        if (ReadInt32(fields["schema"], "HD pack schema") != 1)
        {
            throw new InvalidDataException("HD pack manifest schema is unsupported.");
        }
        _ = ReadString(fields["recipe_key"], "HD pack recipe key", 256);
        int manifestScale = ReadInt32(fields["pack_scale"], "HD pack scale");
        if (manifestScale != launch.PackScale)
        {
            throw new InvalidDataException("HD pack scale does not match the launch scale.");
        }
        if (!string.Equals(ReadString(fields["generation"], "HD pack generation", 32),
                generation, StringComparison.Ordinal))
        {
            throw new InvalidDataException("HD pack generation does not match its selected folder.");
        }

        List<string> pageNames = ReadPages(fields["pages"]);
        Dictionary<string, (string Selector, string SourceId)> bindings = ReadBindings(fields["bindings"]);
        List<ManagedHdPackTile> tiles = ReadTiles(fields["tiles"], bindings, launch.PackScale);
        if (tiles.Count != bindings.Count)
        {
            throw new InvalidDataException("HD pack bindings and tile definitions do not match.");
        }

        var pages = new List<ManagedHdPackPage>(pageNames.Count);
        long totalBytes = manifestLength;
        string definitionPath = Path.Combine(expectedPackDirectory, "hires.txt");
        long definitionLength = AddFileLength(definitionPath, expectedPackDirectory, "HD pack definition",
            ref totalBytes);
        var sourceFiles = new List<ManagedHdPackFile>(pageNames.Count + 1)
        {
            new("hires.txt", definitionPath, definitionLength)
        };

        foreach (string pageName in pageNames)
        {
            string pagePath = Path.Combine(expectedPackDirectory, pageName);
            long pageLength = AddFileLength(pagePath, expectedPackDirectory, "HD pack image page",
                ref totalBytes);
            pages.Add(ReadPage(pageName, pagePath, pageLength, expectedPackDirectory, launch.PackScale));
            sourceFiles.Add(new ManagedHdPackFile(pageName, pagePath, pageLength));
        }

        if (totalBytes > MaximumPackBytes)
        {
            throw new InvalidDataException("HD pack exceeds the 64 MiB generated-pack limit.");
        }

        ValidateTiles(tiles, pages, launch.PackScale);
        byte[] definitionBytes = ReadBounded(definitionPath, definitionLength, "HD pack definition");
        ValidateDefinition(definitionBytes, pageNames, tiles, launch.PackScale);

        string romName = Path.GetFileNameWithoutExtension(launch.RomPath);
        if (string.IsNullOrWhiteSpace(romName) || romName is "." or ".." ||
            romName.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 ||
            !string.Equals(Path.GetFileName(romName), romName, StringComparison.Ordinal))
        {
            throw new InvalidDataException("NES ROM filename cannot be used as an HD pack folder name.");
        }

        RevivalLaunch.EnsureNoLinkedSegments(launch.Home);
        RequireDirectory(launch.Home, "private Mesen home");
        return new ValidatedManagedHdPack(generation, romName, launch.Home, sourceFiles);
    }

    private static string GetWorkspace(RevivalLaunchManifest launch)
    {
        string gameDirectory = Path.GetDirectoryName(launch.RomPath)
            ?? throw new InvalidDataException("NES ROM has no game directory.");
        string libraryDirectory = Path.GetDirectoryName(gameDirectory)
            ?? throw new InvalidDataException("NES ROM has no library directory.");
        string nesRoot = Path.GetDirectoryName(libraryDirectory)
            ?? throw new InvalidDataException("NES ROM has no NES root.");
        return Path.GetFullPath(Path.Combine(nesRoot, "rendered-games", launch.GameId));
    }

    private static List<string> ReadPages(JsonElement element)
    {
        if (element.ValueKind != JsonValueKind.Array || element.GetArrayLength() == 0)
        {
            throw new InvalidDataException("HD pack must declare at least one image page.");
        }

        var names = new List<string>(element.GetArrayLength());
        int index = 0;
        foreach (JsonElement page in element.EnumerateArray())
        {
            string name = ReadString(page, "HD pack page name", 32);
            string expected = "page-" + index.ToString("D3", CultureInfo.InvariantCulture) + ".png";
            if (!string.Equals(name, expected, StringComparison.Ordinal))
            {
                throw new InvalidDataException("HD pack page names must be ordered generated page files.");
            }
            names.Add(name);
            index++;
        }
        return names;
    }

    private static Dictionary<string, (string Selector, string SourceId)> ReadBindings(JsonElement element)
    {
        if (element.ValueKind != JsonValueKind.Array || element.GetArrayLength() == 0)
        {
            throw new InvalidDataException("HD pack must contain at least one generated binding.");
        }

        var bindings = new Dictionary<string, (string Selector, string SourceId)>(StringComparer.Ordinal);
        foreach (JsonElement item in element.EnumerateArray())
        {
            Dictionary<string, JsonElement> fields = ReadObject(item, BindingFields, "HD pack binding");
            string id = ReadString(fields["binding_id"], "HD pack binding ID", 256);
            string selector = ReadString(fields["selector"], "HD pack selector", 128);
            string sourceId = ReadString(fields["source_id"], "HD pack source ID", 64);
            if (!IsLowerHex(sourceId, 64) || !bindings.TryAdd(id, (selector, sourceId)))
            {
                throw new InvalidDataException("HD pack binding identity is invalid or duplicated.");
            }
        }
        return bindings;
    }

    private static List<ManagedHdPackTile> ReadTiles(
        JsonElement element,
        IReadOnlyDictionary<string, (string Selector, string SourceId)> bindings,
        int scale)
    {
        if (element.ValueKind != JsonValueKind.Array || element.GetArrayLength() == 0 ||
            element.GetArrayLength() != bindings.Count)
        {
            throw new InvalidDataException("HD pack tile metadata does not match its bindings.");
        }

        var tiles = new List<ManagedHdPackTile>(element.GetArrayLength());
        var usedBindings = new HashSet<string>(StringComparer.Ordinal);
        var usedCoordinates = new HashSet<(int Page, int X, int Y)>();
        foreach (JsonElement item in element.EnumerateArray())
        {
            Dictionary<string, JsonElement> fields = ReadObject(item, TileFields, "HD pack tile");
            string id = ReadString(fields["binding_id"], "HD pack tile binding ID", 256);
            string selector = ReadString(fields["selector"], "HD pack tile selector", 128);
            string sourceId = ReadString(fields["source_id"], "HD pack tile source ID", 64);
            if (!bindings.TryGetValue(id, out var binding) ||
                !string.Equals(binding.Selector, selector, StringComparison.Ordinal) ||
                !string.Equals(binding.SourceId, sourceId, StringComparison.Ordinal) ||
                !usedBindings.Add(id))
            {
                throw new InvalidDataException("HD pack tile metadata does not match its binding.");
            }

            int page = ReadNonNegativeInt(fields["page"], "HD pack tile page");
            int x = ReadNonNegativeInt(fields["x"], "HD pack tile X position");
            int y = ReadNonNegativeInt(fields["y"], "HD pack tile Y position");
            if (ReadInt32(fields["size"], "HD pack tile size") != 8 * scale)
            {
                throw new InvalidDataException("HD pack tile size does not match its scale.");
            }
            if (!usedCoordinates.Add((page, x, y)))
            {
                throw new InvalidDataException("HD pack tiles cannot overlap in the generated atlas.");
            }
            tiles.Add(new ManagedHdPackTile(page, SelectorTag(selector), x, y));
        }
        return tiles;
    }

    private static string SelectorTag(string selector)
    {
        string[] parts = selector.Split(':', 2);
        if (parts.Length != 2)
        {
            throw new InvalidDataException("HD pack selector format is unsupported.");
        }
        string[] values = parts[1].Split(',');
        if (parts[0] == "rom" && values.Length == 2 && IsUpperHex(values[0], 1, 8) &&
            IsUpperHex(values[1], 8, 8))
        {
            return values[0] + "," + values[1];
        }
        if (parts[0] == "ram" && values.Length == 2 && IsUpperHex(values[0], 32, 32) &&
            IsUpperHex(values[1], 8, 8))
        {
            return values[0] + "," + values[1] + ",0,-1";
        }
        throw new InvalidDataException("HD pack selector format is unsupported.");
    }

    private static ManagedHdPackPage ReadPage(
        string name, string path, long length, string root, int scale)
    {
        if (length < 33)
        {
            throw new InvalidDataException($"HD pack page {name} is too short to be a PNG.");
        }
        RequireOwnedFile(path, root, "HD pack image page");
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read,
            4096, FileOptions.SequentialScan);
        if (stream.Length != length)
        {
            throw new InvalidDataException($"HD pack page {name} changed during validation.");
        }
        Span<byte> header = stackalloc byte[33];
        stream.ReadExactly(header);
        ReadOnlySpan<byte> signature = [137, 80, 78, 71, 13, 10, 26, 10];
        if (!header[..8].SequenceEqual(signature) ||
            BinaryPrimitives.ReadUInt32BigEndian(header[8..12]) != 13 ||
            !header[12..16].SequenceEqual("IHDR"u8) || header[24] != 8 || header[25] != 6 ||
            header[26] != 0 || header[27] != 0 || header[28] != 0)
        {
            throw new InvalidDataException($"HD pack page {name} is not a generated 8-bit RGBA PNG.");
        }
        uint rawWidth = BinaryPrimitives.ReadUInt32BigEndian(header[16..20]);
        uint rawHeight = BinaryPrimitives.ReadUInt32BigEndian(header[20..24]);
        int tileSize = 8 * scale;
        if (rawWidth == 0 || rawHeight == 0 || rawWidth > 2048 || rawHeight > 2048 ||
            rawWidth % tileSize != 0 || rawHeight % tileSize != 0)
        {
            throw new InvalidDataException($"HD pack page {name} has invalid generated dimensions.");
        }
        return new ManagedHdPackPage(name, (int)rawWidth, (int)rawHeight);
    }

    private static void ValidateTiles(
        IReadOnlyList<ManagedHdPackTile> tiles,
        IReadOnlyList<ManagedHdPackPage> pages,
        int scale)
    {
        int tileSize = 8 * scale;
        foreach (ManagedHdPackTile tile in tiles)
        {
            if (tile.Page < 0 || tile.Page >= pages.Count)
            {
                throw new InvalidDataException("HD pack tile refers to an undeclared image page.");
            }
            ManagedHdPackPage page = pages[tile.Page];
            if (tile.X % tileSize != 0 || tile.Y % tileSize != 0 ||
                tile.X > page.Width - tileSize || tile.Y > page.Height - tileSize)
            {
                throw new InvalidDataException("HD pack tile lies outside its generated image page.");
            }
        }
    }

    private static void ValidateDefinition(
        byte[] bytes,
        IReadOnlyList<string> pageNames,
        IReadOnlyList<ManagedHdPackTile> metadataTiles,
        int scale)
    {
        if (bytes.Any(value => value > 0x7f))
        {
            throw new InvalidDataException("HD pack definition must use generated ASCII text.");
        }
        string text = Encoding.ASCII.GetString(bytes);
        string[] lines = text.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n');
        if (lines.Any(line => line.Contains('\r')))
        {
            throw new InvalidDataException("HD pack definition contains an unsupported line ending.");
        }
        int lineCount = lines.Length;
        if (lineCount > 0 && lines[^1].Length == 0)
        {
            lineCount--;
        }
        if (lineCount < 4 || lines[0] != "<ver>109" ||
            lines[1] != "<scale>" + scale.ToString(CultureInfo.InvariantCulture))
        {
            throw new InvalidDataException("HD pack definition must use the generated version 109 scale header.");
        }

        int cursor = 2;
        for (int page = 0; page < pageNames.Count; page++, cursor++)
        {
            if (cursor >= lineCount || lines[cursor] != "<img>" + pageNames[page])
            {
                throw new InvalidDataException("HD pack definition image references do not match declared pages.");
            }
        }

        var definitionTiles = new HashSet<ManagedHdPackTile>();
        for (; cursor < lineCount; cursor++)
        {
            if (string.IsNullOrEmpty(lines[cursor]) || !lines[cursor].StartsWith("<tile>", StringComparison.Ordinal))
            {
                throw new InvalidDataException("HD pack definition contains an unexpected directive.");
            }
            ManagedHdPackTile tile = ParseTileDirective(lines[cursor][6..], scale);
            if (!definitionTiles.Add(tile))
            {
                throw new InvalidDataException("HD pack definition contains a duplicate tile placement.");
            }
        }

        if (definitionTiles.Count != metadataTiles.Count ||
            metadataTiles.Any(tile => !definitionTiles.Contains(tile)))
        {
            throw new InvalidDataException("HD pack definition tile references do not match its manifest.");
        }
    }

    private static ManagedHdPackTile ParseTileDirective(string text, int scale)
    {
        string[] fields = text.Split(',');
        int page;
        string selectorTag;
        int x;
        int y;
        if (fields.Length == 7 && IsUpperHex(fields[1], 1, 8) && IsUpperHex(fields[2], 8, 8))
        {
            page = ReadDecimal(fields[0], "HD pack tile page");
            selectorTag = fields[1] + "," + fields[2];
            x = ReadDecimal(fields[3], "HD pack tile X position");
            y = ReadDecimal(fields[4], "HD pack tile Y position");
            if (fields[5] != "1" || fields[6] != "N")
            {
                throw new InvalidDataException("HD pack ROM tile directive uses unsupported flags.");
            }
        }
        else if (fields.Length == 9 && IsUpperHex(fields[1], 32, 32) &&
                 IsUpperHex(fields[2], 8, 8) && fields[3] == "0" && fields[4] == "-1")
        {
            page = ReadDecimal(fields[0], "HD pack tile page");
            selectorTag = fields[1] + "," + fields[2] + ",0,-1";
            x = ReadDecimal(fields[5], "HD pack tile X position");
            y = ReadDecimal(fields[6], "HD pack tile Y position");
            if (fields[7] != "1" || fields[8] != "N")
            {
                throw new InvalidDataException("HD pack RAM tile directive uses unsupported flags.");
            }
        }
        else
        {
            throw new InvalidDataException("HD pack tile directive does not match the generated version 109 format.");
        }

        if (x % (8 * scale) != 0 || y % (8 * scale) != 0)
        {
            throw new InvalidDataException("HD pack tile placement is not aligned to its generated scale.");
        }
        return new ManagedHdPackTile(page, selectorTag, x, y);
    }

    private static long AddFileLength(string path, string root, string description, ref long total)
    {
        RequireOwnedFile(path, root, description);
        long length = GetLength(path, description);
        try
        {
            total = checked(total + length);
        }
        catch (OverflowException error)
        {
            throw new InvalidDataException("HD pack byte count overflowed its limit.", error);
        }
        if (total > MaximumPackBytes)
        {
            throw new InvalidDataException("HD pack exceeds the 64 MiB generated-pack limit.");
        }
        return length;
    }

    private static long GetLength(string path, string description)
    {
        try
        {
            long length = new FileInfo(path).Length;
            if (length < 0 || length > MaximumPackBytes)
            {
                throw new InvalidDataException($"Managed {description} exceeds the 64 MiB limit.");
            }
            return length;
        }
        catch (FileNotFoundException error)
        {
            throw new InvalidDataException($"Managed {description} is missing.", error);
        }
        catch (DirectoryNotFoundException error)
        {
            throw new InvalidDataException($"Managed {description} is missing.", error);
        }
    }

    private static byte[] ReadBounded(string path, long expectedLength, string description)
    {
        if (expectedLength > MaximumPackBytes || expectedLength > int.MaxValue)
        {
            throw new InvalidDataException($"Managed {description} exceeds its read limit.");
        }
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read,
            4096, FileOptions.SequentialScan);
        if (stream.Length != expectedLength)
        {
            throw new InvalidDataException($"Managed {description} changed during validation.");
        }
        byte[] buffer = new byte[(int)expectedLength];
        stream.ReadExactly(buffer);
        if (stream.ReadByte() != -1)
        {
            throw new InvalidDataException($"Managed {description} grew during validation.");
        }
        return buffer;
    }

    private static Dictionary<string, JsonElement> ReadObject(
        JsonElement element, IEnumerable<string> expectedFields, string description)
    {
        if (element.ValueKind != JsonValueKind.Object)
        {
            throw new InvalidDataException($"Managed {description} must be an object.");
        }
        var expected = new HashSet<string>(expectedFields, StringComparer.Ordinal);
        var values = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        foreach (JsonProperty property in element.EnumerateObject())
        {
            if (!expected.Contains(property.Name) || !values.TryAdd(property.Name, property.Value))
            {
                throw new InvalidDataException($"Managed {description} has an unknown or duplicate field.");
            }
        }
        if (values.Count != expected.Count)
        {
            throw new InvalidDataException($"Managed {description} is missing a required field.");
        }
        return values;
    }

    private static string ReadString(JsonElement element, string description, int maximumLength)
    {
        if (element.ValueKind != JsonValueKind.String)
        {
            throw new InvalidDataException($"Managed {description} must be text.");
        }
        string value = element.GetString()!;
        if (value.Length == 0 || value.Length > maximumLength || value.Any(char.IsControl))
        {
            throw new InvalidDataException($"Managed {description} has an invalid length or character.");
        }
        return value;
    }

    private static int ReadInt32(JsonElement element, string description)
    {
        if (element.ValueKind != JsonValueKind.Number || !element.TryGetInt32(out int value))
        {
            throw new InvalidDataException($"Managed {description} must be an integer.");
        }
        return value;
    }

    private static int ReadNonNegativeInt(JsonElement element, string description)
    {
        int value = ReadInt32(element, description);
        if (value < 0)
        {
            throw new InvalidDataException($"Managed {description} cannot be negative.");
        }
        return value;
    }

    private static int ReadDecimal(string value, string description)
    {
        if (!int.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out int parsed) || parsed < 0)
        {
            throw new InvalidDataException($"Managed {description} is not a nonnegative decimal number.");
        }
        return parsed;
    }

    private static bool IsLowerHex(string value, int length) =>
        value.Length == length && value.All(character => character is >= '0' and <= '9' or >= 'a' and <= 'f');

    private static bool IsUpperHex(string value, int minimumLength, int maximumLength) =>
        value.Length >= minimumLength && value.Length <= maximumLength &&
        value.All(character => character is >= '0' and <= '9' or >= 'A' and <= 'F');

    private static bool SamePath(string left, string right) =>
        string.Equals(Path.GetFullPath(left), Path.GetFullPath(right), StringComparison.OrdinalIgnoreCase);

    private static void RequireOwnedDirectory(string path, string root, string description)
    {
        EnsureContained(path, root, description);
        RevivalLaunch.EnsureNoLinkedSegments(path);
        if (!Directory.Exists(path))
        {
            throw new InvalidDataException($"Managed {description} is missing.");
        }
    }

    private static void RequireOwnedFile(string path, string root, string description)
    {
        EnsureContained(path, root, description);
        RevivalLaunch.EnsureNoLinkedSegments(path);
        if (!File.Exists(path) || Directory.Exists(path))
        {
            throw new InvalidDataException($"Managed {description} is missing or is not a regular file.");
        }
    }

    internal static void EnsureContained(string path, string root, string description)
    {
        string fullPath = Path.GetFullPath(path);
        string fullRoot = Path.GetFullPath(root);
        string relative = Path.GetRelativePath(fullRoot, fullPath);
        if (Path.IsPathRooted(relative) || relative == ".." ||
            relative.StartsWith(".." + Path.DirectorySeparatorChar, StringComparison.Ordinal) ||
            relative.StartsWith(".." + Path.AltDirectorySeparatorChar, StringComparison.Ordinal))
        {
            throw new InvalidDataException($"Managed {description} escapes its owned folder.");
        }
    }

    private static void RequireDirectory(string path, string description)
    {
        if (!Directory.Exists(path))
        {
            throw new InvalidDataException($"Managed {description} is missing.");
        }
    }
}
