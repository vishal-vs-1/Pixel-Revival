from pathlib import Path

from systems.nes.runtime_setup import NesRuntimeLocator, RUNTIME_ROOT_ENV


class StubRuntime:
    def __init__(self, executable, manifest, available):
        self.executable = executable
        self.manifest = manifest
        self._available = available

    def available(self):
        return self._available


def make_runtime_files(root):
    executable = root / "nes" / "player" / "Mesen.exe"
    manifest = root / "nes" / "graphics-runtime.json"
    executable.parent.mkdir(parents=True)
    manifest.parent.mkdir(parents=True, exist_ok=True)
    executable.write_bytes(b"fixture")
    manifest.write_text("{}", encoding="utf-8")
    return executable, manifest


def test_installed_runtime_has_priority_over_developer_build(tmp_path):
    installed = tmp_path / "tools" / "nes-runtime"
    developer = tmp_path / "build" / "common-runtime-u22"
    installed_files = make_runtime_files(installed)
    make_runtime_files(developer)

    locator = NesRuntimeLocator(
        tmp_path, packaged=False,
        runtime_factory=lambda executable, manifest: StubRuntime(
            executable, manifest, executable == installed_files[0]),
    )

    runtime = locator.locate()

    assert runtime is not None
    assert runtime.executable == installed_files[0]


def test_developer_build_is_available_only_for_unfrozen_app(tmp_path):
    developer = tmp_path / "build" / "common-runtime-u22"
    executable, manifest = make_runtime_files(developer)
    factory = lambda exe, config: StubRuntime(exe, config, True)

    source_runtime = NesRuntimeLocator(
        tmp_path, packaged=False, runtime_factory=factory).locate()
    packaged_runtime = NesRuntimeLocator(
        tmp_path, packaged=True, runtime_factory=factory).locate()

    assert source_runtime is not None
    assert source_runtime.manifest == manifest
    assert packaged_runtime is None


def test_capture_enabled_developer_build_precedes_original_only_build(tmp_path):
    capture = tmp_path / "build" / "common-runtime-u18c-capture"
    original_only = tmp_path / "build" / "common-runtime-u22"
    capture_executable, _ = make_runtime_files(capture)
    make_runtime_files(original_only)

    runtime = NesRuntimeLocator(
        tmp_path, packaged=False,
        runtime_factory=lambda exe, config: StubRuntime(exe, config, True),
    ).locate()

    assert runtime is not None and runtime.executable == capture_executable


def test_explicit_runtime_root_overrides_all_discovery(tmp_path):
    configured = tmp_path / "external" / "nes-runtime"
    executable, _ = make_runtime_files(configured)
    make_runtime_files(tmp_path / "build" / "common-runtime-u22")
    locator = NesRuntimeLocator(
        tmp_path, packaged=False,
        environment={RUNTIME_ROOT_ENV: str(configured)},
        runtime_factory=lambda exe, manifest: StubRuntime(exe, manifest, True),
    )

    runtime = locator.locate()

    assert runtime is not None
    assert runtime.executable == executable


def test_invalid_explicit_override_fails_closed(tmp_path):
    make_runtime_files(tmp_path / "build" / "common-runtime-u22")
    locator = NesRuntimeLocator(
        tmp_path, packaged=False,
        environment={RUNTIME_ROOT_ENV: "relative/path"},
        runtime_factory=lambda exe, manifest: StubRuntime(exe, manifest, True),
    )

    assert locator.locate() is None


def test_missing_or_invalid_runtime_remains_unavailable(tmp_path):
    installed = tmp_path / "tools" / "nes-runtime"
    executable, manifest = make_runtime_files(installed)
    locator = NesRuntimeLocator(
        tmp_path, packaged=True,
        runtime_factory=lambda exe, config: StubRuntime(exe, config, False),
    )

    assert locator.locate() is None

    executable.unlink()
    assert NesRuntimeLocator(
        tmp_path, packaged=True,
        runtime_factory=lambda exe, config: StubRuntime(exe, config, True),
    ).locate() is None
