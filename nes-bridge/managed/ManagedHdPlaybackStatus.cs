using System;
using System.Linq;
using System.Runtime.InteropServices;

namespace Revival.Mesen;

[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)]
public struct ManagedHdNativeStatus
{
    public uint StructBytes;
    public uint AbiVersion;
    public uint PackLoaded;
    public uint PackScale;
    public uint CommonRendererActive;
    public uint FrameWidth;
    public uint FrameHeight;
    public uint Reserved;
    public uint IdentityAvailable;
    public uint GraphicsAbiVersion;
    public ulong SubmittedFrames;
    public ulong PresentedFrames;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 65)] public string? GraphicsBuildId;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 129)] public string? AdapterVendor;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 257)] public string? AdapterRenderer;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 129)] public string? AdapterVersion;
}

public readonly record struct ManagedHdFrameStatus(
    uint PackLoaded,
    uint PackScale,
    uint CommonRendererActive,
    ulong SubmittedFrames,
    ulong PresentedFrames,
    uint Width,
    uint Height);

public readonly record struct ManagedGraphicsIdentity(
    bool Available,
    uint AbiVersion,
    string? BuildId,
    string? Vendor,
    string? Renderer,
    string? Version);

/// <summary>Checks native loader and presented-frame evidence without owning either runtime.</summary>
public static class ManagedHdPlaybackStatus
{
    public static bool IsHdFrameReady(ManagedHdFrameStatus status, int expectedScale)
    {
        if (expectedScale is not (2 or 4) || status.PackLoaded != 1 ||
            status.PackScale != expectedScale || status.CommonRendererActive != 1 ||
            status.SubmittedFrames == 0 || status.PresentedFrames == 0)
        {
            return false;
        }

        // Allow overscan cropping while still rejecting native-resolution frames.
        return status.Width >= 192u * (uint)expectedScale &&
               status.Height >= 180u * (uint)expectedScale;
    }

    public static bool IdentityMatches(ManagedGraphicsIdentity identity, string expectedBuildId)
    {
        return identity.Available && identity.AbiVersion == 1 &&
               IsBuildId(expectedBuildId) &&
               string.Equals(identity.BuildId, expectedBuildId, StringComparison.Ordinal) &&
               !string.IsNullOrWhiteSpace(identity.Vendor) &&
               !string.IsNullOrWhiteSpace(identity.Renderer);
    }

    public static bool IsReady(ManagedHdFrameStatus status, ManagedGraphicsIdentity identity,
        int expectedScale, string expectedBuildId)
    {
        return IsHdFrameReady(status, expectedScale) &&
               IdentityMatches(identity, expectedBuildId);
    }

    public static string Describe(ManagedHdFrameStatus status)
    {
        return $"pack={(status.PackLoaded == 1 ? "loaded" : "missing")} " +
               $"scale={status.PackScale} renderer={(status.CommonRendererActive == 1 ? "common" : "inactive")} " +
               $"submitted={status.SubmittedFrames} presented={status.PresentedFrames} " +
               $"frame={status.Width}x{status.Height}";
    }

    public static string Describe(ManagedHdFrameStatus status, ManagedGraphicsIdentity identity,
        string expectedBuildId)
    {
        string build = IsBuildId(identity.BuildId)
            ? identity.BuildId!
            : "unavailable";
        string buildMatch = IdentityMatches(identity, expectedBuildId) ? "match" : "mismatch";
        string vendor = Clip(identity.Vendor, 40);
        string renderer = Clip(identity.Renderer, 72);
        string version = Clip(identity.Version, 28);
        string details = $"{Describe(status)} abi={identity.AbiVersion} build={buildMatch}:{build} " +
                         $"adapter={vendor}/{renderer} gl={version}";
        return details.Length <= 240 ? details : details[..240];
    }

    private static bool IsBuildId(string? value)
    {
        return value is { Length: 64 } && value.All(character =>
            character is >= '0' and <= '9' or >= 'a' and <= 'f');
    }

    private static string Clip(string? value, int maximum)
    {
        string clean = new string((value ?? string.Empty)
            .Where(character => !char.IsControl(character)).ToArray());
        return clean.Length <= maximum ? clean : clean[..maximum];
    }
}
