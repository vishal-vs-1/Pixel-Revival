"""Fit player artwork to a pose and build a Mesen HD sprite pack."""

from PIL import Image, ImageOps

from .capture import owned_path
from .live_pose import LivePose
from .pose_layouts import _pose_image, offsets_for, pose_key


SCALE = 4


CELL = 8 * SCALE


IMAGE_FORMATS = ('PNG', 'JPEG', 'WEBP', 'BMP', 'GIF', 'TIFF', 'ICO')


def fit_image(path):
    return _fit_canvas(_read_image(path), (2 * CELL, 2 * CELL))


def _read_image(path):
    if path.stat().st_size > 32 * 1024 * 1024:
        raise ValueError('Choose an image smaller than 32 MB.')
    try:
        with Image.open(path, formats=IMAGE_FORMATS) as source:
            if source.width * source.height > 16_000_000:
                raise ValueError('Choose an image with fewer than 16 million pixels.')
            image = ImageOps.exif_transpose(source).convert('RGBA')
    except Image.DecompressionBombError as error:
        raise ValueError('Choose a smaller image.') from error
    if image.getchannel('A').getbbox() is None:
        raise ValueError('The image is fully transparent.')
    return image


def _fit_canvas(image, size):
    image = ImageOps.contain(image.crop(image.getchannel('A').getbbox()), size, Image.Resampling.LANCZOS)
    art = Image.new('RGBA', size)
    art.paste(image, ((art.width - image.width) // 2, art.height - image.height))
    return art


def _fit_pose(artwork, pose, chr_rom, *, artwork_follows_pose=False):
    original = _pose_image(pose, chr_rom)
    canvas = Image.new('RGBA', (original.width * SCALE, original.height * SCALE))
    bounds = original.getchannel('A').getbbox()
    source_bounds = artwork.getchannel('A').getbbox()
    if not bounds or not source_bounds:
        raise ValueError('A player pose and its replacement must both contain visible pixels.')
    left, top, right, bottom = (edge * SCALE for edge in bounds)
    height = bottom - top
    source = artwork.crop(source_bounds)
    # Fill the visible height and keep wide uploads inside the original sprite.
    width = min(right - left, max(1, round(source.width * height / source.height)))
    pixels = source.resize((width, height), Image.Resampling.LANCZOS)
    # The renderer sees the already oriented pose. Only uploaded art needs
    # this extra flip to face the same way as the captured layout.
    if not artwork_follows_pose and all(cell.mirrors & 1 for cell in pose[:2]):
        pixels = ImageOps.mirror(pixels)
    canvas.paste(pixels, (left + (right - left - width) // 2, top))
    return canvas


def write_pack(home, poses, image, overrides=None, *, chr_rom, artwork_follows_pose=False):
    overrides = overrides or {}
    replacements = [(pose, overrides.get(pose_key(pose), image)) for pose in poses]
    replacements = [(pose, artwork) for pose, artwork in replacements
                    if artwork is not None and (not isinstance(pose, LivePose) or pose.replaceable)]
    if not replacements:
        raise ValueError('Assign an image to at least one pose before playing.')
    pack = owned_path(home, 'HdPacks/game')
    pack.mkdir(parents=True, exist_ok=True)
    pages = (len(replacements) + 63) // 64
    definitions = ['<ver>109', f'<scale>{SCALE}', *[f'<img>{_atlas_name(i)}' for i in range(pages)]]
    pitch_x = max([2 * CELL, *(pose.size[0] * SCALE for pose, _ in replacements if isinstance(pose, LivePose))])
    pitch_y = max([4 * CELL, *(pose.size[1] * SCALE for pose, _ in replacements if isinstance(pose, LivePose))])
    atlases = [Image.new('RGBA', (min(8, len(replacements) - page * 64) * pitch_x,
                                 ((min(64, len(replacements) - page * 64) + 7) // 8) * pitch_y))
               for page in range(pages)]
    definitions.extend(f'<condition>y{sign}{offset},positionCheckY,{op},{limit}'
                       for offset in (8, 16, 24) for sign, op, limit in (('lo', '>=', offset), ('hi', '<', 240 - offset)))
    definitions.extend(('<condition>minX,positionCheckX,>=,8', '<condition>maxX,positionCheckX,<,248'))
    # Put full-height rules before any matching four-cell sublayout.
    replacements.sort(key=lambda pair: -len(pair[0]))
    guards = set()
    for index, (pose, artwork) in enumerate(replacements):
        page, slot = divmod(index, 64)
        page_x = (slot % 8) * pitch_x
        page_y = (slot // 8) * pitch_y
        live = isinstance(pose, LivePose)
        width, height = pose.size if live else (16, 4 * len(pose))
        art = _fit_pose(artwork, pose, chr_rom, artwork_follows_pose=artwork_follows_pose)
        offsets = offsets_for(pose)
        for corner, (part, (dx, dy)) in enumerate(zip(pose, offsets)):
            x, y = dx * SCALE, dy * SCALE
            atlases[page].paste(part.orient(art.crop((x, y, x + CELL, y + CELL))), (page_x + x, page_y + y))
            filters = ['hmirror' if part.mirrors & 1 else '!hmirror', 'vmirror' if part.mirrors & 2 else '!vmirror']
            if live:
                for axis, lower, upper in (('X', dx, 256 - width + dx), ('Y', dy, 240 - height + dy)):
                    name = f'live{axis}{lower}_{upper}'
                    if name not in guards:
                        definitions.append(f'<condition>{name}a,positionCheck{axis},>=,{lower}')
                        definitions.append(f'<condition>{name}b,positionCheck{axis},<=,{upper}')
                        guards.add(name)
                    filters.extend((f'{name}a', f'{name}b'))
            else:
                filters.append('minX' if dx else 'maxX')
                if dy:
                    filters.append(f'ylo{dy}')
                if dy < height - 8:
                    filters.append(f'yhi{height - 8 - dy}')
            for neighbor, (other, (nx, ny)) in enumerate(zip(pose, offsets)):
                if neighbor == corner:
                    continue
                name = f'p{index}c{corner}n{neighbor}'
                nx = (nx - dx) * (-1 if part.mirrors & 1 else 1)
                ny = (ny - dy) * (-1 if part.mirrors & 2 else 1)
                token = other.selector if live else f'{other.index:X}'
                definitions.append(f'<condition>{name},spriteNearby,{nx},{ny},{token},{other.palette}')
                filters.append(name)
            token = part.selector if live else f'{part.index:X}'
            definitions.append(f"[{'&'.join(filters)}]<tile>{page},{token},{part.palette},{x + page_x},{y + page_y},1,N")
    for index, atlas in enumerate(atlases):
        atlas.save(owned_path(pack, _atlas_name(index)))
    owned_path(pack, 'hires.txt').write_text('\n'.join(definitions) + '\n', encoding='ascii')


def _atlas_name(index):
    return 'player.png' if index == 0 else f'player-{index}.png'
