using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;

namespace Revival.Mesen;

public sealed record NesControlCodes(
    ushort Up,
    ushort Down,
    ushort Left,
    ushort Right,
    ushort A,
    ushort B,
    ushort Start,
    ushort Select);

public static class RevivalNesControls
{
    private static readonly string[] Actions =
        ["UP", "DOWN", "LEFT", "RIGHT", "A", "B", "START", "SELECT"];

    public static string ToNativeKeyName(string token)
    {
        ArgumentNullException.ThrowIfNull(token);

        if (token.Length == 5 && token.StartsWith("KEY_", StringComparison.Ordinal) &&
            token[4] is >= 'A' and <= 'Z')
        {
            return token[4].ToString();
        }
        if (token.Length == 7 && token.StartsWith("DIGIT_", StringComparison.Ordinal) &&
            token[6] is >= '0' and <= '9')
        {
            return token[6].ToString();
        }
        if (token.Length == 8 && token.StartsWith("NUMPAD_", StringComparison.Ordinal) &&
            token[7] is >= '0' and <= '9')
        {
            return "Numpad " + token[7];
        }

        return token switch
        {
            "ARROW_UP" => "Up Arrow",
            "ARROW_DOWN" => "Down Arrow",
            "ARROW_LEFT" => "Left Arrow",
            "ARROW_RIGHT" => "Right Arrow",
            "NUMPAD_ENTER" => "Numpad Enter",
            "ENTER" => "Enter",
            "SPACE" => "Space",
            _ => throw new InvalidDataException("NES controls contain an unsupported physical key token.")
        };
    }

    public static NesControlCodes Resolve(
        IReadOnlyDictionary<string, string> controls,
        Func<string, ushort> keyCodeResolver)
    {
        ArgumentNullException.ThrowIfNull(controls);
        ArgumentNullException.ThrowIfNull(keyCodeResolver);
        if (controls.Count != Actions.Length || controls.Keys.Any(action => !Actions.Contains(action, StringComparer.Ordinal)))
        {
            throw new InvalidDataException("NES controls must contain exactly the eight supported actions.");
        }

        var nativeNames = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (string action in Actions)
        {
            if (!controls.TryGetValue(action, out string? token))
            {
                throw new InvalidDataException("NES controls are missing a required action.");
            }
            nativeNames.Add(action, ToNativeKeyName(token));
        }

        ushort escapeCode = ResolveCode("Esc", keyCodeResolver);
        ushort fullscreenCode = ResolveCode("F11", keyCodeResolver);
        if (escapeCode == 0 || fullscreenCode == 0 || escapeCode == fullscreenCode)
        {
            throw new InvalidDataException("Mesen could not resolve its reserved Escape and fullscreen keys.");
        }

        var codes = new Dictionary<string, ushort>(StringComparer.Ordinal);
        var usedCodes = new HashSet<ushort>();
        foreach (string action in Actions)
        {
            string nativeName = nativeNames[action];
            ushort code = ResolveCode(nativeName, keyCodeResolver);
            if (code == 0)
            {
                throw new InvalidDataException($"Mesen could not resolve the NES {action} key.");
            }
            if (code == escapeCode || code == fullscreenCode)
            {
                throw new InvalidDataException($"The NES {action} key conflicts with a reserved Mesen shortcut.");
            }
            if (!usedCodes.Add(code))
            {
                throw new InvalidDataException("NES controls resolve to duplicate native keys.");
            }
            codes.Add(action, code);
        }

        return new NesControlCodes(
            codes["UP"], codes["DOWN"], codes["LEFT"], codes["RIGHT"],
            codes["A"], codes["B"], codes["START"], codes["SELECT"]);
    }

    private static ushort ResolveCode(string nativeName, Func<string, ushort> keyCodeResolver)
    {
        try
        {
            return keyCodeResolver(nativeName);
        }
        catch (Exception error)
        {
            throw new InvalidDataException($"Mesen failed to resolve key '{nativeName}'.", error);
        }
    }
}
