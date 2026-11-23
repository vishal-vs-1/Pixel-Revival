"""Import decoding contracts. Decoders produce source images; enhancement is a later stage."""
from __future__ import annotations

from dataclasses import dataclass, field
import binascii
import hashlib
import io
import json
import os
from pathlib import Path
import struct
from typing import Protocol
import zipfile
from PIL import Image

from systems.jme.resources.gameloft_artwork import GameloftSpriteImages
from systems.jme.resources.png_header import HEADER_BYTES, has_swapped_png_header, restore_png_header

VERSION = 3
MAX_PIXELS = 4_194_304
MAX_RESOURCE = 16 * 1024 * 1024


class AssetSink:
    def __init__(self, folder, identity, fresh_sources):
        self.folder = folder / 'original-assets'
        self.folder.mkdir(parents=True, exist_ok=True)
        self.identity = identity
        self.records = []
        self.fresh_sources = fresh_sources
        self.saved_ids = set()

    def add(self, image, resource, decoder, **metadata):
        if image.width * image.height > MAX_PIXELS:
            raise ValueError('Decoded asset dimensions exceed the limit')
        image = image.convert('RGBA')
        key = self.identity(image)
        path = self.folder / (key + '.png')
        if key not in self.saved_ids and not path.exists():
            temporary = path.with_suffix('.tmp')
            try:
                image.save(temporary, format='PNG', compress_level=1)
                os.replace(temporary, path)
            finally:
                temporary.unlink(missing_ok=True)
            if self.fresh_sources is not None:
                self.fresh_sources[key] = image.size
        self.saved_ids.add(key)
        self.records.append(dict(resource=resource, image_id=key, width=image.width,
                                 height=image.height, decoder=decoder, **metadata))
        return key


@dataclass
class DecodeContext:
    jar: Path
    folder: Path
    archive: zipfile.ZipFile
    sink: AssetSink
    java: str
    player: Path | None
    progress: object
    direct: set = field(default_factory=set)
    recovered: set = field(default_factory=set)
    blocks: list = field(default_factory=list)
    containers: list = field(default_factory=list)
    features: dict = field(default_factory=dict)
    outcomes: dict = field(default_factory=dict)

    def resources(self):
        return [entry for entry in self.archive.infolist() if not entry.is_dir()
                and not entry.filename.endswith('.class')
                and not entry.filename.upper().startswith('META-INF/')
                and entry.file_size <= MAX_RESOURCE]


class AssetDecoder(Protocol):
    name: str
    def supports(self, context: DecodeContext) -> bool: ...
    def decode(self, context: DecodeContext) -> None: ...


class DecoderRegistry:
    def __init__(self, decoders):
        self.decoders = tuple(decoders)

    def decode(self, context):
        selected = []
        for decoder in self.decoders:
            if decoder.supports(context):
                before = len(context.sink.records)
                decoder.decode(context)
                selected.append(decoder.name)
                context.outcomes[decoder.name] = len(context.sink.records) - before
        return selected


def read_image(data):
    try:
        with Image.open(io.BytesIO(data)) as image:
            if image.width * image.height > MAX_PIXELS:
                return None
            return image.convert('RGBA')
    except (OSError, ValueError, Image.DecompressionBombError):
        return None


class StandardImages:
    name = 'standard-images-v1'
    def supports(self, context): return True
    def decode(self, context):
        for entry in context.resources():
            image = read_image(context.archive.read(entry))
            if image is not None:
                context.sink.add(image, entry.filename, self.name)
                context.direct.add(entry.filename)


def embedded_pngs(data):
    """Recognize complete PNG streams using chunk boundaries and CRCs, not signatures alone."""
    signature = b'\x89PNG\r\n\x1a\n'
    position = 0
    while True:
        start = data.find(signature, position)
        if start < 0: return
        position = start + 8
        cursor = position
        while cursor + 12 <= len(data):
            size = struct.unpack_from('>I', data, cursor)[0]
            end = cursor + size + 12
            if size > MAX_RESOURCE or end > len(data): break
            chunk = data[cursor + 4:cursor + 8]
            expected = struct.unpack_from('>I', data, end - 4)[0]
            if binascii.crc32(data[cursor + 4:end - 4]) & 0xffffffff != expected: break
            cursor = end
            if chunk == b'IEND':
                image = read_image(data[start:cursor])
                if image is not None: yield start, image
                position = cursor
                break


class EmbeddedImages:
    name = 'embedded-png-v1'
    def supports(self, context): return True
    def decode(self, context):
        for entry in context.resources():
            if entry.filename in context.direct: continue
            for offset, image in embedded_pngs(context.archive.read(entry)):
                context.sink.add(image, entry.filename + '@' + str(offset), self.name)


class SwappedHeaderImages:
    name = 'swapped-png-header-v1'

    def supports(self, context):
        return True

    def decode(self, context):
        for entry in context.resources():
            if entry.filename in context.direct:
                continue
            with context.archive.open(entry) as resource:
                header = resource.read(HEADER_BYTES)
                if not has_swapped_png_header(header):
                    continue
                restored = restore_png_header(header + resource.read())
            candidate = next(embedded_pngs(restored), None)
            if candidate is not None and candidate[0] == 0:
                context.sink.add(candidate[1], entry.filename, self.name)
                context.recovered.add(entry.filename)


