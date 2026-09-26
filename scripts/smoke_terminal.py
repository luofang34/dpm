#!/usr/bin/env python3
"""Exercise the operator console with a real controlling terminal and external mutations."""
import errno
import fcntl
import os
import pty
import select
import sqlite3
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

from smoke_agent import CLI, ROOT, run_cli


class Console:
    def __init__(self, database):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.size(130, 42)

        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [str(CLI), '--database', str(database), 'tui'], cwd=ROOT,
            stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env={**os.environ, 'TERM': 'xterm-256color', 'NO_COLOR': '1'},
            preexec_fn=controlling_terminal,
        )
        os.close(self.slave)
        self.output = b''
        self.reply_offset = 0

    def size(self, columns, rows):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))

    def read_until(self, marker, start=0):
        deadline = time.monotonic() + 15
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

    def key(self, keys, marker):
        start = len(self.output)
        os.write(self.master, keys)
        self.read_until(marker, start)

    def stop(self, key):
        self.key(key, b'\x1b[?1049l')
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
        console.key(b'r', b'waiting-terminal-refresh')
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


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-terminal-') as temporary:
        smoke(Path(temporary))
    print('PASS: real terminal Gantt keys, Detail, external agent refresh, read-only navigation and q/Ctrl-C cleanup')
