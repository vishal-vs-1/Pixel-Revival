/* Launcher-owned comparison input. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.Component;
import java.awt.event.KeyEvent;
import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.function.BiConsumer;
import java.util.HashSet;
import java.util.Set;

public final class ComparisonInput {
    private static final String READY = System.getProperty("jme.comparisonReady");

    private ComparisonInput() { }

    public static boolean enabled() { return READY != null; }

    public static void start(BiConsumer<String, Boolean> deliver) {
        start(deliver, () -> { });
    }

    public static void start(BiConsumer<String, Boolean> deliver, Runnable close) {
        if (!enabled() && !Boolean.getBoolean("jme.launcherControl")) return;
        Thread reader = new Thread(() -> {
            Set<String> held = new HashSet<String>();
            try (BufferedReader lines = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.US_ASCII))) {
                if (READY != null) Files.createFile(Paths.get(READY));
                String line;
                while ((line = lines.readLine()) != null) {
                    if (line.equals("!CLOSE")) break;
                    if (line.length() < 2 || line.length() > 13) continue;
                    char operation = line.charAt(0);
                    String token = line.substring(1);
                    if (operation == '+' && held.add(token)) deliver.accept(token, true);
                    else if (operation == '-' && held.remove(token)) deliver.accept(token, false);
                }
            } catch (Exception failure) {
                System.err.println("Comparison input: " + failure);
            } finally {
                // Losing the launcher's pipe must never leave movement held.
                for (String token : held) deliver.accept(token, false);
                close.run();
            }
        }, "launcher-comparison-input");
        reader.setDaemon(true);
        reader.start();
    }

    public static KeyEvent awtEvent(Component target, String token, boolean pressed) {
        int key;
        int location = KeyEvent.KEY_LOCATION_STANDARD;
        if (token.matches("KEY_[A-Z]")) key = KeyEvent.VK_A + token.charAt(4) - 'A';
        else if (token.matches("DIGIT_[0-9]")) key = KeyEvent.VK_0 + token.charAt(6) - '0';
        else if (token.matches("NUMPAD_[0-9]")) {
            key = KeyEvent.VK_NUMPAD0 + token.charAt(7) - '0';
            location = KeyEvent.KEY_LOCATION_NUMPAD;
        } else {
            switch (token) {
                case "ARROW_UP": key = KeyEvent.VK_UP; break;
                case "ARROW_DOWN": key = KeyEvent.VK_DOWN; break;
                case "ARROW_LEFT": key = KeyEvent.VK_LEFT; break;
                case "ARROW_RIGHT": key = KeyEvent.VK_RIGHT; break;
                case "NUMPAD_ENTER":
                    location = KeyEvent.KEY_LOCATION_NUMPAD;
                    key = KeyEvent.VK_ENTER; break;
                case "ENTER": key = KeyEvent.VK_ENTER; break;
                case "SPACE": key = KeyEvent.VK_SPACE; break;
                default: return null;
            }
        }
        return new KeyEvent(target, pressed ? KeyEvent.KEY_PRESSED : KeyEvent.KEY_RELEASED,
                            System.currentTimeMillis(), 0, key, KeyEvent.CHAR_UNDEFINED, location);
    }
}
