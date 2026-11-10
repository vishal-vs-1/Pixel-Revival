"""Translate application operations to the existing Java ME library."""

from .contracts import JAVA_ME
from .presentation import SystemPresentation


class JavaMeSystem:
    system = JAVA_ME

    def __init__(self, library, *, demo=None):
        self._library = library
        self.presentation = SystemPresentation(
            "Choose a Java ME game JAR", (("Java ME game", "*.jar"),), demo)

    @property
    def control_profiles(self):
        return self._library.control_profiles

    def games(self):
        return self._library.games()

    def import_game(self, request, progress):
        return self._library.import_game(request.path, progress, request.artwork_backend)

    def recover_assets(self, on_error, should_stop):
        self._library.recover_assets(on_error, should_stop)

    def workspace(self, identity):
        return self._library.workspace(identity)

    def texture_dir(self, identity, backend):
        return self._library.texture_dir(self.workspace(identity), backend)

    def delete_game(self, identity, *args, **kwargs):
        return self._library.delete_game(identity, *args, **kwargs)

    def set_control_profile_id(self, identity, slot_id):
        return self._library.set_control_profile_id(identity, slot_id)

    @staticmethod
    def _legacy_game(game):
        # The routing tag lives in application views, outside the legacy metadata format.
        result = dict(game)
        result.pop("system", None)
        return result

    def save(self, game):
        self._library.save(self._legacy_game(game))

    def prepare_style(self, game, backend, progress):
        return self._library.prepare_style(self._legacy_game(game), backend, progress)

    def launch(self, game, original=False, **kwargs):
        legacy = self._legacy_game(game)
        result = self._library.launch(legacy, original, **kwargs)
        # Startup repair may refresh the hint that the App displays after launch.
        game.update(legacy)
        return result
