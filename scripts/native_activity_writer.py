#!/usr/bin/env python3
"""Write run activity into a disposable store through one persistent agent connection, so the
native suite can reach the store's real retention limit (10,000 records) in seconds rather than the
minutes a process per record would take. Every record goes through the shared application, exactly
as `dpm run record` does; nothing touches the database directly.

usage: native_activity_writer.py DATABASE RUN FIRST_SEQUENCE COUNT
"""
import sys

from smoke_agent import Agent

ACTOR = 'service:dpm-claude'


def main(database, run, first, count):
    agent = Agent(database, ACTOR)
    try:
        for sequence in range(first, first + count):
            agent.call('record_run_activity', {'run': run, 'source_sequence': sequence, 'kind': 'tool_result', 'text': f'record {sequence}'})
    finally:
        agent.close()
    print(f'wrote {count} records from {first}')


if __name__ == '__main__':
    if len(sys.argv) != 5:
        raise SystemExit(__doc__)
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
