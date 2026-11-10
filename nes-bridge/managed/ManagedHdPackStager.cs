using System;
using System.IO;

namespace Revival.Mesen;

/// <summary>Copies a validated immutable pack into the private Mesen home for one Enhanced session.</summary>
public sealed class ManagedHdPackStager
{
    private readonly Action<string, string, long> _copyFile;

    public ManagedHdPackStager(Action<string, string, long>? copyFile = null)
    {
        _copyFile = copyFile ?? CopyExactFile;
    }

    /// <summary>Stages an Enhanced pack atomically; Original launches intentionally have no pack side effects.</summary>
    public string? Stage(RevivalLaunchManifest launch)
    {
        ArgumentNullException.ThrowIfNull(launch);
        if (launch.Mode == "original")
        {
            if (launch.PackManifest is not null)
            {
                throw new InvalidDataException("Original mode cannot stage an HD pack.");
            }
            return null;
        }
        if (launch.Mode != "enhanced")
        {
            throw new InvalidDataException("Managed launch mode must be original or enhanced.");
        }

        ValidatedManagedHdPack pack = ManagedHdPackValidator.Validate(launch);
        string home = Path.GetFullPath(pack.Home);
        string packsRoot = Path.GetFullPath(Path.Combine(home, "HdPacks"));
        string destination = Path.GetFullPath(Path.Combine(packsRoot, pack.RomName));
        string staging = Path.GetFullPath(Path.Combine(packsRoot,
            $".staging-{pack.RomName}-{Guid.NewGuid():N}"));
        ManagedHdPackValidator.EnsureContained(packsRoot, home, "private HD pack root");
        ManagedHdPackValidator.EnsureContained(destination, home, "private HD pack destination");
        ManagedHdPackValidator.EnsureContained(staging, home, "private HD pack staging folder");
        RevivalLaunch.EnsureNoLinkedSegments(home);
        RevivalLaunch.EnsureNoLinkedSegments(packsRoot);
        RevivalLaunch.EnsureNoLinkedSegments(destination);
        RevivalLaunch.EnsureNoLinkedSegments(staging);
        if (!Directory.Exists(home))
        {
            throw new InvalidDataException("Managed private Mesen home is missing.");
        }
        if (File.Exists(packsRoot))
        {
            throw new InvalidDataException("Private HD pack root is a file.");
        }
        if (File.Exists(destination) || Directory.Exists(destination))
        {
            throw new InvalidDataException("Private HD pack destination already exists.");
        }
        if (File.Exists(staging) || Directory.Exists(staging))
        {
            throw new InvalidDataException("Private HD pack staging path already exists.");
        }

        try
        {
            Directory.CreateDirectory(packsRoot);
            RevivalLaunch.EnsureNoLinkedSegments(packsRoot);
            Directory.CreateDirectory(staging);
            RevivalLaunch.EnsureNoLinkedSegments(staging);
            foreach (ManagedHdPackFile file in pack.Files)
            {
                string target = Path.Combine(staging, file.Name);
                ManagedHdPackValidator.EnsureContained(target, home, "staged HD pack file");
                RevivalLaunch.EnsureNoLinkedSegments(file.SourcePath);
                RevivalLaunch.EnsureNoLinkedSegments(target);
                _copyFile(file.SourcePath, target, file.Length);
                RevivalLaunch.EnsureNoLinkedSegments(target);
                var copied = new FileInfo(target);
                if (!copied.Exists || copied.Length != file.Length)
                {
                    throw new InvalidDataException($"Staged HD pack file {file.Name} is incomplete.");
                }
            }

            if (File.Exists(destination) || Directory.Exists(destination))
            {
                throw new InvalidDataException("Private HD pack destination appeared during staging.");
            }
            Directory.Move(staging, destination);
            return destination;
        }
        catch (Exception error)
        {
            try
            {
                if (Directory.Exists(staging))
                {
                    Directory.Delete(staging, recursive: true);
                }
            }
            catch (Exception cleanupError)
            {
                throw new IOException(
                    $"Managed HD pack staging failed ({error.Message}) and its temporary files could not be removed.",
                    new AggregateException(error, cleanupError));
            }
            throw new InvalidDataException($"Managed HD pack staging failed: {error.Message}", error);
        }
    }

    private static void CopyExactFile(string source, string destination, long expectedLength)
    {
        using var input = new FileStream(source, FileMode.Open, FileAccess.Read, FileShare.Read,
            64 * 1024, FileOptions.SequentialScan);
        if (input.Length != expectedLength)
        {
            throw new InvalidDataException("HD pack source changed after validation.");
        }
        using var output = new FileStream(destination, FileMode.CreateNew, FileAccess.Write, FileShare.None,
            64 * 1024, FileOptions.SequentialScan);
        byte[] buffer = new byte[64 * 1024];
        long remaining = expectedLength;
        while (remaining > 0)
        {
            int requested = (int)Math.Min(buffer.Length, remaining);
            int read = input.Read(buffer, 0, requested);
            if (read == 0)
            {
                throw new InvalidDataException("HD pack source ended during staging.");
            }
            output.Write(buffer, 0, read);
            remaining -= read;
        }
        if (input.ReadByte() != -1)
        {
            throw new InvalidDataException("HD pack source grew during staging.");
        }
        output.Flush(flushToDisk: true);
    }
}
