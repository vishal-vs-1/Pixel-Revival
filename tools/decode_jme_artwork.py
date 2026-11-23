"""Extract Java ME source artwork without initializing a game or emulator."""
import argparse
from pathlib import Path
import zipfile

from runtime_assets.contracts import image_source_id
from systems.jme.resources.decoding import DecoderRegistry, decode_jar, default_registry
from systems.jme.resources.indexed import IndexedImages, MAX_LAYOUT_BYTES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('jar', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--indexed-layout', type=Path, help='JSON layout for raw indexed resources')
    args = parser.parse_args()

    try:
        registry = default_registry()
        if args.indexed_layout is not None:
            with args.indexed_layout.open('rb') as layout:
                indexed = IndexedImages(layout.read(MAX_LAYOUT_BYTES + 1))
            registry = DecoderRegistry((*registry.decoders, indexed))
        report, _ = decode_jar(args.jar, args.output, image_source_id, progress=print, registry=registry)
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        parser.exit(1, f'Artwork extraction failed: {error}\n')

    print(f'Extracted {report["unique_images"]} unique source images to {args.output / "original-assets"}')
    print(f'Resource provenance is in {args.output / "catalogue.json"}')


if __name__ == '__main__':
    main()
