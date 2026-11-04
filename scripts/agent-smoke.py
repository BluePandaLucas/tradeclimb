#!/usr/bin/env python3
"""Exercise the source-to-development agent loop with isolated synthetic data."""

import argparse
import importlib.util
import json
import os
import pathlib
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.parse
import uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("tc_agent", ROOT / "scripts/tc_agent.py")
agent = importlib.util.module_from_spec(spec)
spec.loader.exec_module(agent)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--agent-account")
args = parser.parse_args()


def until(fn, seconds=90):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except OSError:
            pass
        time.sleep(0.1)
    raise AssertionError("agent workflow timed out")


def sandbox_scopes():
    result = subprocess.run(
        [
            "systemctl",
            "--user",
            "list-units",
            "--type=scope",
            "--state=active",
            "--no-legend",
            "--plain",
            "--no-pager",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return {
        line.split()[0]
        for line in result.stdout.splitlines()
        if line.startswith("tc-sandbox-")
    }


with tempfile.TemporaryDirectory(prefix="tc-agent-tools-") as folder:
    root = pathlib.Path(folder)
    data = root / "data"
    binary = ROOT / "target/release/tc"

    def cli(*arguments):
        result = subprocess.run(
            [str(binary), "--data-dir", str(data), *arguments],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        )
        return json.loads(result.stdout)

    demo = cli("demo")
    campaign = cli("list")["campaigns"][0]
    start = campaign["development_start"]
    minute = 60_000
    # Keep real compiled-strategy QA short; these are disposable synthetic records.
    campaign.update(
        development_end=start + 4 * 1440 * minute,
        validation_start=start + 4 * 1440 * minute,
        validation_end=start + 5 * 1440 * minute,
    )
    with sqlite3.connect(data / "tradeclimb.db") as db:
        db.execute("delete from jobs")
        db.execute(
            "update objects set data=? where kind='campaign' and id=?",
            (json.dumps(campaign), campaign["id"]),
        )
    keyfile = root / "agent.key"
    issued = cli(
        "auth", "issue-agent", "--name", "agent-loop", "--output", str(keyfile)
    )
    otherfile = root / "other.key"
    other = cli("auth", "issue-agent", "--name", "other", "--output", str(otherfile))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    log = open(root / "service.log", "w+")
    original_scopes = sandbox_scopes()

    def launch():
        service = subprocess.Popen(
            [
                str(binary),
                "--data-dir",
                str(data),
                "serve",
                "--bind",
                f"127.0.0.1:{port}",
                "--assets",
                str(ROOT / "dist"),
            ],
            cwd=ROOT,
            stdout=log,
            stderr=log,
        )
        until(lambda: (data / "auth/agent-endpoint.json").exists())

        def ready():
            endpoint = json.loads((data / "auth/agent-endpoint.json").read_text())[
                "socket"
            ]
            client = agent.Client(socket_path=endpoint, token_file=keyfile)
            return client if client.request("/api/agent/capabilities") else None

        return service, until(ready)

    service = None
    try:
        service, client = launch()
        endpoint = client.socket
        second = agent.Client(socket_path=endpoint, token_file=otherfile)

        def denied(path, body=None, status=400, which=client):
            try:
                which.request(path, body)
            except agent.ApiError as error:
                assert error.status == status, (path, error.status, str(error))
            else:
                raise AssertionError("unexpected access: " + path)

        caps = client.request("/api/agent/capabilities")
        assert caps["objective"] == "sharpe_daily_v1"
        assert caps["limits"]["trace_frames"] == 512
        project = root / "project"
        kit = client.initialize(project)
        assert kit["platform"] == caps["platform"]
        assert (project / "sdk/crates/tc-sdk/src/lib.rs").is_file()
        source = (project / "src/main.rs").read_text()
        link = project / "src/leak.rs"
        link.symlink_to(keyfile)
        try:
            agent.read_source(project)
        except ValueError:
            pass
        else:
            raise AssertionError("client followed a source symlink")
        link.unlink()
        os.mkfifo(link)
        try:
            agent.read_source(project)
        except ValueError:
            pass
        else:
            raise AssertionError("client read a special file")
        link.unlink()
        # Exercise the route's larger source limit, without changing strategy behavior.
        (project / "src/main.rs").write_text(
            source + "\n/*" + "fixture " * 7000 + "*/\n"
        )
        (project / "src/notes.rs").write_text("//" + "fixture " * 2000)
        print("building valid candidate", flush=True)
        first = client.submit(project, "Agent fixture")
        assert client.submit(project, "Agent fixture")["id"] == first["id"]
        denied("/api/agent/candidates/" + first["id"], status=403, which=second)
        first = client.wait("candidate", first["id"])
        assert first["state"] == "completed", first
        assert (
            first["preflight"]["passed"]
            and not first["preflight"]["performance_evidence"]
        )
        strategy = first["strategy"]
        trace_request = {"start": start, "end": start + 10 * minute, "max_frames": 3}
        job = client.run(strategy, campaign["id"], trace=trace_request)
        assert (
            client.run(strategy, campaign["id"], trace=trace_request)["id"] == job["id"]
        )
        job = client.wait("job", job["id"])
        assert job["state"] == "completed", job
        trace = client.request("/api/agent/jobs/" + job["id"] + "/trace")
        assert trace["truncated"] and len(trace["events"]) == 3
        assert trace["events"][0]["intents"] and not trace["events"][0]["fills"]
        assert trace["events"][1]["fills"][0]["time"] == start + minute
        diagnostics = client.request("/api/agent/jobs/" + job["id"] + "/diagnostics")
        assert (
            diagnostics["metrics"]["sharpe_days"] == 4
            and diagnostics["failure"] is None
        )
        query = {
            "campaign": campaign["id"],
            "market": "BTC-EUR",
            "start": start,
            "end": start + 5 * minute,
            "limit": 2,
        }

        def candles(q):
            return client.request("/api/agent/candles?" + urllib.parse.urlencode(q))

        page = candles(query)
        assert page["synthetic"] and page["next_start"] == start + 2 * minute
        assert len(page["candles"]) == 2 and page["missing_minutes"] == 0
        assert (
            candles({**query, "start": page["next_start"]})["candles"][0]["time"]
            == start + 2 * minute
        )
        denied(
            "/api/agent/candles?"
            + urllib.parse.urlencode(
                {
                    **query,
                    "start": campaign["validation_start"],
                    "end": campaign["validation_end"],
                }
            )
        )
        denied(
            "/api/agent/candles?"
            + urllib.parse.urlencode(
                {**query, "start": 1785542400000, "end": 1785542460000}
            )
        )
        denied(
            "/api/jobs",
            {
                "strategy": strategy,
                "campaign": campaign["id"],
                "period": "validation",
                "idempotency_key": "deny",
            },
            403,
        )
        denied(
            "/api/jobs",
            {
                "strategy": strategy,
                "campaign": campaign["id"],
                "period": "development",
                "trace": {**trace_request, "end": campaign["validation_end"]},
                "idempotency_key": "bad-trace",
            },
        )
        bad_body = {
            "platform": caps["platform"],
            "idempotency_key": "bad-path",
            "name": "Invalid",
            "markets": ["BTC-EUR"],
            "files": {"src/main.rs": source, "build.rs": "fn main() {}"},
        }
        denied("/api/agent/candidates", bad_body)
        denied(
            "/api/agent/candidates",
            {**bad_body, "files": {"src/main.rs": source, "src/../leak.rs": ""}},
        )
        denied(
            "/api/agent/candidates",
            {**bad_body, "files": {"src/main.rs": source}, "platform": "stale"},
        )
        (project / "src/main.rs").write_text("this is not Rust")
        print("checking compiler diagnostics", flush=True)
        invalid = client.wait("candidate", client.submit(project, "Bad compile")["id"])
        assert (
            invalid["state"] == "failed"
            and invalid["diagnostic"]["code"] == "compile_failed"
        ), invalid
        assert "src/main.rs" in invalid["diagnostic"]["log"]
        (project / "src/main.rs").write_text(
            source.replace("Decimal::new(5, 1)", "Decimal::new(2, 0)")
        )
        print("checking preflight rejection", flush=True)
        invalid = client.wait("candidate", client.submit(project, "Bad weight")["id"])
        assert (
            invalid["state"] == "failed"
            and invalid["diagnostic"]["code"] == "preflight_failed"
        ), invalid
        assert invalid["strategy"] is None
        # Preflight dates are short; a later runtime error should preserve its stderr.
        failure_source = source.replace(
            "if frame.warmup",
            f'if frame.time >= {start + 1440 * minute} {{ eprintln!("author debug marker"); anyhow::bail!("deliberate runtime fixture"); }}\n        if frame.warmup',
        )
        (project / "src/main.rs").write_text(failure_source)
        print("building revision and checking runtime diagnostics", flush=True)
        revised = client.wait(
            "candidate", client.submit(project, "Agent revision", parent=strategy)["id"]
        )
        assert revised["state"] == "completed", revised
        failed = client.wait(
            "job",
            client.run(
                revised["strategy"],
                campaign["id"],
                trace={
                    "start": start + 1438 * minute,
                    "end": start + 1441 * minute,
                    "max_frames": 8,
                },
            )["id"],
        )
        assert failed["state"] == "failed", failed
        diagnostic = client.request("/api/agent/jobs/" + failed["id"] + "/diagnostics")
        assert diagnostic["failure"]["code"] == "strategy_exit", diagnostic
        assert "author debug marker" in diagnostic["failure"]["message"], diagnostic
        assert client.request("/api/agent/jobs/" + failed["id"] + "/trace")["events"][
            -1
        ]["error"]
        (project / "src/main.rs").write_text(source)
        pending = client.submit(project, "Cancel fixture")
        until(
            lambda: client.request("/api/agent/candidates/" + pending["id"])["state"]
            == "building"
        )
        time.sleep(0.3)
        assert (
            client.request("/api/agent/candidates/" + pending["id"] + "/cancel", {})[
                "state"
            ]
            == "cancelled"
        )
        assert (
            client.request("/api/agent/candidates/" + pending["id"] + "/cancel", {})[
                "state"
            ]
            == "cancelled"
        )
        pending = second.submit(project, "Revoke fixture")
        until(
            lambda: second.request("/api/agent/candidates/" + pending["id"])["state"]
            == "building"
        )
        cli("auth", "revoke", other["id"])
        denied("/api/agent/candidates/" + pending["id"], status=401, which=second)
        interrupted = client.submit(project, "Restart fixture")
        until(
            lambda: client.request("/api/agent/candidates/" + interrupted["id"])[
                "state"
            ]
            == "building"
        )
        service.send_signal(signal.SIGINT)
        service.wait(timeout=10)
        assert service.returncode == 0
        service, client = launch()
        assert (
            client.request("/api/agent/candidates/" + interrupted["id"])["state"]
            == "interrupted"
        )
        until(lambda: sandbox_scopes() <= original_scopes, seconds=10)
        if args.agent_account:
            token_name = "tools-" + uuid.uuid4().hex + ".key"
            project_name = "tools-" + uuid.uuid4().hex
            home = "/var/lib/" + args.agent_account
            subprocess.run(
                [
                    "sudo",
                    "-n",
                    "install",
                    "-o",
                    args.agent_account,
                    "-g",
                    args.agent_account,
                    "-m",
                    "600",
                    str(keyfile),
                    home + "/" + token_name,
                ],
                check=True,
            )
            try:
                output = subprocess.run(
                    [
                        str(ROOT / "scripts/run-agent"),
                        "/usr/bin/python3",
                        "/tools/tc_agent.py",
                        "init",
                        project_name,
                    ],
                    cwd=ROOT,
                    env=dict(
                        os.environ,
                        TC_DATA_DIR=str(data),
                        TC_AGENT_ACCOUNT=args.agent_account,
                        TC_AGENT_TOKEN_NAME=token_name,
                    ),
                    capture_output=True,
                    text=True,
                    check=True,
                )
                assert json.loads(output.stdout)["source"] == "src/main.rs"
            finally:
                subprocess.run(
                    [
                        "sudo",
                        "-n",
                        "rm",
                        "-rf",
                        home + "/" + token_name,
                        home + "/" + project_name,
                    ],
                    check=True,
                )
        with sqlite3.connect(data / "tradeclimb.db") as db:
            strategies = [
                json.loads(row[0])
                for row in db.execute("select data from objects where kind='strategy'")
            ]
            assert (
                len(strategies) == 5
            )  # Three demo versions and two admitted candidates.
            revision = next(s for s in strategies if s["id"] == revised["strategy"])
            assert revision["parent"] == strategy
            assert (
                db.execute(
                    "select count(*) from objects where kind='final_test'"
                ).fetchone()[0]
                == 0
            )
        budget = cli("budget")
        assert budget["used"] == 0 and not budget["revealed"]
        print(
            "PASS: pinned kit, offline builds, preflight, lineage, dev data/traces, useful failures, isolation, cancellation/revocation/restart; validation/final untouched"
        )
    except BaseException:
        log.flush()
        print((root / "service.log").read_text()[-10000:])
        raise
    finally:
        if service is not None and service.poll() is None:
            service.send_signal(signal.SIGINT)
            service.wait(timeout=10)
        log.close()