class GameloftContainers:
    name = 'gameloft-offset-lzma-v1'
    def supports(self, context):
        from systems.jme.resources.gameloft import layout
        for entry in context.resources():
            if entry.filename in context.direct: continue
            parts = layout(entry.filename, context.archive.read)
            if parts: context.containers.append((entry.filename, parts))
        return bool(context.containers)

    def decode(self, context):
        from systems.jme.resources.gameloft import unpack
        folder = context.folder / 'static-assets' / 'blocks'
        folder.mkdir(parents=True, exist_ok=True)
        total = 0
        for resource, parts in context.containers:
            for index, content_type, data in unpack(parts, context.archive.read):
                total += len(data)
                if total > 128 * 1024 * 1024: raise ValueError('Decoded resources exceed the import limit')
                name = hashlib.sha256(resource.encode()).hexdigest()[:12] + '-' + str(index) + '.bin'
                path = folder / name
                path.write_bytes(data)
                context.blocks.append(dict(resource=resource, index=index, path=path,
                                           content_type=content_type, size=len(data)))
                image = read_image(data)
                if image is not None:
                    context.sink.add(image, f'{resource}#{index}', self.name)
        context.progress(f'Unpacked {len(context.blocks)} blocks from {len(context.containers)} recognized containers.')


def default_registry():
    return DecoderRegistry([StandardImages(), EmbeddedImages(), GameloftContainers(),
                            SwappedHeaderImages(), GameloftSpriteImages()])


def decode_jar(jar, folder, identity, java=None, player=None, progress=lambda message: None, registry=None,
               *, fresh_sources=None):
    """The persistent source preparation stage shared by Add and Prepare."""
    jar, folder = Path(jar).resolve(), Path(folder).resolve()
    active_registry=registry or default_registry()
    registry_names=[getattr(decoder, 'cache_key', decoder.name) for decoder in active_registry.decoders]
    fingerprint = hashlib.sha256(jar.read_bytes()).hexdigest()
    report_path = folder / 'asset-decoding.json'
    catalogue_path = folder / 'catalogue.json'
    if report_path.is_file() and catalogue_path.is_file():
        report = json.loads(report_path.read_text(encoding='utf-8'))
        catalogue = json.loads(catalogue_path.read_text(encoding='utf-8'))
        if (report.get('version') == VERSION and report.get('registry')==registry_names and report.get('jar_sha256') == fingerprint
                and all((folder/'original-assets'/(row['image_id']+'.png')).is_file() for row in catalogue)):
            progress(f'Reusing {report["unique_images"]} saved source images; decoding is already complete.')
            return report, catalogue
    sink = AssetSink(folder, identity, fresh_sources)
    with zipfile.ZipFile(jar) as archive:
        entries=archive.infolist()
        if len(entries)>20000 or sum(entry.file_size for entry in entries)>128*1024*1024:
            raise ValueError('Archive exceeds the supported import limit')
        context = DecodeContext(jar, folder, archive, sink, java,
                                Path(player).resolve() if player is not None else None, progress)
        progress('Detecting stored image and packed resource formats…')
        selected = active_registry.decode(context)
        packed_names={part.name for _,parts in context.containers for part in parts}
        embedded_names={row['resource'].split('@')[0] for row in sink.records if row['decoder']=='embedded-png-v1'}
        unclassified=[entry.filename for entry in context.resources()
                      if entry.filename not in context.direct|context.recovered|packed_names|embedded_names
                      and Path(entry.filename).suffix.lower() not in {'.txt','.mid','.midi','.wav','.mp3','.amr','.json','.jad'}]
        pixel_api=any(b'createRGBImage' in context.archive.read(entry) or b'drawRGB' in context.archive.read(entry)
                      for entry in entries if entry.filename.endswith('.class'))
        uncertain=bool(pixel_api and unclassified and not context.containers)
        recovered = embedded_names | context.recovered
        decision=('packed-assets' if context.containers else 'unresolved-pixel-path' if uncertain
                  else 'standard-images' if context.direct else 'embedded-images' if recovered else 'unknown')
        report = dict(version=VERSION, registry=registry_names, jar_sha256=fingerprint, decoders=selected,
                      additional_decoding=True if context.containers or recovered else None if uncertain or not context.direct else False,
                      decision=decision,unclassified_resources=unclassified,generated_pixel_api=pixel_api,
                      unique_images=len({row['image_id'] for row in sink.records}),
                      source_records=len(sink.records), containers=len(context.containers),
                      container_blocks=len(context.blocks),
                      features=context.features, outcomes=context.outcomes,
                      coverage='recognized image resources and packed sprites; unknown formats unverified')
    for path, value in [(catalogue_path,sink.records),(report_path,report)]:
        temporary=path.with_suffix('.tmp')
        temporary.write_text(json.dumps(value,indent=2),encoding='utf-8')
        os.replace(temporary,path)
    return report, sink.records
