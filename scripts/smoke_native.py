#!/usr/bin/env python3
"""A compiled Swift client against the persistent native helper, and the helper's answers against
the real CLI and agent-tool processes.

The helper is the `native_exchange` example: a test fixture that serves the native boundary over
stdin and stdout. The Swift program in tests/native/swift is the typed client. Writes go through
the real `dpm` CLI as another process would make them. Afterwards the envelopes the Swift client
decoded are compared, at one pinned clock, with the CLI `--json` output and the agent tools'
structuredContent for the same store, so a native view is the same payload the other adapters
return.

Skipped loudly, not passed, where no Swift compiler is available.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from smoke_agent import CLI, ROOT, Agent, pinned_clock, release_binary, run_cli, run_cli_envelope

FIXTURE = ROOT / 'tests/support/execution-plan.json'
SWIFT = sorted((ROOT / 'tests/native/swift').glob('*.swift'))
WORKER = 'agent:worker'


def build_helper():
    """The helper is an example, which `cargo build --workspace` does not build."""
    subprocess.run(['cargo', 'build', '--release', '-p', 'dpm-app', '--example', 'native_exchange', '--locked'],
                   cwd=ROOT, check=True, timeout=900)
    return release_binary('examples/native_exchange', 'DPM_NATIVE_HELPER')


def compile_proof(directory):
    proof = directory / 'proof'
    subprocess.run(['swiftc', '-O', '-swift-version', '5', '-parse-as-library', *map(str, SWIFT), '-o', str(proof)],
                   cwd=ROOT, check=True, timeout=900)
    return proof


def lagged_plan(directory):
    """The fixture with no decision gates and a half-hour start-to-start lag from TEST-A to TEST-B,
    so TEST-B's readiness depends on time alone once TEST-A has started."""
    plan = json.loads(FIXTURE.read_text())
    plan['decisions'] = {}
    keys = {item['key']: identity for identity, item in plan['work_items'].items()}
    for edge in plan['dependencies']:
        if edge['predecessor'] == keys['TEST-A'] and edge['successor'] == keys['TEST-B']:
            edge['kind'], edge['lag_hours'] = 'StartStart', 0.5
    path = directory / 'plan.json'
    path.write_text(json.dumps(plan))
    return path


def compare(name, native, other, source):
    assert native == other, f'{name}: the native envelope differs from {source}:\n native {json.dumps(native, sort_keys=True)}\n other  {json.dumps(other, sort_keys=True)}'


def compare_payloads(database, payloads, at):
    """The same store and clock through the CLI and the agent tools."""
    agent = Agent(database, 'agent:parity', clock=at)
    try:
        cli = lambda *words: run_cli_envelope(database, '--clock', at, *words)
        pairs = {
            'status': (cli('status'), ('project_status', {})),
            'next': (cli('next'), ('next_work', {})),
            'explain': (cli('explain', 'TEST-B'), ('explain_work', {'key': 'TEST-B'})),
            'show': (cli('show', 'TEST-A'), ('get_work', {'key': 'TEST-A'})),
            'history': (cli('history'), ('history', {})),
            'revision': (cli('revision'), ('workspace_revision', {})),
            'runs': (cli('run', 'list'), ('list_runs', {})),
        }
        for name, (from_cli, (tool, arguments)) in pairs.items():
            native = payloads[name]
            compare(name, native, from_cli, 'the CLI --json output')
            compare(name, native, agent.call(tool, arguments), 'the agent tool structuredContent')
    finally:
        agent.close()
    return len(pairs)


def main():
    if shutil.which('swiftc') is None:
        print('SKIPPED: no swiftc on this machine; the Swift client proof did not run and nothing is claimed.')
        return 0
    helper = build_helper()
    with tempfile.TemporaryDirectory() as scratch:
        directory = Path(scratch)
        proof = compile_proof(directory)
        database = directory / 'native.sqlite'
        run_cli(database, 'import', str(lagged_plan(directory)))
        at = pinned_clock()
        out = directory / 'payloads.json'
        # `--trace` prints every line on the wire to standard error.
        env = {**os.environ, 'DPM_NATIVE_TRACE': '1'} if '--trace' in sys.argv else None
        result = subprocess.run([str(proof), '--helper', str(helper), '--database', str(database), '--dpm', str(CLI),
                                 '--clock', at, '--out', str(out)], cwd=ROOT, text=True, timeout=90, env=env)
        assert result.returncode == 0, f'the Swift proof failed (exit {result.returncode})'
        compared = compare_payloads(database, json.loads(out.read_text()), at)
    print(f'PASS: Swift client over the persistent helper (handover, typed errors, clock-only, links, reconnect); '
          f'{compared} native envelopes equal the CLI and agent-tool payloads')
    return 0


if __name__ == '__main__':
    sys.exit(main())
