"""Register validated sprite modules from the unpacked Java ME resource blocks."""
from systems.jme.resources.gameloft_sprite_layout import SIGNATURE, SpriteFormatError, read_sprite_layout
from systems.jme.resources.gameloft_sprites import decode_sprite_images

_MAX_BLOCK_PIXELS = 4_194_304
_MAX_IMPORT_PIXELS = 16_777_216
_MAX_IMPORT_IMAGES = 16_384


class GameloftSpriteImages:
    name = 'gameloft-indexed-sprites-v1'
    # New sprite encodings (raw 0x5602 bytes, content-type 3 blocks) change the
    # extracted set, so older decode reports must be rebuilt on the next Add.
    cache_key = 'gameloft-indexed-sprites-v2'

    def supports(self, context):
        return any(self.has_signature(block) for block in context.blocks)

    @staticmethod
    def has_signature(block):
        try:
            with block['path'].open('rb') as stream:
                return stream.read(len(SIGNATURE)) == SIGNATURE
        except OSError:
            return False

    def decode(self, context):
        remaining_pixels = _MAX_IMPORT_PIXELS
        remaining_images = _MAX_IMPORT_IMAGES
        stats = dict(decoded_blocks=0, empty_blocks=0, unsupported_blocks=0, limited_blocks=0)
        emitted = 0
        for block in context.blocks:
            if not self.has_signature(block):
                continue
            with block['path'].open('rb') as stream:
                data = stream.read()
            try:
                layout = read_sprite_layout(data)
                if layout is None or not layout.modules:
                    stats['empty_blocks'] += 1
                    continue
                pixels, images = self._work_size(layout)
                if pixels > min(remaining_pixels, _MAX_BLOCK_PIXELS) or images > remaining_images:
                    stats['limited_blocks'] += 1
                    continue
                remaining_pixels -= pixels
                remaining_images -= images
                sources = decode_sprite_images(layout)
            except SpriteFormatError:
                stats['unsupported_blocks'] += 1
                continue

            stats['decoded_blocks'] += 1
            resource = f"{block['resource']}#{block['index']}"
            for module, palette_bank, palette, image in sources:
                context.sink.add(image, resource, self.name, module=module,
                                 palette_bank=palette_bank, palette=palette)
                emitted += 1
                if emitted == 1 or emitted % 500 == 0:
                    context.progress(f'Extracted {emitted:,} packed sprite images so far.')
        if any(stats.values()):
            context.features['gameloft_sprites'] = stats
            context.progress(f"Decoded {stats['decoded_blocks']} packed sprite blocks.")

    @staticmethod
    def _work_size(layout):
        pixels = images = 0
        for module in layout.modules:
            variant_count = len(layout.palettes[module.palette_bank])
            pixels += module.width * module.height * variant_count
            images += variant_count
        return pixels, images
