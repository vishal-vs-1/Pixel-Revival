"""Read saved Java ME artwork for the library without touching Tk."""

import json

from PIL import Image, ImageOps


def load_java_preview(library, ref, backend, icon, count_textures, background):
    textures = library.texture_dir(ref, backend)
    count = sum(1 for _ in textures.glob("*.png")) if count_textures else None
    if not icon:
        return None, count
    folder = library.workspace(ref)
    try:
        catalogue = json.loads((folder / "catalogue.json").read_text(encoding="utf-8"))
        record = next(row for row in catalogue if row["resource"].lstrip("/") == icon)
        filename = record["image_id"] + ".png"
        path = textures / filename
        if not path.is_file():
            path = folder / "original-assets" / filename
        with Image.open(path) as image:
            thumbnail = ImageOps.contain(image.convert("RGBA"), (88, 88), Image.Resampling.LANCZOS)
    except (OSError, ValueError, TypeError, AttributeError, StopIteration, KeyError,
            Image.DecompressionBombError):
        return None, count

    plate = Image.new("RGBA", (96, 96), background)
    plate.alpha_composite(thumbnail, ((96 - thumbnail.width) // 2, (96 - thumbnail.height) // 2))
    return plate, count
