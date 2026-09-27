#!/usr/bin/env python3
"""Build a native qualification archive with exact committed source and SHA-256 checksums."""
import argparse
import gzip
import hashlib
import io
import json
import subprocess
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.check_output(args, cwd=ROOT)


def clean_revision():
    if command('git', 'status', '--porcelain', '--untracked-files=all').strip():
        raise SystemExit('Commit the intended source first: release archives require a clean checkout.')
    return command('git', 'rev-parse', 'HEAD').decode().strip()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_archive(path, prefix, files, epoch):
    with path.open('wb') as raw, gzip.GzipFile(fileobj=raw, filename='', mode='wb', mtime=epoch) as zipped:
        with tarfile.open(fileobj=zipped, mode='w') as archive:
            for name, (data, mode) in sorted(files.items()):
                entry = tarfile.TarInfo(f'{prefix}/{name}')
                entry.size, entry.mode, entry.mtime = len(data), mode, epoch
                archive.addfile(entry, io.BytesIO(data))


def package(output):
    revision = clean_revision()
    config = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']
    version = config['version']
    compiler = command('rustc', '-vV').decode()
    target = next(line.removeprefix('host: ') for line in compiler.splitlines() if line.startswith('host: '))
    if not ('apple-darwin' in target or 'linux' in target):
        raise SystemExit(f'Native release qualification is not configured for {target}.')
    epoch = int(command('git', 'show', '-s', '--format=%ct', revision))
    source = command('git', 'archive', '--format=tar', revision)
    built = command('cargo', 'build', '--locked', '--release', '--target', target,
                    '-p', 'dpm', '-p', 'dpm-mcp', '--bins', '--message-format=json')
    binaries = {}
    for line in built.splitlines():
        item = json.loads(line)
        if item.get('reason') == 'compiler-artifact' and item.get('executable'):
            binaries[item['target']['name']] = Path(item['executable']).read_bytes()
    if set(binaries) != {'dpm', 'dpm-mcp'}:
        raise SystemExit(f'Expected dpm and dpm-mcp artifacts; got {sorted(binaries)}')
    if clean_revision() != revision:
        raise SystemExit('The checkout changed while building; no archive was written.')
    files = {f'bin/{name}': (data, 0o755) for name, data in binaries.items()}
    with tarfile.open(fileobj=io.BytesIO(source)) as archive:
        for entry in archive:
            if entry.isfile():
                files[f'source/{entry.name}'] = (archive.extractfile(entry).read(), entry.mode)
            elif not entry.isdir():
                raise SystemExit(f'Unsupported source archive entry: {entry.name}')
    files['LICENSE'] = files['source/LICENSE']
    files['INSTALL.txt'] = (b'''DPM - DAG Project Manager

This is a qualification build, not a published or signed installer.
Verify the adjacent .sha256 file before extracting the archive.
Put bin/dpm and bin/dpm-mcp together in a directory on your PATH.
In a new project directory, run: dpm init "My project"
Run dpm to open the console, or dpm status --json for the machine view.
For a local MCP client, use dpm-mcp --project /absolute/project --actor agent:NAME.
Actor names are local identities, not authenticated accounts.

The source/ directory contains the corresponding committed source, Cargo.lock,
build instructions, and CLI/MCP documentation. Rebuild using:
  cd source
  cargo build --workspace --release --locked
See source/README.md and source/docs/mcp.md for supported behavior and limitations.
Licensing: AGPL-3.0-only; see LICENSE. Keep the source with redistributed binaries.
''', 0o644)
    manifest = {'version': version, 'target': target, 'revision': revision,
                'rustc': compiler.strip(), 'license': config['license'],
                'sha256': {name: digest(data) for name, (data, _) in sorted(files.items())}}
    files['manifest.json'] = ((json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode(), 0o644)
    prefix = f'dpm-{version}-{target}'
    output.mkdir(parents=True, exist_ok=True)
    destination = output / f'{prefix}.tar.gz'
    checksum = output / f'{prefix}.tar.gz.sha256'
    if destination.exists() or checksum.exists():
        raise SystemExit(f'Use an empty output directory; {destination} or its checksum already exists.')
    with tempfile.TemporaryDirectory(prefix='.dpm-package-', dir=output) as staging:
        pending = Path(staging) / destination.name
        write_archive(pending, prefix, files, epoch)
        value = digest(pending.read_bytes())
        # Exclusive creation also protects an archive produced concurrently.
        with destination.open('xb') as stream:
            stream.write(pending.read_bytes())
        with checksum.open('x') as stream:
            stream.write(f'{value}  {destination.name}\n')
    return destination


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='directory for archive and checksum')
    print(package(parser.parse_args().output.resolve()))
