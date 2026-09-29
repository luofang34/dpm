#!/usr/bin/env python3
"""Exercise schema versioning, backup, restore and store verification through the real CLI."""
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

from smoke_agent import CLI, ROOT, unwrap, with_actor


def cli(*arguments, cwd=ROOT, error=None):
    result = subprocess.run([str(CLI), '--json', *map(str, with_actor(arguments))], cwd=cwd,
                            capture_output=True, text=True, timeout=30)
    value = json.loads(result.stdout)
    if error:
        assert result.returncode != 0 and value['error']['code'] == error, (arguments, value, result.stderr)
    else:
        assert result.returncode == 0, (arguments, value, result.stderr)
        value = unwrap(value)
    return value


def pragma(path, name):
    with sqlite3.connect(path) as connection:
        return connection.execute(f'PRAGMA {name}').fetchone()[0]


def history(database):
    return cli('--database', database, 'history', '--limit', '1000')['entries']


def live_store(directory):
    live = directory / 'live.sqlite'
    cli('--database', live, 'import', ROOT / 'tests/support/execution-plan.json')
    cli('--database', live, 'claim', 'TEST-A')
    assert pragma(live, 'user_version') == 3
    return live


def backups_during_writes(directory, live):
    def write():
        for step in range(12):
            cli('--database', live, *(('block', 'TEST-A', f'waiting {step}') if step % 2 == 0 else ('unblock', 'TEST-A')))

    writer = threading.Thread(target=write)
    writer.start()
    reports = []
    for index in range(4):
        reports.append(cli('--database', live, 'backup', '--to', directory / f'during-{index}.sqlite'))
    writer.join()
    for index, report in enumerate(reports):
        assert report['revision'] == report['operation_count'], report
        assert cli('verify-store', directory / f'during-{index}.sqlite') == report
    counts = [report['operation_count'] for report in reports]
    assert counts == sorted(counts), counts


def round_trip(directory, live):
    backup = directory / 'backup.sqlite'
    report = cli('--database', live, 'backup', '--to', backup)
    assert report['schema_version'] == 3 and report['operation_count'] == 13, report
    assert report['genesis_revision'] == 0 and 'origin_revision' not in report, report
    assert pragma(backup, 'journal_mode') == 'delete'
    before = backup.read_bytes()
    cli('--database', live, 'backup', '--to', backup, error='target_exists')
    assert backup.read_bytes() == before
    exported = cli('--database', live, 'export')
    cli('restore', '--from', backup, '--to', live, error='target_exists')
    assert cli('--database', live, 'export') == exported
    restored = directory / 'restored/state.sqlite'
    restored.parent.mkdir()
    restored_report = cli('restore', '--from', backup, '--to', restored)
    assert {**restored_report, 'path': None} == {**report, 'path': None}
    assert pragma(restored, 'journal_mode') == 'wal'
    assert cli('--database', restored, 'export') == exported
    assert history(restored) == history(live)
    return backup


