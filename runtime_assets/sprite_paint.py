"""Offline, palette-based drawing of one transparent pixel-art pose."""

import argparse
from pathlib import Path

import numpy as np
from PIL import Image


SAMPLES = 16
HALF = SAMPLES // 2
MAX_SIDE = 128
STYLES = ("ink", "classic", "fill")


def _diagonal_fill(labels, x, y):
    # A junction belongs to four pixels. A repeated diagonal can continue
    # through it, while a three-to-one corner gets a single beveled corner.
    nw, ne = labels[y - 1, x - 1:x + 1]
    sw, se = labels[y, x - 1:x + 1]
    if nw == se and ne == sw:
        if nw == ne:
            return None
        # In a checkerboard, favor the diagonal supported by more neighbors.
        # Keep a tie square rather than guessing between two equal features.
        nw_se = np.count_nonzero(
            np.array([labels[y - 2, x - 1], labels[y - 1, x - 2],
                      labels[y + 1, x], labels[y, x + 1]]) == nw
        )
        ne_sw = np.count_nonzero(
            np.array([labels[y - 2, x], labels[y - 1, x + 1],
                      labels[y + 1, x - 1], labels[y, x - 2]]) == ne
        )
        if nw_se == ne_sw:
            return None
        return nw if nw_se > ne_sw else ne
    if nw == se:
        return nw
    if ne == sw:
        return ne
    return None


def _paint_geometry(source, *, repeat_edges=False):
    # Reserve label zero for transparency regardless of the hidden RGB bytes.
    pixels = np.array(source.convert("RGBA"))
    pixels[pixels[:, :, 3] == 0] = 0
    palette, inverse = np.unique(
        pixels.reshape(-1, 4), axis=0, return_inverse=True
    )
    if palette[0, 3] != 0:
        palette = np.vstack((np.zeros((1, 4), dtype=np.uint8), palette))
        inverse += 1
    # Two bytes per label suffice for every pixel in a 128x128 source.
    labels = np.pad(inverse.reshape(source.height, source.width).astype(np.uint16), 2,
                    mode="edge" if repeat_edges else "constant")
    detail = labels.repeat(SAMPLES, axis=0).repeat(SAMPLES, axis=1)

    # One fill owns every point, so neighboring shapes share the same edge.
    # A half-pixel bevel leaves the middle of every original pixel untouched.
    axis = np.arange(-HALF, HALF, dtype=float) + 0.5
    triangle = np.abs(axis[:, None]) + np.abs(axis[None, :]) <= HALF
    for y in range(2, source.height + 3):
        for x in range(2, source.width + 3):
            fill = _diagonal_fill(labels, x, y)
            if fill is not None:
                x0, y0 = x * SAMPLES - HALF, y * SAMPLES - HALF
                corner = detail[y0:y0 + SAMPLES, x0:x0 + SAMPLES]
                corner[triangle] = fill
    return detail, palette, np.bincount(inverse, minlength=len(palette))


def _outer_ink(visible, width):
    outline = np.zeros(visible.shape, dtype=bool)
    for shift in ((0, width), (0, -width), (width, 0), (-width, 0)):
        outline |= ~np.roll(visible, shift, axis=(0, 1))
    return outline & visible


def _inner_ink(detail, palette, color_counts):
    # Ink both sides of a shared edge, but only between distinct, established
    # fills. Near-identical colors and tiny accents keep their own edges.
    established = (palette[:, 3] > 0) & (color_counts >= 3)
    visible_fills = established[detail]
    ink = np.zeros(detail.shape, dtype=bool)
    rgb = palette[:, :3].astype(np.int16)
    chroma = rgb - rgb.mean(axis=1, keepdims=True)
    direction = chroma / np.maximum(np.linalg.norm(chroma, axis=1, keepdims=True), 1)
    neutral = np.ptp(rgb, axis=1) < 24
    for axis in (0, 1):
        neighbor = np.roll(detail, 2, axis=axis)
        y, x = np.nonzero(visible_fills & established[neighbor] & (detail != neighbor))
        near, far = detail[y, x], neighbor[y, x]
        delta = np.max(np.abs(rgb[near] - rgb[far]), axis=1)
        hue_agreement = np.sum(direction[near] * direction[far], axis=1)
        # Skip ordinary shadows within a color. Keep strong value changes,
        # such as a colored fill next to black or white.
        different_fill = neutral[near] | neutral[far] | (hue_agreement < 0.974) | (delta >= 128)
        distinct = (delta >= 55) & different_fill
        y, x = y[distinct], x[distinct]
        ink[y, x] = True
        if axis == 0:
            ink[y - 2, x] = True
        else:
            ink[y, x - 2] = True
    return ink


