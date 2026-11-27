/* JAR Revival Java ME sprite artwork. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.AlphaComposite;
import java.awt.Graphics2D;
import java.awt.RenderingHints;
import java.awt.image.BufferedImage;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.WeakHashMap;
import javax.imageio.ImageIO;
import javax.imageio.ImageReader;
import javax.imageio.stream.ImageInputStream;
import javax.microedition.lcdui.Image;
import javax.microedition.lcdui.game.GameCanvas;

/** Opt-in MIDP image and sprite-sheet region replacement at the draw boundary. */
public final class PlayerPoses {
    public static final boolean ENABLED = System.getProperty("jme.playerPoses") != null;
    static final boolean RECORD = Boolean.getBoolean("jme.recordPoses");
    private static final boolean REPLACE = Boolean.parseBoolean(System.getProperty("jme.replacePoses", "true"));
    private static final int LIMIT = 192;
    private static final int DIRECTIONS = GameCanvas.LEFT_PRESSED | GameCanvas.RIGHT_PRESSED
            | GameCanvas.UP_PRESSED | GameCanvas.DOWN_PRESSED;
    private static final Map<Image, Map<String, Pose>> SOURCES = new WeakHashMap<>();
    private static final Set<String> CAPTURED = new HashSet<>();
    private static final Map<String, Image> REPLACEMENTS = new HashMap<>();
    private static boolean loaded;
    private static BufferedImage fallback;
    private static final Path ROOT = ENABLED ? Paths.get(System.getProperty("jme.playerPoses")) : null;
    private static final Path CAPTURE_ROOT = ENABLED
            ? Paths.get(System.getProperty("jme.poseCaptureRoot", ROOT.toString())) : null;
    private static final class Pose {
        String key;
        Image replacement;
        long frame = -2;
        int x, y, direction, matches;
    }

    private PlayerPoses() { }

    static String hex(byte[] bytes) {
        StringBuilder result = new StringBuilder(bytes.length * 2);
        for (byte value : bytes) {
            result.append(Character.forDigit((value & 255) >>> 4, 16));
            result.append(Character.forDigit(value & 15, 16));
        }
        return result.toString();
    }

    public static synchronized Image source(Image image, int x, int y, int w, int h, int drawX, int drawY) {
        if (image == null || image.isMutable() || w < 4 || h < 4 || w > 128 || h > 128
                || x < 0 || y < 0 || x > image.getWidth() - w || y > image.getHeight() - h) return null;
        String region = x + "," + y + "," + w + "," + h;
        Map<String, Pose> regions = SOURCES.computeIfAbsent(image, unused -> new HashMap<>());
        Pose previous = regions.get(region);
        if (previous != null) {
            if (RECORD && previous.key != null) observeMovement(previous, drawX, drawY);
            return previous.replacement;
        }
        if (regions.size() >= 512) return null;
        Pose pose = new Pose();
        Image replacement = null;
        try {
            BufferedImage crop = image.getCanvas().getSubimage(x, y, w, h);
            pose.key = Enhancement.hash(crop);
            replacement = resolve(pose.key, w, h, crop, false);
        } catch (Exception failure) {
            System.err.println("Player pose artwork: " + failure);
        }
        pose.replacement = replacement;
        regions.put(region, pose);
        return replacement;
    }

    static synchronized Image resolve(String key, int w, int h, BufferedImage crop, boolean drawn) {
        try {
            Path capture = CAPTURE_ROOT.resolve("poses").resolve(key + ".png");
            if (RECORD && crop != null && !CAPTURED.contains(key) && CAPTURED.size() < LIMIT) {
                if (!loaded) {
                    try (java.util.stream.Stream<Path> paths = Files.list(capture.getParent())) {
                        paths.map(p -> p.getFileName().toString())
                                .filter(n -> n.matches("[0-9a-f]{64}\\.png")).limit(LIMIT)
                                .forEach(n -> CAPTURED.add(n.substring(0, 64)));
                    }
                    loaded = true;
                }
                if (CAPTURED.size() < LIMIT && !CAPTURED.contains(key)) {
                    Path temp = Files.createTempFile(capture.getParent(), "pose-", ".tmp");
                    try {
                        if (ImageIO.write(crop, "PNG", temp.toFile())) {
                            Files.move(temp, capture, StandardCopyOption.REPLACE_EXISTING);
                            if (drawn) Files.write(CAPTURE_ROOT.resolve("drawn").resolve(key), new byte[0]);
                            CAPTURED.add(key);
                        }
                    } finally { Files.deleteIfExists(temp); }
                }
            }
            // Original mode still records poses without applying saved artwork.
            if (!REPLACE) return null;
            if (REPLACEMENTS.containsKey(key)) return REPLACEMENTS.get(key);
            if (REPLACEMENTS.size() < 512) {
                Path override = ROOT.resolve("images").resolve(key + ".png");
                boolean selected = Files.isRegularFile(ROOT.resolve("selected").resolve(key), LinkOption.NOFOLLOW_LINKS);
                BufferedImage art = readImage(override);
                // Automatic capture includes terrain and effects. Only explicit links use the default.
                if (art == null && selected) {
                    if (fallback == null) fallback = readImage(ROOT.resolve("images/default.png"));
                    art = fallback;
                }
                BufferedImage original = crop;
                if (art != null && drawn && original == null
                        && Files.isRegularFile(capture, LinkOption.NOFOLLOW_LINKS) && Files.size(capture) <= 131_072)
                    original = ImageIO.read(capture.toFile());
                Image replacement = art == null ? null : fit(art, w, h, original);
                if (replacement != null) REPLACEMENTS.put(key, replacement);
                return replacement;
            }
        } catch (Exception failure) {
            System.err.println("Player pose artwork: " + failure);
        }
        return null;
    }

