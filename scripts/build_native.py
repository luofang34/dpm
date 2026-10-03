#!/usr/bin/env python3
"""Build the packaged native bridge: the Rust helper, the Swift client library, a minimal host, the
SwiftUI observer and the app bundles that carry them.

Layout of the result, under the Cargo target directory in `native/`:

    DPMHost.app/Contents/MacOS/dpm-host        the minimal host
    DPMHost.app/Contents/Helpers/dpm-native    the helper, found from the host's own location
    DPMHost.app/Contents/Info.plist
    DPMObserver.app/Contents/MacOS/dpm-observer      the SwiftUI live execution observer
    DPMObserver.app/Contents/Helpers/dpm-native      its own copy of the helper
    DPMObserver.app/Contents/Info.plist
    lib/libDPMNative.a, lib/DPMNative.swiftmodule   the client library, usable on its own
    lib/libDPMObserverCore.a, lib/DPMObserverCore.swiftmodule   the observer's engine, no UI
    qualify                                    the integration suite (see scripts/smoke_native.py)

The bundles are signed ad hoc, which needs no credential and only makes them launchable;
distribution signing and notarization are not part of this. Requires a Swift compiler on macOS.
"""
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SWIFT = ROOT / 'native/swift'
LIBRARY_SOURCES = sorted((SWIFT / 'Sources/DPMNative').glob('*.swift'))
OBSERVER_CORE_SOURCES = sorted((SWIFT / 'Sources/DPMObserverCore').glob('*.swift'))
OBSERVER_SOURCES = sorted((SWIFT / 'Sources/DPMObserver').glob('*.swift'))
# One deployment target for every Swift module, so the library, the host, the observer and the suite
# agree on which APIs they may use.
FLAGS = ['-O', '-swift-version', '5', '-target', f'{platform.machine()}-apple-macosx14.0']


def run(*command, **options):
    subprocess.run([str(part) for part in command], check=True, cwd=ROOT, **options)


def target_directory():
    metadata = subprocess.check_output(['cargo', 'metadata', '--format-version', '1', '--no-deps', '--locked'], cwd=ROOT, text=True)
    return Path(json.loads(metadata)['target_directory'])


def build(rust=True):
    """Build everything and return the paths the integration suite needs. `rust=False` reuses the
    helper already built, for iterating on the Swift sources alone."""
    if sys.platform != 'darwin':
        raise SystemExit(f'the native bridge is a macOS client and is not built on {sys.platform}')
    if shutil.which('swiftc') is None:
        raise SystemExit('swiftc is not available: the native bridge cannot be built here')
    if rust:
        run('cargo', 'build', '--release', '-p', 'dpm-native', '--locked')
    # Without Cargo's own answer, the directory Cargo would use by default.
    target = target_directory() if rust else Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target').resolve()
    helper = target / 'release' / 'dpm-native'
    if not helper.is_file():
        raise SystemExit(f'{helper} is missing; build it without --no-rust')
    out = target / 'native'
    library = out / 'lib'
    shutil.rmtree(out, ignore_errors=True)
    library.mkdir(parents=True)
    run('swiftc', *FLAGS, '-parse-as-library', '-emit-library', '-static', '-emit-module', '-module-name', 'DPMNative',
        '-emit-module-path', library / 'DPMNative.swiftmodule', '-o', library / 'libDPMNative.a', *LIBRARY_SOURCES)
    run('swiftc', *FLAGS, '-parse-as-library', '-emit-library', '-static', '-emit-module', '-module-name', 'DPMObserverCore',
        '-I', library, '-emit-module-path', library / 'DPMObserverCore.swiftmodule', '-o', library / 'libDPMObserverCore.a',
        *OBSERVER_CORE_SOURCES)
    app = out / 'DPMHost.app/Contents'
    (app / 'MacOS').mkdir(parents=True)
    (app / 'Helpers').mkdir()
    host = app / 'MacOS/dpm-host'
    run('swiftc', *FLAGS, '-parse-as-library', '-I', library, '-L', library, '-lDPMNative',
        SWIFT / 'Sources/DPMHost/main.swift', '-o', host)
    shutil.copy2(helper, app / 'Helpers/dpm-native')
    shutil.copy2(SWIFT / 'Info.plist', app / 'Info.plist')
    if shutil.which('codesign'):
        run('codesign', '--force', '--sign', '-', '--deep', out / 'DPMHost.app', stderr=subprocess.DEVNULL)
    observer = out / 'DPMObserver.app/Contents'
    (observer / 'MacOS').mkdir(parents=True)
    (observer / 'Helpers').mkdir()
    executable = observer / 'MacOS/dpm-observer'
    run('swiftc', *FLAGS, '-parse-as-library', '-I', library, '-L', library, '-lDPMObserverCore', '-lDPMNative',
        *OBSERVER_SOURCES, '-o', executable)
    shutil.copy2(helper, observer / 'Helpers/dpm-native')
    shutil.copy2(SWIFT / 'ObserverInfo.plist', observer / 'Info.plist')
    if shutil.which('codesign'):
        run('codesign', '--force', '--sign', '-', '--deep', out / 'DPMObserver.app', stderr=subprocess.DEVNULL)
    return {'app': out / 'DPMHost.app', 'host': host, 'helper': app / 'Helpers/dpm-native', 'library': library, 'out': out,
            'observer': out / 'DPMObserver.app', 'observer_executable': executable, 'observer_helper': observer / 'Helpers/dpm-native'}


def compile_suite(paths):
    """The integration suite, linked against the same library the host uses."""
    suite = paths['out'] / 'qualify'
    sources = sorted((SWIFT / 'Qualification').glob('*.swift'))
    run('swiftc', *FLAGS, '-parse-as-library', '-I', paths['library'], '-L', paths['library'], '-lDPMObserverCore', '-lDPMNative', *sources, '-o', suite)
    return suite


if __name__ == '__main__':
    built = build(rust='--no-rust' not in sys.argv)
    print(f"built {built['app']}")
    if '--suite' in sys.argv:
        print(f"built {compile_suite(built)}")
