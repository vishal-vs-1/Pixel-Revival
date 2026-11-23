"""Artwork admission policy for large offline sprite catalogues."""
import json

from .contracts import SourceKind

PACKED_SPRITE_DECODER = 'gameloft-indexed-sprites-v1'
LARGE_PACKED_CATALOGUE = 512


def has_large_sprite_catalogue(game):
    decoding = game.get('asset_decoding')
    outcomes = decoding.get('outcomes') if isinstance(decoding, dict) else None
    if not isinstance(outcomes, dict):
        return False
    count = outcomes.get(PACKED_SPRITE_DECODER, 0)
    return isinstance(count, int) and count >= LARGE_PACKED_CATALOGUE


def packed_only_sources(workspace):
    """Keep assets also delivered by ordinary image files eligible at startup."""
    try:
        rows = json.loads((workspace / 'catalogue.json').read_text(encoding='utf-8'))
        packed, regular = set(), set()
        for row in rows:
            (packed if row['decoder'] == PACKED_SPRITE_DECODER else regular).add(row['image_id'])
        return packed - regular
    except (OSError, ValueError, AttributeError, KeyError, TypeError):
        return set()


def on_demand_packed(recipe):
    return recipe.algorithm == 'realesrgan-x4plus-anime'


def wait_for_capture(source, recipe):
    return source.origin == SourceKind.PACKED_OFFLINE and on_demand_packed(recipe)