def _paint_colors(detail, palette, color_counts, style):
    art = palette[detail]
    if style == "fill":
        return art
    visible = art[:, :, 3] > 0
    broad_fill = visible & (color_counts >= 6)[detail]
    fills = palette[:, :3].astype(np.float32)
    if style == "ink":
        # Push color and value apart while keeping the original hue as the guide.
        value = (fills @ np.array([0.2126, 0.7152, 0.0722]))[:, None]
        fills = np.clip(fills + 0.18 * (fills - value) + 0.10 * (value - 110), 0, 255)
        light_shift, shadow_shift = (5, 4), (-7, -5)
        light, shade = 0.23, 0.30
    else:
        light_shift, shadow_shift = (4, 3), (-6, -4)
        light, shade = 0.20, 0.25
    rgb = fills.astype(np.float32)[detail]

    # Local edge light follows the geometry of broad fills. Confine it to
    # one side of each shape instead of laying bands across the whole pose.
    upper_left = np.roll(detail, light_shift, axis=(0, 1))
    lower_right = np.roll(detail, shadow_shift, axis=(0, 1))
    highlights = broad_fill & (upper_left != detail)
    shadows = broad_fill & ~highlights & (lower_right != detail)
    rgb[highlights] = rgb[highlights] * (1 - light) + np.array([255, 230, 177]) * light
    rgb[shadows] = rgb[shadows] * (1 - shade) + np.array([42, 28, 68]) * shade

    # Keep the ink inside the silhouette to preserve the pose's footprint.
    if style == "ink":
        outline = _outer_ink(visible, 4)
        seams = _inner_ink(detail, palette, color_counts)
        rgb[seams] = (8, 7, 10)
        rgb[outline] = 0
        rgb[~visible] = 0
    else:
        outline = _outer_ink(visible, 2)
        rgb[outline] = rgb[outline] * 0.35 + np.array([29, 23, 37]) * 0.65
    art[:, :, :3] = np.rint(rgb).astype(np.uint8)
    return art


def paint_pose(source: Image.Image, *, scale: int = 4, style: str = "ink") -> Image.Image:
    """Draw an RGBA pixel sprite without a model or character template."""
    if max(source.size) > MAX_SIDE:
        raise ValueError("Use a native-resolution sprite up to 128 pixels per side.")
    if type(scale) is not int or not 2 <= scale <= 8:
        raise ValueError("Artwork scale must be between 2 and 8.")
    if style not in STYLES:
        raise ValueError(f"Choose one of these paint styles: {', '.join(STYLES)}.")
    detail, palette, counts = _paint_geometry(source, repeat_edges=style == "fill")
    art = _paint_colors(detail, palette, counts, style)
    pad = 2 * SAMPLES
    art = art[pad:pad + source.height * SAMPLES, pad:pad + source.width * SAMPLES]
    return Image.fromarray(art).resize(
        (source.width * scale, source.height * scale), Image.Resampling.BOX
    )


def redraw_asset(source: Image.Image) -> Image.Image:
    """Trace an asset at 4x, bounding the supersampled working image."""
    scale = 4
    halo = 2
    step = MAX_SIDE - 2 * halo
    result = Image.new("RGBA", (source.width * scale, source.height * scale))
    for y in range(0, source.height, step):
        for x in range(0, source.width, step):
            right = min(x + step, source.width)
            bottom = min(y + step, source.height)
            left_halo, top_halo = max(0, x - halo), max(0, y - halo)
            # Keep the neighboring pixels needed by the tracer at patch edges.
            patch = source.crop((left_halo, top_halo, min(source.width, right + halo),
                                 min(source.height, bottom + halo)))
            painted = paint_pose(patch, scale=scale, style="fill")
            box = (x - left_halo, y - top_halo, right - left_halo, bottom - top_halo)
            result.paste(painted.crop(tuple(value * scale for value in box)), (x * scale, y * scale))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Native-resolution transparent sprite PNG")
    parser.add_argument("output", type=Path, help="Painted sprite PNG")
    parser.add_argument("--scale", type=int, default=4, choices=range(2, 9))
    parser.add_argument("--style", choices=STYLES, default="ink")
    args = parser.parse_args()
    if args.source.resolve() == args.output.resolve():
        parser.error("Choose an output path different from the source.")
    with Image.open(args.source, formats=["PNG"]) as source:
        paint_pose(source, scale=args.scale, style=args.style).save(args.output, format="PNG", optimize=True)


if __name__ == "__main__":
    main()
