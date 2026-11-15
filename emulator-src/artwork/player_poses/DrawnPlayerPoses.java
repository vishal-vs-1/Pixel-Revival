/* JAR Revival procedural sprite capture. GPL-3.0-or-later. */
package org.recompile.mobile;

import java.awt.Color;
import java.awt.Graphics2D;
import java.awt.Rectangle;
import java.awt.image.BufferedImage;
import java.nio.ByteBuffer;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.Map;
import javax.microedition.lcdui.Image;

/** Collects consecutive, overlapping filled arcs on one graphics surface. */
final class DrawnPlayerPoses {
    private static final int MAX_ARCS = 64;
    private static final int MAX_SIZE = 96;
    private final ArrayList<int[]> arcs = new ArrayList<>();
    private final Map<String, Image> matches = new LinkedHashMap<>();
    private Rectangle bounds;

    interface DrawArc { void draw(int[] arc); }
    interface DrawImage { void draw(Image image, int x, int y); }

    boolean add(int x, int y, int w, int h, int start, int sweep, int color) {
        if (w <= 0 || h <= 0 || w > MAX_SIZE || h > MAX_SIZE || sweep == 0
                || arcs.size() >= MAX_ARCS) return false;
        Rectangle box = new Rectangle(x, y, w, h);
        Rectangle combined = bounds == null ? box : bounds.union(box);
        if (bounds != null && (!bounds.intersects(box) || combined.width > MAX_SIZE
                || combined.height > MAX_SIZE)) return false;
        bounds = combined;
        arcs.add(new int[]{x, y, w, h, start, sweep, color});
        return true;
    }

    void flush(DrawArc original, DrawImage replacement) {
        if (arcs.isEmpty()) return;
        Image image = null;
        Rectangle area = bounds;
        if (arcs.size() >= 8 && area.width >= 8 && area.height >= 8) image = resolve(area);
        bounds = null;
        try {
            if (image != null) replacement.draw(image, area.x, area.y);
            else for (int[] arc : arcs) original.draw(arc);
        } finally { arcs.clear(); }
    }

    private Image resolve(Rectangle box) {
        try {
            ByteBuffer data = ByteBuffer.allocate(4 + arcs.size() * 28);
            data.putInt(0x41524331);
            for (int[] arc : arcs) {
                data.putInt(arc[0] - box.x).putInt(arc[1] - box.y);
                for (int i = 2; i < arc.length; i++) data.putInt(arc[i]);
            }
            String key = PlayerPoses.hex(MessageDigest.getInstance("SHA-256").digest(data.array()));
            if (matches.containsKey(key)) return matches.get(key);
            if (matches.size() >= 256) return null;
            BufferedImage preview = null;
            if (PlayerPoses.RECORD) {
                preview = new BufferedImage(box.width, box.height, BufferedImage.TYPE_INT_ARGB);
                Graphics2D graphics = preview.createGraphics();
                try {
                    for (int[] arc : arcs) {
                        graphics.setColor(new Color(arc[6], true));
                        graphics.fillArc(arc[0] - box.x, arc[1] - box.y, arc[2], arc[3], arc[4], arc[5]);
                    }
                } finally { graphics.dispose(); }
            }
            Image image = PlayerPoses.resolve(key, box.width, box.height, preview, true);
            if (Boolean.getBoolean("jme.poseTrace"))
                System.out.println("Drawn pose " + key + " " + arcs.size() + " " + box
                        + " replacement=" + (image != null));
            matches.put(key, image);
            return image;
        } catch (Exception failure) {
            System.err.println("Drawn player pose: " + failure);
            return null;
        }
    }
}
