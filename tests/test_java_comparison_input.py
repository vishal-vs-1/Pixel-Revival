from pathlib import Path
import subprocess
import pytest

from systems.jme.jdk import locate_java
from tools.patches.java_me.frontend import patch_comparison_frontend, patch_control_profile_frontend

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize('ending', ['', '!CLOSE\n'])
def test_shared_java_input_releases_on_eof_and_filters_repeat_presses(tmp_path, ending):
    subprocess.run([locate_java('javac'), '--release', '8', '-d', str(tmp_path),
                    str(ROOT / 'emulator-src/input/ComparisonInput.java'),
                    str(ROOT / 'tests/java/ComparisonInputCheck.java')],
                   check=True, capture_output=True, timeout=30)
    result = subprocess.run([locate_java(), '-Djava.awt.headless=true',
                             f'-Djme.comparisonReady={tmp_path / "ready"}',
                             '-cp', str(tmp_path), 'ComparisonInputCheck'],
                            input='+ARROW_RIGHT\n+ARROW_RIGHT\n' + ending, text=True,
                            capture_output=True, timeout=10, check=True)
    assert 'comparison input passed' in result.stdout
    assert (tmp_path / 'ready').is_file()


def test_cpu_comparison_input_starts_before_game_code_and_upgrades_old_patch():
    source = (ROOT / 'tests/fixtures/freej2me-plus/FreeJ2ME.java.txt').read_text()
    source = patch_control_profile_frontend(source)
    patched = patch_comparison_frontend(source)
    assert patched.index('ComparisonInput.start(') < patched.index('Mobile.getPlatform().runJar();')
    assert patch_comparison_frontend(patched) == patched
    # Simulate the previous installed hook order to check migration on rebuild.
    run = '\t\t\tMobile.getPlatform().runJar();\n'
    old = patched.replace(run, '', 1)
    start = old.index('ComparisonInput.start(')
    old = old[:start] + 'Mobile.getPlatform().runJar();\n\t\t\t' + old[start:]
    assert patch_comparison_frontend(old) == patched
