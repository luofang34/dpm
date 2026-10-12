#!/usr/bin/env python3
"""Publish the workspace's crates.io packages in dependency order, resuming where a run stopped.

A version already on crates.io is skipped, so a run stopped by a failure or by the registry's
rate limit for new crates is simply started again. A rate-limit response waits and retries the
same crate. With --dry-run every package is packaged and built against the others without upload.
"""
import argparse
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

AGENT = 'dpm-publish (https://github.com/luofang34/dpm)'
# crates.io admits a short burst of new crates and then about one every ten minutes.
RATE_LIMIT_WAIT_S = 11 * 60
RATE_LIMIT_ATTEMPTS = 6


def packages():
    """Workspace members that publish, dependencies before dependents."""
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version', '1', '--no-deps', '--locked']))
    members = {p['name']: p for p in metadata['packages'] if p.get('publish') != []}
    ordered, seen = [], set()

    def visit(name):
        if name in seen:
            return
        seen.add(name)
        for dependency in members[name]['dependencies']:
            if dependency['name'] in members and dependency['kind'] is None:
                visit(dependency['name'])
        ordered.append((name, members[name]['version']))

    for name in sorted(members):
        visit(name)
    return ordered


def published(name, version):
    request = urllib.request.Request(f'https://crates.io/api/v1/crates/{name}/{version}', headers={'User-Agent': AGENT})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status == 200
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        raise


def publish(name):
    for attempt in range(1, RATE_LIMIT_ATTEMPTS + 1):
        result = subprocess.run(['cargo', 'publish', '--locked', '-p', name], capture_output=True, text=True)
        sys.stderr.write(result.stderr)
        if result.returncode == 0:
            return
        if '429' not in result.stderr and 'Too Many Requests' not in result.stderr:
            raise SystemExit(f'{name}: cargo publish failed')
        print(f'{name}: rate limited (attempt {attempt}); waiting {RATE_LIMIT_WAIT_S} s', flush=True)
        time.sleep(RATE_LIMIT_WAIT_S)
    raise SystemExit(f'{name}: still rate limited after {RATE_LIMIT_ATTEMPTS} attempts; run again later')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dry-run', action='store_true', help='package and build every crate without uploading')
    arguments = parser.parse_args()
    order = packages()
    print('order:', ' '.join(f'{n} {v}' for n, v in order), flush=True)
    if arguments.dry_run:
        subprocess.run(['cargo', 'publish', '--workspace', '--locked', '--dry-run'], check=True)
        return
    for name, version in order:
        if published(name, version):
            print(f'{name} {version}: already on crates.io', flush=True)
            continue
        publish(name)
        print(f'{name} {version}: published', flush=True)


if __name__ == '__main__':
    main()
