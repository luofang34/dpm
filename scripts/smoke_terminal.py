#!/usr/bin/env python3
"""Exercise the operator console with a real controlling terminal and external mutations."""
import errno
import fcntl
import json
import os
import pty
import select
import signal
import sqlite3
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

from smoke_agent import CLI, ROOT, run_cli


class Console:
    def __init__(self, database=None, project=None):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.size(130, 42)

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        selection = ['--project', str(project)] if project else ['--database', str(database)]
        self.process = subprocess.Popen(
            [str(CLI), *selection, 'tui'], cwd=ROOT,
            stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env={**os.environ, 'TERM': 'xterm-256color', 'NO_COLOR': '1'},
            preexec_fn=controlling_terminal,
        )
        os.close(self.slave)
        self.output = b''
        self.reply_offset = 0

    def size(self, columns, rows):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))

    def read_until(self, marker, start=0, timeout=15):
        deadline = time.monotonic() + timeout
        while marker not in self.output[start:]:
            remaining = deadline - time.monotonic()
            assert remaining > 0, (marker, self.output[-6000:])
            readable, _, _ = select.select([self.master], [], [], remaining)
            assert readable, (marker, self.output[-6000:])
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    raise AssertionError((marker, self.output[-6000:])) from error
                raise
            assert chunk, (marker, self.process.poll(), self.output[-6000:])
            self.output += chunk
            while (index := self.output.find(b'\x1b[6n', self.reply_offset)) >= 0:
                os.write(self.master, b'\x1b[1;1R')
                self.reply_offset = index + 4

    def appears(self, marker):
        """Wait for text the console shows by itself, with no key press.

        A frame only writes the cells that changed, so text drawn over earlier text can arrive in
        pieces. Each resize makes the console repaint every cell, so the whole marker is sent at
        least once after the change.
        """
        deadline = time.monotonic() + 15
        columns = 130
        while True:
            start = len(self.output)
            try:
                self.read_until(marker, start, timeout=1.5)
                return
            except AssertionError:
                assert time.monotonic() < deadline, (marker, self.output[-6000:])
            columns = 261 - columns
            self.size(columns, 42)

    def key(self, keys, marker):
        start = len(self.output)
        os.write(self.master, keys)
        self.read_until(marker, start)

    def stop(self, key=None, signal_number=None):
        if signal_number is None:
            self.key(key, b'\x1b[?1049l')
        else:
            # An external signal, unlike the ^C key, is not delivered through raw-mode input.
            start = len(self.output)
            os.kill(self.process.pid, signal_number)
            self.read_until(b'\x1b[?1049l', start)
        # Drain output before waiting: a PTY close can wait for its final cursor bytes.
        deadline = time.monotonic() + 15
        while True:
            remaining = deadline - time.monotonic()
            assert remaining > 0, self.output[-6000:]
            readable, _, _ = select.select([self.master], [], [], remaining)
            assert readable, self.output[-6000:]
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not chunk:
                break
            self.output += chunk
        self.process.wait(timeout=5)
        assert self.process.returncode == 0, self.output[-6000:]
        assert b'\x1b[?1000l' in self.output, 'mouse capture not disabled'
        assert termios.tcgetattr(self.master) == self.original, 'raw mode not restored'

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)



def smoke(directory):
    database = directory / 'terminal.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    initial = run_cli(database, 'export')
    console = Console(database)
    try:
        console.read_until(b'Recommended')
        console.key(b'g', b'Gantt')
        # Each navigation key must produce a frame before the next event is sent.
        for key in [b'\x1b[C', b'\x1b[D', b'+', b'-', b'\x1b[F', b'\x1b[H', b'f']:
            console.key(key, b'\x1b[?25l')
        console.key(b'\r', b'Objective:')
        console.key(b'1', b'Recommended')
        run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:terminal')
        run_cli(database, 'block', 'TEST-A', 'waiting-terminal-refresh', '--actor', 'agent:terminal')
        # Another process commits; the console follows without a key press.
        console.appears(b'waiting-terminal-refresh')
        console.stop(b'q')
    finally:
        console.close()
    # Terminal navigation and reload may not append any domain operations.
    final = run_cli(database, 'export')
    assert final['revision'] == initial['revision'] + 2
    with sqlite3.connect(database) as connection:
        assert connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0] == 2
    interrupted = Console(database)
    try:
        interrupted.read_until(b'waiting-terminal-refresh')
        interrupted.stop(b'\x03')
    finally:
        interrupted.close()
    for signal_number in (signal.SIGINT, signal.SIGTERM):
        signalled = Console(database)
        try:
            signalled.read_until(b'waiting-terminal-refresh')
            signalled.stop(signal_number=signal_number)
        finally:
            signalled.close()


def locate(project, workspace, database):
    (project / '.dpm/project.toml').write_text(
        f"version = 3\nworkspace = '{workspace}'\ndatabase = '{database}'\n")


def restored_copy(directory):
    """A locator repointed at a restored older copy is reported, not shown as current."""
    project = directory / 'restored-project'
    (project / '.dpm').mkdir(parents=True)
    live = project / '.dpm/state.sqlite'
    run_cli(live, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    workspace = run_cli(live, 'export')['workspace']['id']
    locate(project, workspace, 'state.sqlite')
    backup = directory / 'restored-backup.sqlite'
    run_cli(live, 'backup', '--to', str(backup))
    run_cli(live, 'claim', 'TEST-A', '--actor', 'agent:terminal')
    run_cli(live, 'block', 'TEST-A', 'blocked-after-backup', '--actor', 'agent:terminal')
    console = Console(project=project)
    try:
        console.read_until(b'blocked-after-backup')
        restored = project / '.dpm/restored.sqlite'
        report = subprocess.run(
            [str(CLI), '--json', 'restore', '--from', str(backup), '--to', str(restored)],
            cwd=directory, capture_output=True, text=True, timeout=15, check=True)
        lineage = json.loads(report.stdout)['data']['lineage_id']
        locate(project, workspace, 'restored.sqlite')
        # The older revision of another lineage is reported while the last snapshot stays.
        console.appears(b'STALE')
        console.appears(lineage.encode())
        console.key(b'r', b'\x1b[?25l')
        # After the operator accepts it, the restored history is followed like any other.
        run_cli(restored, 'claim', 'TEST-A', '--actor', 'agent:terminal')
        run_cli(restored, 'block', 'TEST-A', 'restored-continues', '--actor', 'agent:terminal')
        console.appears(b'restored-continues')
        console.stop(b'q')
    finally:
        console.close()
    assert run_cli(live, 'export')['revision'] == 2


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-terminal-') as temporary:
        smoke(Path(temporary))
        restored_copy(Path(temporary))
    print('PASS: real terminal Gantt keys, Detail, external agent changes followed without a key, '
          'a restored older copy reported until accepted, read-only navigation and '
          'q/Ctrl-C/SIGINT/SIGTERM cleanup')
