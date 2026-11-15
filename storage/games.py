"""Remove owned game files while retaining the complete emulator save directory."""
from pathlib import Path
import re
import shutil
import os
import uuid


def validate_game_id(game_id):
    if not isinstance(game_id,str) or len(game_id)!=64 or any(c not in '0123456789abcdef' for c in game_id):
        raise ValueError('Invalid game identifier')
    return game_id


class GameStorage:
    """Filesystem policy only; player and conversion lifetimes belong to the caller."""

    def __init__(self,library,rendered,*,source_filename='game.jar'):
        if (not isinstance(source_filename, str) or
                not re.fullmatch(r'[a-zA-Z0-9_-]{1,64}\.[a-zA-Z0-9]{1,12}', source_filename) or
                source_filename.partition('.')[0].upper() in
                {'CON', 'PRN', 'AUX', 'NUL', *(f'{port}{n}' for port in ('COM', 'LPT') for n in range(10))}):
            raise ValueError('Game source filename must be a plain, safe basename.')
        self.library=Path(library).resolve()
        self.rendered=Path(rendered).resolve()
        self.source_filename=source_filename

    @staticmethod
    def _owned(path,parent):
        # Reject redirected game roots and data entries before any recursive
        # removal. shutil.rmtree itself does not traverse nested directory links.
        if path.is_symlink() or path.is_junction():
            raise ValueError('A linked game folder cannot be deleted from the library.')
        resolved=path.resolve()
        if not resolved.is_relative_to(parent) or resolved==parent:
            raise ValueError('Game data must remain inside its library folder.')
        return path

    def game_paths(self,game_id):
        identity=validate_game_id(game_id)
        source=self._owned(self.library/identity,self.library)
        workspace=self._owned(self.rendered/identity,self.rendered)
        if source.is_relative_to(workspace) or workspace.is_relative_to(source):
            raise ValueError('The game library and rendered data folders must be separate.')
        return source, workspace

    def source_path(self,game_id):
        source, _ = self.game_paths(game_id)
        return self._owned(source / self.source_filename, source)

    def deletion_paths(self,game_id):
        source, workspace = self.game_paths(game_id)
        self._owned(source / self.source_filename, source)
        entries=list(workspace.iterdir()) if workspace.exists() else []
        data=[self._owned(entry,workspace) for entry in entries if entry.name.casefold()!='saves']
        # Keep metadata until the other removal steps succeed, so a failed
        # removal can be retried from the library.
        data.sort(key=lambda entry:entry.name=='game.json')
        return source,workspace,data

    @staticmethod
    def _remove(path,parent):
        GameStorage._owned(path,parent)
        if path.is_dir():shutil.rmtree(path)
        elif path.exists():path.unlink()

    def delete(self,game_id):
        source,workspace,data=self.deletion_paths(game_id)
        metadata=workspace/'game.json'
        cartridge=source/self.source_filename
        backup=self.library/('.deleting-'+uuid.uuid4().hex+'-'+self.source_filename)

        # Rebuildable files can be removed in place. Keep the library entry
        # intact until that cleanup finishes, so a locked file can be retried.
        for entry in data:
            if entry != metadata:
                self._remove(entry,workspace)

        try:
            if cartridge.is_file():
                # Keep a recovery copy until the source and metadata are gone.
                try:os.link(cartridge,backup)
                except OSError:shutil.copy2(cartridge,backup)
            self._remove(source,self.library)
            metadata.unlink(missing_ok=True)
        except Exception as error:
            if backup.is_file() and not cartridge.is_file():
                try:
                    source.mkdir(exist_ok=True)
                    os.replace(backup,cartridge)
                except OSError as restore_error:
                    error.add_note(f'Could not restore the cartridge: {restore_error}. Recovery copy: {backup}')
            if cartridge.is_file():
                try:backup.unlink(missing_ok=True)
                except OSError:pass
            raise
        try:backup.unlink(missing_ok=True)
        except OSError:pass
        if workspace.is_dir() and not any(workspace.iterdir()):
            try:workspace.rmdir()
            except OSError:pass
        return workspace/'saves'
