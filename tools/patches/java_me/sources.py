"""Install owned Java sources into the upstream Java package tree."""

from pathlib import Path
import shutil


ROOT = Path(__file__).resolve().parents[3]
COMMON_GRAPHICS_JAVA = ('GraphicsPacketWriter.java', 'NativeGraphics.java')


def copy_common_graphics_java(vendor_source, java_source=None):
    """Copy only production JNI classes while preserving their Java package path."""
    java_source = java_source or ROOT / 'native-graphics/jni/java/org/revival/graphics'
    package_target = Path(vendor_source) / 'org/revival/graphics'
    package_target.mkdir(parents=True, exist_ok=True)
    for filename in COMMON_GRAPHICS_JAVA:
        source = Path(java_source) / filename
        if not source.is_file():
            raise FileNotFoundError('Missing common graphics Java source: ' + str(source))
        shutil.copy2(source, package_target / filename)


def install_sources(root, source):
    # The source folders group concerns. Upstream Java still expects these
    # classes together in org.recompile.mobile.
    additions = sorted((root / 'emulator-src').rglob('*.java'))
    if not additions or len({path.name.casefold() for path in additions}) != len(additions):
        raise RuntimeError('Expected distinct Java integration source filenames.')
    for addition in additions:
        shutil.copy2(addition, source / 'mobile' / addition.name)
    copy_common_graphics_java(root / 'vendor/freej2me-plus/src',
                              root / 'native-graphics/jni/java/org/revival/graphics')
    for obsolete in ('DecodedAssetCapture.java', 'PackedAssetProbe.java', 'MibSpriteDecoder.java'):
        stale = source / 'mobile' / obsolete
        if stale.exists(): stale.unlink()
