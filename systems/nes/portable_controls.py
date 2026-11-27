"""Translate keyboard bindings to the portable Mesen player settings."""

from .controls import normalize_nes_bindings


PORTABLE_CONFIG_VERSION = 6


_KEY_CODES = {'ARROW_LEFT': 23, 'ARROW_UP': 24, 'ARROW_RIGHT': 25, 'ARROW_DOWN': 26,
              'ENTER': 6, 'NUMPAD_ENTER': 511, 'SPACE': 18,
              **{f'KEY_{letter}': index for index, letter in enumerate('ABCDEFGHIJKLMNOPQRSTUVWXYZ', 44)},
              **{f'DIGIT_{digit}': 34 + digit for digit in range(10)},
              **{f'NUMPAD_{digit}': 74 + digit for digit in range(10)}}


def set_keyboard(settings, controls=None):
    bindings = normalize_nes_bindings(controls)
    port = settings.setdefault('Nes', {}).setdefault('Port1', {})
    port['Type'] = 'NesController'
    port['Mapping2'] = {action.title(): _KEY_CODES[key] for action, key in bindings.items()}
    if settings.get('ConfigUpgrade', 0) < PORTABLE_CONFIG_VERSION:
        # First-run initialization would overwrite the explicit keyboard map.
        settings['ConfigUpgrade'] = PORTABLE_CONFIG_VERSION