    private static BufferedImage readImage(Path path) throws java.io.IOException {
        if (!Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) || Files.size(path) > 524_288) return null;
        ImageReader png = ImageIO.getImageReadersByFormatName("PNG").next();
        try (ImageInputStream stream = ImageIO.createImageInputStream(path.toFile())) {
            png.setInput(stream);
            return png.getWidth(0) <= 256 && png.getHeight(0) <= 256 ? png.read(0) : null;
        } finally { png.dispose(); }
    }

    private static void observeMovement(Pose pose, int x, int y) {
        if (pose.matches >= 3 || !CAPTURED.contains(pose.key)) return;
        long frame = RuntimeAssets.completedFrameCount();
        if (frame == pose.frame) return;
        int direction = MobilePlatform.keyState & DIRECTIONS;
        int dx = x - pose.x, dy = y - pose.y;
        if (frame == pose.frame + 1 && direction == pose.direction
                && Math.abs(dx) <= 16 && Math.abs(dy) <= 16
                && (direction == GameCanvas.LEFT_PRESSED && dx < 0
                    || direction == GameCanvas.RIGHT_PRESSED && dx > 0
                    || direction == GameCanvas.UP_PRESSED && dy < 0
                    || direction == GameCanvas.DOWN_PRESSED && dy > 0)
                && ++pose.matches == 3) {
            try {
                Path marker = CAPTURE_ROOT.resolve("suggested").resolve(pose.key);
                if (!Files.exists(marker)) Files.createFile(marker);
            } catch (java.io.IOException failure) {
                System.err.println("Player pose suggestion: " + failure);
            }
        }
        pose.frame = frame;
        pose.direction = direction;
        pose.x = x;
        pose.y = y;
    }

    private static java.awt.Rectangle visibleBounds(BufferedImage art) {
        int left = art.getWidth(), top = art.getHeight(), right = 0, bottom = 0;
        for (int y = 0; y < art.getHeight(); y++) for (int x = 0; x < art.getWidth(); x++) {
            if ((art.getRGB(x, y) >>> 24) != 0) {
                left = Math.min(left, x); top = Math.min(top, y);
                right = Math.max(right, x + 1); bottom = Math.max(bottom, y + 1);
            }
        }
        return right <= left || bottom <= top ? new java.awt.Rectangle()
                : new java.awt.Rectangle(left, top, right - left, bottom - top);
    }

    private static Image fit(BufferedImage art, int w, int h, BufferedImage original) {
        java.awt.Rectangle shape = visibleBounds(art);
        if (shape.isEmpty()) return null;
        java.awt.Rectangle destination = original != null && original.getWidth() == w && original.getHeight() == h
                ? visibleBounds(original) : new java.awt.Rectangle(0, 0, w, h);
        if (destination.isEmpty()) return null;
        int width = Math.min(destination.width,
                Math.max(1, (int) Math.round((double) shape.width * destination.height / shape.height)));
        BufferedImage canvas = new BufferedImage(w, h, BufferedImage.TYPE_INT_ARGB);
        Graphics2D graphics = canvas.createGraphics();
        try {
            graphics.setComposite(AlphaComposite.Src);
            graphics.setRenderingHint(RenderingHints.KEY_INTERPOLATION, RenderingHints.VALUE_INTERPOLATION_BILINEAR);
            int x = destination.x + (destination.width - width) / 2;
            graphics.drawImage(art, x, destination.y, x + width, destination.y + destination.height,
                    shape.x, shape.y, shape.x + shape.width, shape.y + shape.height, null);
        } finally { graphics.dispose(); }
        return Image.createRGBImage(canvas.getRGB(0, 0, w, h, null, 0, w), w, h, true);
    }
}
