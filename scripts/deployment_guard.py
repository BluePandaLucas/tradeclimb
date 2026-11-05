"""Read-only deployment preflight for work that a TC restart would interrupt."""
from contextlib import closing
import pathlib
import sqlite3


TERMINAL = ('completed', 'failed', 'cancelled', 'interrupted')


def require_idle(data_dir):
    database = pathlib.Path(data_dir) / 'tradeclimb.db'
    if not database.exists():
        return  # Initial installation has no work to interrupt.
    try:
        with closing(sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True, timeout=5)) as db:
            db.execute('BEGIN')  # One consistent snapshot of jobs and builds.
            active = []
            for kind, table, predicate in (
                ('evaluation', 'jobs', '1'),
                ('candidate', 'objects', "kind='candidate'"),
            ):
                rows = db.execute(
                    f"SELECT id, COALESCE(json_extract(data,'$.state'),'unknown') "
                    f"FROM {table} WHERE {predicate} AND "
                    "COALESCE(json_extract(data,'$.state'),'unknown') NOT IN (?,?,?,?)",
                    TERMINAL,
                ).fetchall()
                active.extend(f'{kind} {identifier} ({state})' for identifier, state in rows)
            if db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='final_test'").fetchone():
                active.extend(f'final evaluation {identifier} ({state})' for identifier, state in
                              db.execute("SELECT id,state FROM final_test WHERE state='running'"))
    except sqlite3.Error as error:
        raise SystemExit('Deployment refused: could not verify idle TC state; no restart performed.') from error
    if active:
        raise SystemExit('Deployment refused: active TC work: ' + ', '.join(active) +
                         '. Wait for terminal state, then rerun installation. No restart performed.')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('data_dir', type=pathlib.Path)
    require_idle(parser.parse_args().data_dir)
    print('TC has no active evaluations or candidate builds.')