def damage(directory, backup):
    truncated = directory / 'truncated.sqlite'
    truncated.write_bytes(backup.read_bytes()[: backup.stat().st_size // 2])
    cli('verify-store', truncated, error='corrupt_store')
    target = directory / 'from-truncated.sqlite'
    cli('restore', '--from', truncated, '--to', target, error='corrupt_store')
    assert not target.exists()
    gap = directory / 'gap.sqlite'
    shutil.copyfile(backup, gap)
    with sqlite3.connect(gap) as connection:
        connection.execute('DELETE FROM operations WHERE sequence = 2')
    cli('verify-store', gap, error='corrupt_store')
    tampered(directory, backup)


def tampered(directory, backup):
    edits = {
        'trigger': 'CREATE TRIGGER wipe AFTER INSERT ON operations BEGIN '
                   'DELETE FROM operations WHERE sequence < NEW.sequence; END',
        'head': 'DELETE FROM operations WHERE sequence = 1',
        'snapshot': "UPDATE plan_state SET snapshot_json = "
                    "json_set(snapshot_json, '$.workspace.name', 'Rewritten')",
        'genesis': "UPDATE genesis SET plan_json = json_set(plan_json, '$.workspace.name', 'Rewritten')",
        'json': "UPDATE operations SET command_json = '{' WHERE sequence = 2",
    }
    for name, sql in edits.items():
        path = directory / f'{name}.sqlite'
        shutil.copyfile(backup, path)
        with sqlite3.connect(path) as connection:
            connection.execute(sql)
        failure = cli('verify-store', path, error='corrupt_store')['error']['message']
        assert str(path.resolve()) in failure, failure
        if name in ('snapshot', 'genesis'):
            # Replay findings are reported, never repaired.
            assert 'does not reproduce the snapshot' in failure and 'workspace' in failure, failure
            target = directory / f'{name}-restored.sqlite'
            cli('restore', '--from', path, '--to', target, error='corrupt_store')
            assert not target.exists()
    assert 'operation sequence 2 command_json' in failure, failure
    before = (directory / 'trigger.sqlite').read_bytes()
    cli('--database', directory / 'trigger.sqlite', 'unblock', 'TEST-A', error='corrupt_store')
    assert (directory / 'trigger.sqlite').read_bytes() == before
    for arguments in [('backup', '--to', directory / 'live.sqlite-journal'),
                      ('restore', '--from', backup, '--to', directory / 'x.sqlite-wal')]:
        cli('--database', directory / 'live.sqlite', *arguments, error='invalid_request')
    assert not list(directory.glob('*.sqlite-journal')) and not list(directory.glob('x.sqlite*'))


def versions(directory, backup):
    future = directory / 'future.sqlite'
    shutil.copyfile(backup, future)
    with sqlite3.connect(future) as connection:
        connection.execute('PRAGMA user_version = 99')
    before = future.read_bytes()
    for arguments in [('status', '--no-simulation'), ('claim', 'TEST-B'), ('backup', '--to', directory / 'x.sqlite')]:
        cli('--database', future, *arguments, error='unsupported_schema_version')
    cli('verify-store', future, error='unsupported_schema_version')
    assert future.read_bytes() == before and not (directory / 'x.sqlite').exists()
    retired(directory, backup)


RETIRED_LAYOUTS = {
    # Version 2 recorded only a history origin; 0 (no header) and 1 recorded nothing.
    2: 'CREATE TABLE history_origin (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), '
       'revision INTEGER NOT NULL); INSERT INTO history_origin VALUES (1, 0);',
    1: '',
    0: '',
}


def retired(directory, backup):
    for version, extra in RETIRED_LAYOUTS.items():
        old = directory / f'retired-{version}.sqlite'
        shutil.copyfile(backup, old)
        with sqlite3.connect(old) as connection:
            connection.executescript(
                'ALTER TABLE plan_state RENAME COLUMN snapshot_json TO plan_json; DROP TABLE genesis; '
                f'{extra} PRAGMA user_version = {version};')
        before = old.read_bytes()
        for arguments in [('status', '--no-simulation'), ('history',), ('export',), ('claim', 'TEST-B'),
                          ('backup', '--to', directory / 'x.sqlite'),
                          ('import', ROOT / 'tests/support/execution-plan.json')]:
            failure = cli('--database', old, *arguments, error='unsupported_schema_version')['error']['message']
            assert f'schema version {version}' in failure and 'dpm export' in failure, failure
            assert 'dpm import' in failure and 'nothing was written' in failure, failure
        cli('verify-store', old, error='unsupported_schema_version')
        assert old.read_bytes() == before, f'a retired version {version} store is never modified'
        assert not (directory / 'x.sqlite').exists()
        assert not list(directory.glob(f'retired-{version}.sqlite-*'))



def unsupported_plan_wal(directory, backup):
    """A crashed writer's committed WAL must survive rejected plan formats byte for byte."""
    for table, column in [('plan_state', 'snapshot_json'), ('genesis', 'plan_json')]:
        old = directory / f'format2-{table}.sqlite'
        shutil.copyfile(backup, old)
        writer = """
import os, sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute('PRAGMA journal_mode=WAL')
connection.execute('PRAGMA wal_autocheckpoint=0')
connection.execute(sys.argv[2])
connection.commit()
os._exit(0)
"""
        sql = f"UPDATE {table} SET {column} = json_set({column}, '$.format_version', 2)"
        subprocess.run([sys.executable, '-c', writer, str(old), sql], check=True, timeout=15)
        paths = [Path(str(old) + suffix) for suffix in ('', '-wal', '-shm')]
        before = {path: path.read_bytes() for path in paths[:2]}
        assert paths[2].exists()
        for arguments in [('status', '--no-simulation'), ('history',), ('export',),
                          ('claim', 'TEST-B'), ('backup', '--to', directory / 'old-backup.sqlite')]:
            failure = cli('--database', old, *arguments, error='corrupt_store')['error']['message']
            assert 'unsupported plan format 2' in failure and 'Preserve the original database' in failure, failure
            assert {path: path.read_bytes() for path in paths[:2]} == before, arguments
            # SQLite may refresh the transient shared index even through a read-only connection.
            assert paths[2].exists(), arguments
        assert not (directory / 'old-backup.sqlite').exists()


def golden(directory):
    """The checked-in store an earlier build wrote must still verify, replay included."""
    path = directory / 'golden.sqlite'
    with sqlite3.connect(path) as connection:
        connection.executescript((ROOT / 'tests/support/golden-v3-store.sql').read_text())
    report = cli('verify-store', path)
    assert report['schema_version'] == 3 and report['operation_count'] >= 10, report
    entries = history(path)
    assert any('ApplyChange' in entry['operation']['command'] for entry in entries), entries


def discovered(directory):
    root = directory / 'project'
    root.mkdir()
    cli('init', 'Store smoke', cwd=root)
    report = cli('verify-store', cwd=root)
    assert report['path'] == str((root / '.dpm/state.sqlite').resolve()), report
    assert report['operation_count'] == 0
    restored = cli('restore', '--from', '.dpm/state.sqlite', '--to', 'copy.sqlite', cwd=root)
    assert restored['path'] == str((root / 'copy.sqlite').resolve()), restored
    assert sorted(path.name for path in root.iterdir()) == ['.dpm', 'copy.sqlite']
    state = root / '.dpm'
    state.chmod(0o555)
    try:
        assert cli('verify-store', '.dpm/state.sqlite', cwd=root)['path'] == report['path']
    finally:
        state.chmod(0o755)
    assert sorted(path.name for path in state.iterdir()) == ['.gitignore', 'project.toml', 'state.sqlite']


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-store-') as temporary:
        directory = Path(temporary)
        os.environ['DPM_CONFIG_DIR'] = str(directory / 'config')
        live = live_store(directory)
        backups_during_writes(directory, live)
        backup = round_trip(directory, live)
        damage(directory, backup)
        versions(directory, backup)
        unsupported_plan_wal(directory, backup)
        golden(directory)
        discovered(directory)
    print('PASS: newer and retired schema versions refused unchanged with guidance, consistent backups '
          'during writes, verified restore to new paths only, exact-layout corruption detection, '
          'replay from genesis reproducing the snapshot with divergence reported and never restored, '
          'the checked-in golden store still replaying, '
          'side-file target refusal and read-only verification through the CLI')
