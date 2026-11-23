from PIL import Image
import pytest

from systems.generated_poses import GeneratedPoses
from systems.jme.pose_artwork import PoseArtwork


def test_failed_batch_keeps_the_previous_generation(tmp_path):
    store = GeneratedPoses(tmp_path, 'regenerated')
    first, second = 'a' * 32, 'b' * 32
    original = Image.new('RGBA', (16, 16), 'red')
    store.regenerate([first, second], lambda key: original)
    previous = store.current([second, first])

    def fail_on_second(key):
        if key == second:
            raise ValueError('Broken capture')
        return original

    with pytest.raises(ValueError, match='Broken capture'):
        store.regenerate([first, second], fail_on_second)
    assert store.current([first, second]) == previous
    assert previous.is_dir()
    assert {child.name for child in previous.parent.iterdir()} == {previous.name, 'current.json'}
    assert store.current([first]) is None


def test_java_generated_play_uses_only_current_confirmed_poses(tmp_path):
    artwork = PoseArtwork(tmp_path)
    selected, assigned, candidate = (char * 64 for char in 'abc')
    source = Image.new('RGBA', (9, 13))
    source.paste('red', (2, 3, 7, 11))
    artwork.folder('poses').mkdir(parents=True)
    for key in (selected, assigned, candidate):
        source.save(artwork.folder('poses') / f'{key}.png')
    artwork.set_selected(selected, True)
    artwork.assign(None, artwork.folder('poses') / f'{candidate}.png')
    artwork.assign(assigned, artwork.folder('poses') / f'{candidate}.png')
    default_before = artwork.assigned(None).read_bytes()
    override_before = artwork.assigned(assigned).read_bytes()

    assert artwork.regenerate() == 2
    folder = artwork.regenerated.current([selected, assigned])
    assert artwork.launch_arguments(False, regenerated=True) == [
        f'-Djme.playerPoses={folder}', '-Djme.recordPoses=false',
    ]
    assert artwork.launch_arguments(False)[0] == f'-Djme.playerPoses={artwork.root}'
    assert {path.stem for path in (folder / 'images').glob('*.png')} == {selected, assigned}
    with Image.open(folder / 'poses' / f'{selected}.png') as capture:
        assert capture.tobytes() == source.tobytes()
    with Image.open(folder / 'images' / f'{selected}.png') as generated:
        assert generated.getchannel('A').getbbox() == (8, 12, 28, 44)
    artwork.set_selected(selected, False)
    with pytest.raises(ValueError, match='Regenerate all confirmed'):
        artwork.launch_arguments(False, regenerated=True)
    assert artwork.assigned(None).read_bytes() == default_before
    assert artwork.assigned(assigned).read_bytes() == override_before
