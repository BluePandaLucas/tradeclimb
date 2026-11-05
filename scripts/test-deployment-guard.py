"""Disposable database and real installer refusal checks; never restart services."""
from contextlib import closing
import json
import os
import pathlib
import sqlite3
import subprocess
import tempfile
import unittest
from deployment_guard import require_idle


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = pathlib.Path(self.temporary.name)
        self.data = self.home / '.local/share/tradeclimb'
        self.data.mkdir(parents=True)
        self.database = self.data / 'tradeclimb.db'
        with closing(sqlite3.connect(self.database)) as db, db:
            db.executescript('CREATE TABLE jobs(id TEXT,data TEXT); '
                             'CREATE TABLE objects(kind TEXT,id TEXT,data TEXT); '
                             'CREATE TABLE final_test(id INTEGER,state TEXT);')

    def test_terminal_records_are_allowed_and_remain_unchanged(self):
        with closing(sqlite3.connect(self.database)) as db, db:
            for state in ('completed', 'failed', 'cancelled', 'interrupted'):
                db.execute('INSERT INTO jobs VALUES(?,?)', (state, json.dumps({'state': state})))
                db.execute('INSERT INTO objects VALUES(?,?,?)', ('candidate', state, json.dumps({'state': state})))
        before = self.database.read_bytes()
        require_idle(self.data)
        self.assertEqual(self.database.read_bytes(), before)
        require_idle(self.home / 'not-installed')

    def test_active_and_unknown_states_refuse_restart(self):
        for table, states in [('jobs', ('queued', 'preparing', 'running', 'unknown')),
                              ('objects', ('queued', 'building', 'preflight', 'unknown'))]:
            for state in states:
                with self.subTest(table=table, state=state):
                    with closing(sqlite3.connect(self.database)) as db, db:
                        db.execute('DELETE FROM jobs')
                        db.execute('DELETE FROM objects')
                        if table == 'jobs':
                            db.execute('INSERT INTO jobs VALUES(?,?)', ('fixture', json.dumps({'state': state})))
                        else:
                            db.execute('INSERT INTO objects VALUES(?,?,?)', ('candidate', 'fixture', json.dumps({'state': state})))
                    with self.assertRaisesRegex(SystemExit, 'Deployment refused: active TC work'):
                        require_idle(self.data)
        with closing(sqlite3.connect(self.database)) as db, db:
            db.execute('DELETE FROM objects')
            db.execute("INSERT INTO final_test VALUES(1,'running')")
        with self.assertRaisesRegex(SystemExit, 'final evaluation'):
            require_idle(self.data)

    def test_unreadable_schema_fails_closed(self):
        with closing(sqlite3.connect(self.database)) as db, db:
            db.execute('DROP TABLE jobs')
        with self.assertRaisesRegex(SystemExit, 'could not verify idle'):
            require_idle(self.data)

    def test_installer_aborts_before_staging_or_service_actions(self):
        with closing(sqlite3.connect(self.database)) as db, db:
            db.execute('INSERT INTO jobs VALUES(?,?)', ('active-fixture', json.dumps({'state': 'running'})))
        result = subprocess.run([str(pathlib.Path(__file__).with_name('install-service')),
                                 '--lan-ip', '192.168.0.223', '--lan-cidr', '192.168.0.0/24'],
                                env={**os.environ, 'HOME': str(self.home)}, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('evaluation active-fixture (running)', result.stderr)
        self.assertFalse((self.home / '.local/lib/tradeclimb').exists())
        self.assertFalse((self.home / '.config/systemd').exists())


if __name__ == '__main__':
    unittest.main()
