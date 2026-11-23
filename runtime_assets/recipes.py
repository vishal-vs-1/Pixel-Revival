"""Recipe selection and backend resolution for the shared artwork worker."""

from .backends import NcnnBackend, PixelBackend, RedrawBackend
from .contracts import pixel_recipe, redraw_recipe
from .pixel import enhance


class ArtworkRecipes:
    def __init__(self):
        self._backends = {}

    def for_backend(self, backend):
        if backend == 'pixel':
            return pixel_recipe(), None
        if backend == "redraw":
            return redraw_recipe(), None
        ncnn = NcnnBackend()
        recipe = ncnn.recipe()
        self._backends[recipe.key] = ncnn
        return recipe, recipe

    def backend_for_recipe(self, recipe):
        if recipe.key in self._backends:
            return self._backends[recipe.key]
        if recipe.algorithm == 'scale2x-rgba':
            backend = PixelBackend(enhance)
        elif recipe.algorithm == RedrawBackend.algorithm:
            backend = RedrawBackend()
        elif recipe.algorithm == 'realesrgan-x4plus-anime':
            backend = NcnnBackend()
            if backend.recipe().key != recipe.key:
                raise RuntimeError('The installed AI model no longer matches this recipe.')
        else:
            raise LookupError(f'No enhancement backend for {recipe.algorithm!r}')
        self._backends[recipe.key] = backend
        return backend
