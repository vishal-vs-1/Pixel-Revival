"""Decode raw 8-bit palette indices from an explicitly supplied archive layout."""
from dataclasses import dataclass
import hashlib
import json

from PIL import Image

from systems.jme.resources.decoding import MAX_PIXELS

MAX_LAYOUT_BYTES = 64 * 1024
MAX_TOTAL_PIXELS = 16 * 1024 * 1024
MAX_FRAMES = 1024


@dataclass(frozen=True)
class IndexedResource:
    resource: str
    palette: str
    width: int
    height: int
    frames: int = 1
    transparent_index: int | None = None

    def __post_init__(self):
        if any(type(name) is not str or not name for name in (self.resource, self.palette)):
            raise ValueError('Indexed resources require archive names for pixels and palette')
        if any(type(count) is not int or count <= 0 for count in (self.width, self.height, self.frames)):
            raise ValueError('Indexed dimensions and frame count must be positive integers')
        if self.width * self.height > MAX_PIXELS or self.frames > MAX_FRAMES:
            raise ValueError('Indexed dimensions or frame count exceed the decoder limit')
        index = self.transparent_index
        if index is not None and (type(index) is not int or not 0 <= index <= 255):
            raise ValueError('Transparent index must be between 0 and 255')

    def images(self, indices, palette):
        colors, remainder = divmod(len(palette), 3)
        if remainder or not 1 <= colors <= 256:
            raise ValueError('Palette must contain between 1 and 256 RGB triples')
        area = self.width * self.height
        if len(indices) != area * self.frames:
            raise ValueError('Indexed payload length does not match the declared frames')
        if max(indices) >= colors or (self.transparent_index is not None and self.transparent_index >= colors):
            raise ValueError('An indexed pixel or transparent index is outside the palette')

        for offset in range(0, len(indices), area):
            image = Image.frombytes('P', (self.width, self.height), indices[offset:offset + area])
            image.putpalette(palette)
            if self.transparent_index is not None:
                image.info['transparency'] = self.transparent_index
            yield image


class IndexedImages:
    name = 'indexed-rgb8-v1'

    def __init__(self, layout):
        if len(layout) > MAX_LAYOUT_BYTES:
            raise ValueError('Indexed layout exceeds the decoder limit')
        entries = json.loads(layout)
        if not isinstance(entries, list) or not entries or any(not isinstance(row, dict) for row in entries):
            raise ValueError('Indexed layout requires a non-empty array of resource descriptions')
        try:
            self.resources = tuple(IndexedResource(**entry) for entry in entries)
        except TypeError as error:
            raise ValueError('Invalid indexed resource fields') from error
        if len({item.resource for item in self.resources}) != len(self.resources):
            raise ValueError('Indexed resources must be unique in the layout')
        if sum(item.width * item.height * item.frames for item in self.resources) > MAX_TOTAL_PIXELS:
            raise ValueError('Indexed layout exceeds the total pixel limit')
        self.cache_key = f'{self.name}:{hashlib.sha256(layout).hexdigest()}'

    def supports(self, context):
        return True

    def decode(self, context):
        archive_entries = {entry.filename: entry for entry in context.resources()}
        for item in self.resources:
            try:
                pixels = context.archive.read(archive_entries[item.resource])
                palette = context.archive.read(archive_entries[item.palette])
                for frame, image in enumerate(item.images(pixels, palette)):
                    context.sink.add(image, item.resource, self.name, frame=frame,
                                     palette=item.palette, transparent_index=item.transparent_index)
            except KeyError as error:
                raise ValueError(f'Indexed resource missing or oversized: {error.args[0]}') from error
            except ValueError as error:
                raise ValueError(f'{item.resource}: {error}') from error
            context.recovered.update((item.resource, item.palette))
