#!/usr/bin/env python3
"""The packaged native bridge, qualified as shipped: the app bundle is copied away from the build
tree and every client runs from another directory, so nothing depends on where it was built.

Three things are proven here:

* Payload parity. The packaged host (`dpm-host`, which finds the bundled helper by its own location)
  prints each view, and those envelopes are compared, at one pinned clock, with the CLI `--json`
  output and the agent tools' structuredContent for the same store.
* The Swift integration suite (native/swift/Qualification), linked against the same client library,
  which drives the real helper, the real CLI and a fault-injecting relay: typed refusals with no
  partial writes, source identity, cancellation, close, early exit, crash, reconnect, window
  cleanup, bounded admission, frame faults and main-thread responsiveness, and the observer's engine
  and model against external CLI writes and run activity.
* The packaged SwiftUI observer (scripts/smoke_observer.py), launched from a copy of its bundle, its own
  state file read back while the CLI writes the stores it follows.

Writes go only to disposable synthetic stores. Skipped loudly, not passed, off macOS or where there is
no Swift compiler.
"""
import json
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

import build_native
import smoke_observer
from smoke_agent import CLI, ROOT, Agent, run_cli, run_cli_envelope

FIXTURE = ROOT / 'tests/support/execution-plan.json'
PROXY = ROOT / 'native/swift/Qualification/Fixtures/fault_proxy.py'
WRITER = ROOT / 'scripts/native_activity_writer.py'
WORKER = 'agent:worker'

# The host's command for each payload, and the CLI and agent-tool call that must equal it.
HOST_COMMANDS = {
    'status': (['status'], ['status'], ('project_status', {})),
    'next': (['next'], ['next'], ('next_work', {})),
    'explain': (['explain', 'TEST-B'], ['explain', 'TEST-B'], ('explain_work', {'key': 'TEST-B'})),
    'show': (['show', 'TEST-A'], ['show', 'TEST-A'], ('get_work', {'key': 'TEST-A'})),
    'history': (['history'], ['history'], ('history', {})),
    'revision': (['revision'], ['revision'], ('workspace_revision', {})),
    'runs': (['runs'], ['run', 'list'], ('list_runs', {})),
}


def clock():
    """Half an hour ahead: later than every operation the suite records while it runs, and one
    instant for every query given it."""
    return (datetime.now(timezone.utc) + timedelta(minutes=30)).isoformat()


def lagged_plan(directory):
    """The fixture with no decision gates and a half-hour start-to-start lag from TEST-A to TEST-B,
    so TEST-B's readiness depends on time alone once TEST-A has started."""
    plan = json.loads(FIXTURE.read_text())
    plan['decisions'] = {}
    keys = {item['key']: identity for identity, item in plan['work_items'].items()}
    for edge in plan['dependencies']:
        if edge['predecessor'] == keys['TEST-A'] and edge['successor'] == keys['TEST-B']:
            edge['kind'], edge['lag_hours'] = 'StartStart', 0.5
    path = directory / 'lagged-plan.json'
    path.write_text(json.dumps(plan))
    return path


def compare(name, native, other, source):
    assert native == other, f'{name}: the host envelope differs from {source}:\n host  {json.dumps(native, sort_keys=True)}\n other {json.dumps(other, sort_keys=True)}'


def host_payload(host, database, at, words):
    """What the packaged host prints, run from the filesystem root rather than any build directory."""
    result = subprocess.run([str(host), '--database', str(database), '--clock', at, *words], cwd='/',
                            capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, f'dpm-host {words} exited {result.returncode}: {result.stdout} {result.stderr}'
    return json.loads(result.stdout)


def payload_parity(host, directory, at):
    database = directory / 'parity.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    agent = Agent(database, 'agent:parity', clock=at)
    try:
        for name, (host_words, cli_words, (tool, arguments)) in HOST_COMMANDS.items():
            native = host_payload(host, database, at, host_words)
            compare(name, native, run_cli_envelope(database, '--clock', at, *cli_words), 'the CLI --json output')
            compare(name, native, agent.call(tool, arguments), 'the agent tool structuredContent')
    finally:
        agent.close()
    return len(HOST_COMMANDS)


def negative_controls(suite, arguments, directory):
    """The suite must be able to fail. Two runs that cannot succeed are required to exit nonzero with
    the reason on standard output: a wait for an event that never happens, and a scenario whose helper
    never connects, which an earlier suite passed with a single unrelated check."""
    controls = {
        'a wait for an event that never happens': (['--only', 'observer-negative-control', '--helper', str(directory / 'DPMHost.app/Contents/Helpers/dpm-native')], 'timed out'),
        'a helper that cannot be started': (['--only', 'observer-duplicates', '--helper', str(directory / 'no-such-helper')], 'stopped early'),
    }
    for name, (selection, expected) in controls.items():
        work = directory / f"control-{len(list(directory.glob('control-*')))}"
        work.mkdir()
        replaced = [work if argument == str(directory / 'suite') else argument for argument in arguments]
        result = subprocess.run([str(suite), *selection, *replaced], cwd='/', capture_output=True, text=True, timeout=300)
        assert result.returncode != 0 and expected in result.stdout, f'the negative control for {name} did not fail as required (exit {result.returncode}):\n{result.stdout[-1500:]}'


def main():
    if sys.platform != 'darwin':
        print(f'SKIPPED: the native bridge is a macOS client and this is {sys.platform}; nothing was built or qualified and nothing is claimed.')
        return 0
    if shutil.which('swiftc') is None:
        print('SKIPPED: no swiftc on this machine; the native bridge was not built or qualified and nothing is claimed.')
        return 0
    built = build_native.build(rust='--no-rust' not in sys.argv)
    suite = build_native.compile_suite(built)
    with tempfile.TemporaryDirectory() as scratch:
        directory = Path(scratch)
        app = directory / 'DPMHost.app'
        shutil.copytree(built['app'], app, symlinks=True)
        host = app / 'Contents/MacOS/dpm-host'
        helper = app / 'Contents/Helpers/dpm-native'
        at = clock()
        compared = payload_parity(host, directory, at)
        work = directory / 'suite'
        work.mkdir()
        arguments = ['--dpm', str(CLI), '--host', str(host), '--plan', str(FIXTURE), '--lagged-plan', str(lagged_plan(directory)),
                     '--proxy', str(PROXY), '--scratch', str(work), '--clock', at, '--writer', str(WRITER)]
        result = subprocess.run([str(suite), '--helper', str(helper), *arguments], cwd='/', timeout=1800)
        assert result.returncode == 0, f'the Swift integration suite failed (exit {result.returncode})'
        negative_controls(suite, arguments, directory)
    print(f'PASS: packaged host and Swift suite against the shipped helper; {compared} host envelopes equal the CLI and agent-tool payloads; both negative controls fail with nonzero exit')
    for line in smoke_observer.main(built['observer']):
        print(f'PASS: observer app: {line}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
