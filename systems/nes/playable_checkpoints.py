"""Find the most recent checkpoint already written by Mesen for a game."""

import json
import re
from zipfile import BadZipFile, ZipFile

from .capture import owned_path

MAX_STATE_BYTES = 512 * 1024
_SESSION_NAME = re.compile(r'[0-9a-f]{32}')
_PLAYER_NAME = re.compile(r'(?:player|recorder)-[0-9a-f]{16}-[0-9a-f]{16}')
_SLOT_NAME = re.compile(r'game_(?:[1-9]|1[01])\.mss', re.IGNORECASE)


def _owned_files(workspace, directory, rule):
    folder = owned_path(workspace, directory)
    if folder.is_dir():
        for path in folder.iterdir():
            if rule.fullmatch(path.name) and not path.is_symlink() and not path.is_junction():
                yield path


def _finished_session(workspace, session, identity):
    try:
        prefix = session.relative_to(workspace)
        documents = []
        for name in ('status.json', 'launch.json'):
            with owned_path(workspace, str(prefix / name)).open('rb') as stream:
                raw = stream.read(65537)
            if len(raw) > 65536:
                return False
            documents.append(json.loads(raw))
        status, launch = documents
        return (status.get('phase') == 'stopped' and status.get('session_id') == session.name
                and launch.get('session_id') == session.name and launch.get('game_id') == identity)
    except (OSError, ValueError, AttributeError):
        return False


def latest_checkpoint(workspace, identity):
    candidates = []

    def include(path, archive=False):
        try:
            if path.is_file():
                candidates.append((path.stat().st_mtime_ns, archive, path))
        except OSError:
            pass

    for path in _owned_files(workspace, 'saves/mesen/states', _SLOT_NAME):
        include(path)
    # The normal managed frontend stores a resume snapshot on clean close.
    for session in _owned_files(workspace, '.runtime/sessions', _SESSION_NAME):
        if _finished_session(workspace, session, identity):
            include(owned_path(workspace, str(session.relative_to(workspace) / 'home/RecentGames/game.rgd')), True)
    for player in _owned_files(workspace, 'playable-assets', _PLAYER_NAME):
        include(owned_path(workspace, str(player.relative_to(workspace) / 'RecentGames/game.rgd')), True)
    if not candidates:
        raise ValueError('No checkpoint is available yet. Play this game and close Mesen normally, then try again. '
                         'You can also press Shift + F1 in Mesen to save a checkpoint at a specific moment.')
    return max(candidates)[2]


def read_checkpoint(path):
    if path.suffix.casefold() != '.rgd':
        with path.open('rb') as stream:
            return stream.read(MAX_STATE_BYTES + 1)
    try:
        if path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError('The Mesen resume archive is too large.')
        with ZipFile(path) as archive:
            entry = archive.getinfo('Savestate.mss')
            if entry.file_size > MAX_STATE_BYTES:
                raise ValueError('The Mesen resume state is too large.')
            with archive.open(entry) as state:
                return state.read(MAX_STATE_BYTES + 1)
    except (BadZipFile, KeyError, RuntimeError) as error:
        raise ValueError('Mesen did not finish writing the resume checkpoint. Use Choose save state for an earlier one.') from error
