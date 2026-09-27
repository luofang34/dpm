#!/usr/bin/env python3
"""Exercise schema versioning, backup, restore and store verification through the real CLI."""
import json
import os
import shutil
import sqlite3
import subprocess
import tempfile
import threading
from pathlib import Path

from smoke_agent import CLI, ROOT, with_actor


def cli(*arguments, cwd=ROOT, error=None):
    result = subprocess.run([str(CLI), '--json', *map(str, with_actor(arguments))], cwd=cwd,
                            capture_output=True, text=True, timeout=30)
    value = json.loads(result.stdout)
    if error:
        assert result.returncode != 0 and value['error']['code'] == error, (arguments, value, result.stderr)
    else:
        assert result.returncode == 0, (arguments, value, result.stderr)
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
    assert pragma(live, 'user_version') == 2
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
    assert report['schema_version'] == 2 and report['operation_count'] == 13, report
    assert report['origin_revision'] == 0 and report['first_base_revision'] == 0, report
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
        'json': "UPDATE operations SET command_json = '{' WHERE sequence = 2",
    }
    for name, sql in edits.items():
        path = directory / f'{name}.sqlite'
        shutil.copyfile(backup, path)
        with sqlite3.connect(path) as connection:
            connection.execute(sql)
        failure = cli('verify-store', path, error='corrupt_store')['error']['message']
        assert str(path.resolve()) in failure, failure
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
    baseline = directory / 'baseline.sqlite'
    shutil.copyfile(backup, baseline)
    with sqlite3.connect(baseline) as connection:
        connection.executescript('DROP TABLE history_origin; PRAGMA user_version = 0;')
    report = cli('verify-store', baseline)
    assert report['schema_version'] == 0 and report['origin_revision'] is None, report
    before = baseline.read_bytes()
    cli('--database', baseline, 'import', ROOT / 'tests/support/execution-plan.json', error='storage_error')
    assert baseline.read_bytes() == before, 'a refused import writes nothing'
    revision = cli('--database', baseline, 'status', '--no-simulation')['revision']
    assert pragma(baseline, 'user_version') == 0, 'reads never stamp'
    cli('--database', baseline, '--base-revision', revision, 'block', 'TEST-A', 'stamped')
    assert pragma(baseline, 'user_version') == 2
    report = cli('verify-store', baseline)
    assert report['operation_count'] == 14 and report['origin_revision'] == 0, report


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
        discovered(directory)
    print('PASS: schema version refusal and origin upgrade, consistent backups during writes, '
          'verified restore to new paths only, exact-layout and history-origin corruption detection, '
          'side-file target refusal and read-only verification through the CLI')
