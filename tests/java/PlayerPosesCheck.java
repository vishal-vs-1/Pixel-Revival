package org.recompile.mobile;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import javax.microedition.lcdui.Image;
import javax.microedition.lcdui.game.GameCanvas;

public final class PlayerPosesCheck {
    public static void main(String[] args) throws Exception {
        Path root = Paths.get(System.getProperty("jme.playerPoses"));
        int[] pixels = new int[16 * 8];
        for (int i = 0; i < pixels.length; i++) pixels[i] = i % 16 < 8 ? 0xffff0000 : 0xff00ff00;
        Image sheet = Image.createRGBImage(pixels, 16, 8, true);
        String left = Enhancement.hash(sheet.getCanvas().getSubimage(0, 0, 8, 8));
        String right = Enhancement.hash(sheet.getCanvas().getSubimage(8, 0, 8, 8));
        if (args[0].equals("record")) {
            MobilePlatform.keyState = GameCanvas.RIGHT_PRESSED;
            for (int frame = 0; frame < 6; frame++) {
                PlayerPoses.source(sheet, 0, 0, 8, 8, frame * 2, 20);
                PlayerPoses.source(sheet, 8, 0, 8, 8, 42, 20);
                RuntimeAssets.endFrame();
            }
            if (!Files.isRegularFile(root.resolve("poses").resolve(left + ".png"))
                    || !Files.isRegularFile(root.resolve("poses").resolve(right + ".png"))
                    || !Files.isRegularFile(root.resolve("suggested").resolve(left))
                    || Files.exists(root.resolve("suggested").resolve(right)))
                throw new AssertionError("Expected two independent regions and only one motion suggestion");
        } else {
            Image leftImage = PlayerPoses.source(sheet, 0, 0, 8, 8, 15, 20);
            Image rightImage = PlayerPoses.source(sheet, 8, 0, 8, 8, 42, 20);
            boolean legacy = args[0].equals("legacy");
            if (leftImage.getCanvas().getRGB(4, 4) != 0xffff00ff
                    || (legacy ? rightImage != null : rightImage.getCanvas().getRGB(4, 4) != 0xff0000ff))
                throw new AssertionError("Unexpected default or per-pose image");
        }
        PlatformFont.setScreenSize(240, 320);
        PlatformImage target = new PlatformImage(64, 64, 0xff202020);
        PlatformGraphics graphics = target.getMIDPGraphics();
        int offset = args[0].equals("record") ? 8 : 20;
        for (int i = 0; i < 10; i++) {
            graphics.setColor(0xff0000 + i);
            graphics.fillArc(offset + i, 15 + i, 20 - 2 * i, 24 - 2 * i, 0, 360);
        }
        graphics.fillRect(0, 0, 1, 1);
        Path marker;
        try (java.util.stream.Stream<Path> markers = Files.list(root.resolve("drawn"))) {
            Path[] keys = markers.toArray(Path[]::new);
            if (keys.length != 1) throw new AssertionError("Expected one drawn pose key");
            marker = keys[0];
        }
        if (!args[0].equals("record")) {
            java.awt.image.BufferedImage preview = javax.imageio.ImageIO.read(
                    root.resolve("poses").resolve(marker.getFileName() + ".png").toFile());
            int top = preview.getHeight(), bottom = 0;
            for (int y = 0; y < preview.getHeight(); y++) for (int x = 0; x < preview.getWidth(); x++) {
                if ((preview.getRGB(x, y) >>> 24) != 0) {
                    top = Math.min(y, top);
                    bottom = Math.max(y + 1, bottom);
                }
            }
            for (int y = 15 + top; y < 15 + bottom; y++)
                if (target.getCanvas().getRGB(30, y) != 0xffff00ff)
                    throw new AssertionError("Replacement does not match the visible pose height at row " + y);
            if (target.getCanvas().getRGB(30, 14 + top) != 0xff202020
                    || target.getCanvas().getRGB(30, 15 + bottom) != 0xff202020)
                throw new AssertionError("Replacement spilled outside the original draw height");
        }
    }
}
