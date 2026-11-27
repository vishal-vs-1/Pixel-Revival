"""Describe independent playback and artwork readiness without starting work."""


def describe_readiness(game, *, image=False, poses=False):
    nes = game.get('system') == 'nes'
    runtime = game.get('runtime_status', 'setup-needed') if nes else game.get('status', 'Ready')
    playable = runtime not in ('setup-needed', 'unsupported') if nes else runtime == 'Ready'
    work = game.get('asset_work', {})
    base = ('prepared' if work.get('READY', 0) else
            'preparing' if sum(work.get(key, 0) for key in ('QUEUED', 'RUNNING', 'DEFERRED')) else
            'not prepared or not loaded yet')
    art = 'generated poses' if poses else 'image' if image else 'none selected'
    recording = 'on' if game.get('record_artwork', True) else 'off'
    return (f"Playback: {'available' if playable else 'setup or preparation needed'} · Base artwork: {base}\n"
            f"Character art: {art}" + (' · NES identity is experimental' if nes else '') +
            f" · Recording on next launch: {recording}" +
            (f"\n{work['completion_error']}" if work.get('completion_error') else '') +
            ('\nArtwork counts are temporarily unavailable; showing the last known counts.'
             if work.get('counts_unavailable') else ''))
