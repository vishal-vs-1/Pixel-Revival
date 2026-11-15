"""Deterministic pixel artwork scaling shared by the game systems."""

import numpy as np
from PIL import Image


def scale2x(image: Image.Image) -> Image.Image:
    """Eric Johnston/Scale2x edge-aware rule, applied to complete RGBA colours."""
    a = np.array(image.convert('RGBA'), dtype=np.uint8)
    a[a[:, :, 3] == 0] = 0
    p = np.pad(a, ((1, 1), (1, 1), (0, 0)), mode='edge')
    b, d, e, f, h = p[:-2, 1:-1], p[1:-1, :-2], a, p[1:-1, 2:], p[2:, 1:-1]
    eq = lambda x, y: np.all(x == y, axis=2)
    active = ~eq(b, h) & ~eq(d, f)
    result = np.empty((a.shape[0] * 2, a.shape[1] * 2, 4), np.uint8)
    for row, col, x, y in [(0, 0, d, b), (0, 1, b, f), (1, 0, d, h), (1, 1, h, f)]:
        result[row::2, col::2] = np.where((active & eq(x, y))[:, :, None], x, e)
    return Image.fromarray(result)


def enhance(image: Image.Image) -> Image.Image:
    return scale2x(scale2x(image))
